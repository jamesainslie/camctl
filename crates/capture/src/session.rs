//! The AVFoundation side: devices, permission, and a capture session that
//! keeps only the newest frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeExternal, AVCaptureOutput,
    AVCaptureSession, AVCaptureSessionPresetInputPriority, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::{CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use crate::policy::{Candidate, Encoding, Want, choose};
use crate::slot::LatestSlot;

/// Why capture could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CaptureError {
    #[error("camera access has not been granted")]
    NotAuthorized,
    #[error("the camera could not be opened: {0}")]
    Input(String),
    #[error("the capture session refused the camera or the video output")]
    Session,
    #[error("the camera refused the format: {0}")]
    Configure(String),
}

/// One captured frame: an IOSurface-backed 420f `CVPixelBuffer`.
#[derive(Debug, Clone)]
pub struct Frame(CFRetained<CVPixelBuffer>);

// SAFETY: CoreVideo buffers are thread-safe CF objects: retain and release
// are atomic and attachments are internally synchronized. `Frame` exposes
// only read access (the buffer handle and its size), so moving one to
// another thread cannot race with anything it allows.
unsafe impl Send for Frame {}

impl Frame {
    /// The pixel buffer, for GPUI's `surface` element.
    #[must_use]
    pub fn pixel_buffer(&self) -> CFRetained<CVPixelBuffer> {
        self.0.clone()
    }

    #[must_use]
    pub fn size(&self) -> (usize, usize) {
        (
            CVPixelBufferGetWidth(&self.0),
            CVPixelBufferGetHeight(&self.0),
        )
    }
}

/// Camera permission, as macOS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorization {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

fn video() -> &'static objc2_av_foundation::AVMediaType {
    // SAFETY: an AVFoundation constant, always present on macOS.
    unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo exists on macOS")
}

/// Whether camctl may use the camera.
#[must_use]
pub fn authorization() -> Authorization {
    // SAFETY: a class method with a valid media type.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(video()) };
    match status {
        AVAuthorizationStatus::Authorized => Authorization::Authorized,
        AVAuthorizationStatus::Denied => Authorization::Denied,
        AVAuthorizationStatus::Restricted => Authorization::Restricted,
        _ => Authorization::NotDetermined,
    }
}

/// Ask for camera access. `done` runs on an arbitrary queue.
pub fn request_access(done: impl Fn(bool) + Send + Sync + 'static) {
    let block = RcBlock::new(move |granted: Bool| done(granted.as_bool()));
    // SAFETY: the block is copied by AVFoundation and called once.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video(), &block) };
}

/// A video capture device.
#[derive(Debug, Clone)]
pub struct Device {
    inner: Retained<AVCaptureDevice>,
    /// AVFoundation's unique ID; for UVC cameras it encodes the USB location
    /// and VID/PID (see [`crate::usb_identity`]).
    pub unique_id: String,
    pub name: String,
}

impl Device {
    /// USB or built-in, from the shape of its unique ID.
    #[must_use]
    pub fn kind(&self) -> DeviceKind {
        if crate::usb_identity(&self.unique_id).is_some() {
            DeviceKind::Usb
        } else {
            DeviceKind::BuiltIn
        }
    }

    /// Whether another app is streaming from it right now.
    #[must_use]
    pub fn in_use_by_another_app(&self) -> bool {
        // SAFETY: a property read on a live device.
        unsafe { self.inner.isInUseByAnotherApplication() }
    }

    /// The formats the device offers, with their policy description.
    #[must_use]
    pub fn candidates(&self) -> Vec<Candidate> {
        self.formats().into_iter().map(|(_, c)| c).collect()
    }

    fn formats(&self) -> Vec<(Retained<AVCaptureDeviceFormat>, Candidate)> {
        // SAFETY: property reads on a live device and its formats.
        unsafe {
            self.inner
                .formats()
                .iter()
                .map(|f| {
                    let desc = f.formatDescription();
                    let dims = CMVideoFormatDescriptionGetDimensions(&desc);
                    let max_fps = f
                        .videoSupportedFrameRateRanges()
                        .iter()
                        .map(|r| r.maxFrameRate())
                        .fold(0.0, f64::max);
                    let candidate = Candidate {
                        encoding: encoding_of(desc.media_sub_type(), self.kind()),
                        width: u32::try_from(dims.width).unwrap_or(0),
                        height: u32::try_from(dims.height).unwrap_or(0),
                        max_fps,
                    };
                    (f, candidate)
                })
                .collect()
        }
    }
}

/// Where a camera's frames come from, which decides what a format costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// A USB (UVC) camera.
    Usb,
    /// The Mac's own camera, fed by its image signal processor.
    BuiltIn,
}

/// How frames of a format with this media subtype `FourCC` arrive.
///
/// macOS hides a UVC camera's compression: its MJPEG and H.264 modes are
/// listed as `420v`, already decoded, while true raw modes keep their own
/// subtype (`yuvs` for YUY2). For the built-in camera `420v` is native.
#[must_use]
pub fn encoding_of(subtype: u32, kind: DeviceKind) -> Encoding {
    match (&subtype.to_be_bytes(), kind) {
        (b"yuvs" | b"2vuy" | b"yuv2", _) | (b"420v" | b"420f", DeviceKind::BuiltIn) => {
            Encoding::Uncompressed
        }
        (b"420v" | b"420f" | b"dmb1" | b"jpeg" | b"mjpa" | b"mjpb", DeviceKind::Usb) => {
            Encoding::Mjpeg
        }
        _ => Encoding::Other,
    }
}

/// Every external and built-in video camera.
#[must_use]
pub fn devices() -> Vec<Device> {
    // SAFETY: AVFoundation constants and a discovery session query.
    unsafe {
        let types = NSArray::from_slice(&[
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
        ]);
        let discovery =
            AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                &types,
                Some(video()),
                AVCaptureDevicePosition::Unspecified,
            );
        discovery
            .devices()
            .iter()
            .map(|d| Device {
                unique_id: d.uniqueID().to_string(),
                name: d.localizedName().to_string(),
                inner: d,
            })
            .collect()
    }
}

/// State shared between the capture queue and whoever consumes frames.
///
/// The slot being full means a wake-up is outstanding: the producer wakes
/// the consumer only when it fills an empty slot, and the consumer empties
/// it in one locked step, so no frame is ever left without a wake-up.
struct Shared {
    latest: LatestSlot<Frame>,
    delivered: AtomicU64,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("delivered", &self.delivered)
            .finish_non_exhaustive()
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; FrameSink adds no
    // Drop, and its ivars (an Arc of Send + Sync state) may be used from the
    // capture queue, which is why the class is not MainThreadOnly.
    #[unsafe(super(NSObject))]
    #[name = "CamctlFrameSink"]
    #[ivars = Arc<Shared>]
    #[derive(Debug)]
    struct FrameSink;

    // SAFETY: NSObjectProtocol has no requirements beyond being an NSObject.
    unsafe impl NSObjectProtocol for FrameSink {}

    // SAFETY: the method below has the protocol's exact signature
    // (`void (AVCaptureOutput *, CMSampleBufferRef, AVCaptureConnection *)`);
    // every other method of the protocol is optional.
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for FrameSink {
        // SAFETY: matches the protocol selector and signature, as above.
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // SAFETY: a valid sample buffer for the duration of the call;
            // the image buffer is retained before we keep it.
            let Some(buffer) = (unsafe { sample.image_buffer() }) else {
                return;
            };
            // GPUI's renderer asserts on anything but 420f; never hand it
            // another format, whatever the output was asked for.
            if CVPixelBufferGetPixelFormatType(&buffer)
                != kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            {
                return;
            }
            let shared = self.ivars();
            let was_empty = shared.latest.put(Frame(buffer));
            shared.delivered.fetch_add(1, Ordering::Relaxed);
            if was_empty {
                (shared.wake)();
            }
        }
    }
);

impl FrameSink {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(shared);
        // SAFETY: NSObject's designated initializer on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}

/// Previews never need more than this; 60 fps MJPEG costs twice the
/// decoding for no visible gain in a small window.
const PREVIEW_MAX_FPS: f64 = 30.0;

/// Cameras report rates like 30.00003 fps; treat those as 30.
const FPS_TOLERANCE: f64 = 0.01;

/// A frame duration for at most `max_fps` that the device accepts: the
/// fastest listed range at or below it, or `max_fps` itself when a
/// continuous range spans it. Devices throw for anything else.
fn frame_duration_at_most(format: &AVCaptureDeviceFormat, max_fps: f64) -> Option<CMTime> {
    // SAFETY: property reads on a live format and its ranges.
    unsafe {
        let ranges = format.videoSupportedFrameRateRanges();
        if let Some(listed) = ranges
            .iter()
            .filter(|r| r.maxFrameRate() <= max_fps + FPS_TOLERANCE)
            .max_by(|a, b| a.maxFrameRate().total_cmp(&b.maxFrameRate()))
        {
            return Some(listed.minFrameDuration());
        }
        ranges
            .iter()
            .any(|r| r.minFrameRate() <= max_fps && max_fps <= r.maxFrameRate())
            .then(|| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "frame rates are small whole numbers"
                )]
                CMTime::new(1, max_fps.round() as i32)
            })
    }
}

/// The capture session, carried onto the capture queue.
struct SessionOnQueue(Retained<AVCaptureSession>);
// SAFETY: Apple documents startRunning and stopRunning as blocking calls to
// make on a background serial queue, which is all this wrapper is used for.
// Retain and release are atomic. `Session::drop` waits for the queue with
// `exec_sync`, so every clone sent there is released before the session is.
unsafe impl Send for SessionOnQueue {}

/// A running (or stopped) capture from one camera.
#[derive(Debug)]
pub struct Session {
    capture: Retained<AVCaptureSession>,
    device: Device,
    output: Retained<AVCaptureVideoDataOutput>,
    _sink: Retained<FrameSink>,
    queue: DispatchRetained<DispatchQueue>,
    shared: Arc<Shared>,
}

impl Session {
    /// Build a stopped session for `device`. `wake` is called (on the
    /// capture queue) when a frame arrives and none is waiting to be taken.
    pub fn new(
        device: &Device,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Session, CaptureError> {
        if authorization() != Authorization::Authorized {
            return Err(CaptureError::NotAuthorized);
        }
        let shared = Arc::new(Shared {
            latest: LatestSlot::default(),
            delivered: AtomicU64::new(0),
            wake: Box::new(wake),
        });
        let queue = DispatchQueue::new("us.ainslies.camctl.capture", DispatchQueueAttr::SERIAL);
        let sink = FrameSink::new(shared.clone());
        // SAFETY: standard AVFoundation setup on live objects; the delegate
        // and queue outlive the output (both are owned by Session).
        unsafe {
            let session = AVCaptureSession::new();
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device.inner)
                .map_err(|e| CaptureError::Input(e.localizedDescription().to_string()))?;
            let output = AVCaptureVideoDataOutput::new();
            output.setAlwaysDiscardsLateVideoFrames(true);
            output.setVideoSettings(Some(&video_settings()));
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*sink)),
                Some(&queue),
            );
            session.beginConfiguration();
            let ok = session.canAddInput(&input) && session.canAddOutput(&output);
            if ok {
                session.addInput(&input);
                session.addOutput(&output);
                // Any other preset lets the session change the device's
                // active format by itself, which would also change the
                // stream of an app already using the camera.
                if session.canSetSessionPreset(AVCaptureSessionPresetInputPriority) {
                    session.setSessionPreset(AVCaptureSessionPresetInputPriority);
                }
            }
            session.commitConfiguration();
            if !ok {
                return Err(CaptureError::Session);
            }
            Ok(Session {
                capture: session,
                device: device.clone(),
                output,
                _sink: sink,
                queue,
                shared,
            })
        }
    }

    /// Stream the format `policy::choose` picks for `want`, at the fastest
    /// supported rate no higher than 30 fps. Leaves the format alone while
    /// another app is using the camera, since changing it would change
    /// their stream too. Returns what is being streamed when camctl set it.
    pub fn configure_for(&self, want: Want) -> Result<Option<Candidate>, CaptureError> {
        if self.device.in_use_by_another_app() {
            return Ok(None);
        }
        let formats = self.device.formats();
        let candidates: Vec<Candidate> = formats.iter().map(|(_, c)| *c).collect();
        let Some(index) = choose(&candidates, want) else {
            return Ok(None);
        };
        let (format, candidate) = &formats[index];
        let device = &self.device.inner;
        // SAFETY: a plain method call on a live device.
        unsafe { device.lockForConfiguration() }
            .map_err(|e| CaptureError::Configure(e.localizedDescription().to_string()))?;
        // AVFoundation throws (NSInvalidArgumentException) for a format or
        // frame duration the device does not list; catch it rather than let
        // it unwind into Rust, and unlock either way.
        let result = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the device is locked for configuration, and both values
            // come from the device's own lists.
            unsafe {
                device.setActiveFormat(format);
                if let Some(duration) = frame_duration_at_most(format, PREVIEW_MAX_FPS) {
                    device.setActiveVideoMinFrameDuration(duration);
                }
            }
        }));
        // SAFETY: balances the lock taken above.
        unsafe { device.unlockForConfiguration() };
        match result {
            Ok(()) => Ok(Some(*candidate)),
            Err(exception) => Err(CaptureError::Configure(
                exception.map_or_else(|| "unknown exception".to_owned(), |e| e.to_string()),
            )),
        }
    }

    /// Start streaming (asynchronously, on the capture queue).
    pub fn start(&self) {
        let session = SessionOnQueue(self.capture.clone());
        // SAFETY: startRunning is meant to be called off the main thread.
        self.queue.exec_async(move || {
            let session = session;
            unsafe { session.0.startRunning() }
        });
    }

    /// Stop streaming; the camera light goes off.
    pub fn stop(&self) {
        let session = SessionOnQueue(self.capture.clone());
        // SAFETY: as for start.
        self.queue.exec_async(move || {
            let session = session;
            unsafe { session.0.stopRunning() }
        });
    }

    /// The newest frame, if one arrived since the last call. Emptying the
    /// slot re-arms the wake-up, so answer every wake with a call, even when
    /// the frame will be thrown away.
    #[must_use]
    pub fn take_frame(&self) -> Option<Frame> {
        self.shared.latest.take()
    }

    /// Start a stopped stream afresh: drop a frame left over from before
    /// the stop, then start.
    pub fn start_fresh(&self) {
        let _ = self.shared.latest.take();
        self.start();
    }

    /// Run `f` on the capture queue after everything already queued there
    /// (a stop, say) has finished. Lets a caller wait for a stuck camera
    /// without blocking its own thread.
    pub fn when_idle(&self, f: impl FnOnce() + Send + 'static) {
        self.queue.exec_async(f);
    }

    /// Frames delivered by AVFoundation since the session was built.
    #[must_use]
    pub fn frames_delivered(&self) -> u64 {
        self.shared.delivered.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // `Session` is not Send (its AVFoundation fields are not), so it is
        // never dropped on the capture queue itself, where exec_sync would
        // trap.
        let session = SessionOnQueue(self.capture.clone());
        // SAFETY: as for start.
        self.queue.exec_sync(move || {
            let session = session;
            unsafe { session.0.stopRunning() }
        });
        // SAFETY: clearing the delegate with a nil queue is allowed.
        unsafe { self.output.setSampleBufferDelegate_queue(None, None) };
        // A barrier: any callback already queued finishes before the sink
        // and the shared state are released.
        self.queue.exec_sync(|| {});
    }
}

/// 420f, Metal compatible: what GPUI's surface renderer requires.
fn video_settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    let as_ns = |s: &CFString| -> &NSString {
        // SAFETY: CFString and NSString are toll-free bridged.
        unsafe { &*std::ptr::from_ref(s).cast::<NSString>() }
    };
    // SAFETY: CoreVideo constants, always present.
    let (format_key, metal_key) = unsafe {
        (
            as_ns(kCVPixelBufferPixelFormatTypeKey),
            as_ns(kCVPixelBufferMetalCompatibilityKey),
        )
    };
    let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
    let metal = NSNumber::new_bool(true);
    let objects: [&AnyObject; 2] = [&format, &metal];
    NSDictionary::from_slices(&[format_key, metal_key], &objects)
}
