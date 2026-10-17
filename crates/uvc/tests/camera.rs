use std::cell::RefCell;
use std::collections::HashMap;

use camctl_uvc::{
    Camera, CameraError, Control, Request, RequestCode, Transport, TransportError, Value,
    parse_configuration,
};

const TINY2_LITE: &[u8] = include_bytes!("fixtures/obsbot-tiny-2-lite-config.bin");

/// Records every request and answers GETs from a table keyed by
/// (request code, wValue, wIndex). Anything unanswered stalls.
#[derive(Default)]
struct FakeTransport {
    answers: HashMap<(RequestCode, u16, u16), Vec<u8>>,
    log: RefCell<Vec<(Request, Option<Vec<u8>>)>>,
}

impl FakeTransport {
    fn answer(mut self, code: RequestCode, value: u16, index: u16, bytes: &[u8]) -> Self {
        self.answers.insert((code, value, index), bytes.to_vec());
        self
    }

    fn sent(&self) -> Vec<(Request, Option<Vec<u8>>)> {
        self.log.borrow().clone()
    }
}

impl Transport for FakeTransport {
    fn control_in(&self, req: &Request) -> Result<Vec<u8>, TransportError> {
        self.log.borrow_mut().push((*req, None));
        self.answers
            .get(&(req.code, req.value, req.index))
            .cloned()
            .ok_or(TransportError::Stall)
    }

    fn control_out(&self, req: &Request, data: &[u8]) -> Result<(), TransportError> {
        self.log.borrow_mut().push((*req, Some(data.to_vec())));
        Ok(())
    }
}

fn camera(t: FakeTransport) -> Camera<FakeTransport> {
    Camera::new(parse_configuration(TINY2_LITE).unwrap(), t)
}

use RequestCode::{GetCur, GetDef, GetInfo, GetMax, GetMin, GetRes, SetCur};
const ZOOM: u16 = 0x0b00;
const PANTILT: u16 = 0x0d00;
const AE_MODE: u16 = 0x0200;
const CT: u16 = 0x0100; // unit 1, interface 0
const PU: u16 = 0x0300; // unit 3, interface 0

#[test]
fn requests_carry_uvc_class_interface_request_types() {
    let (sent, _) = {
        let cam = camera(FakeTransport::default().answer(GetCur, ZOOM, CT, &[37, 0]));
        assert_eq!(cam.get(Control::ZoomAbsolute).unwrap(), Value::Int(37));
        cam.transport().sent()[0].clone()
    };
    assert_eq!(
        (
            sent.request_type(),
            sent.code,
            sent.value,
            sent.index,
            sent.length
        ),
        (0xa1, GetCur, ZOOM, CT, 2)
    );
    assert_eq!(sent.code as u8, 0x81);
}

#[test]
fn get_pan_tilt_decodes_signed_arc_seconds() {
    // pan = 32400 (9 deg), tilt = -302400 (-84 deg), as read from the camera.
    let mut bytes = 32400i32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&(-302_400i32).to_le_bytes());
    let cam = camera(FakeTransport::default().answer(GetCur, PANTILT, CT, &bytes));
    let v = cam.get(Control::PanTiltAbsolute).unwrap();
    assert_eq!(
        v,
        Value::PanTilt {
            pan: 32400,
            tilt: -302_400
        }
    );
    assert_eq!(v.pan_tilt_degrees(), Some((9.0, -84.0)));
}

#[test]
fn set_pan_tilt_encodes_eight_bytes_and_uses_set_cur() {
    let cam = camera(FakeTransport::default());
    cam.set(
        Control::PanTiltAbsolute,
        Value::pan_tilt_from_degrees(-20.0, 5.5).unwrap(),
    )
    .unwrap();
    let (req, data) = &cam.transport().sent()[0];
    assert_eq!(
        (
            req.request_type(),
            req.code,
            req.value,
            req.index,
            req.length
        ),
        (0x21, SetCur, PANTILT, CT, 8)
    );
    let mut want = (-72_000i32).to_le_bytes().to_vec();
    want.extend_from_slice(&19_800i32.to_le_bytes());
    assert_eq!(data.as_deref(), Some(want.as_slice()));
}

#[test]
fn processing_unit_controls_address_their_unit() {
    let cam = camera(FakeTransport::default().answer(GetCur, 0x0400, PU, &[0x10, 0x00]));
    assert_eq!(cam.get(Control::Gain).unwrap(), Value::Int(16));
}

#[test]
fn signed_scalars_sign_extend() {
    let cam = camera(FakeTransport::default().answer(GetCur, 0x0200, PU, &[0xf6, 0xff]));
    assert_eq!(cam.get(Control::Brightness).unwrap(), Value::Int(-10));
}

#[test]
fn range_reads_min_max_res_and_default() {
    let t = FakeTransport::default()
        .answer(GetMin, ZOOM, CT, &[0, 0])
        .answer(GetMax, ZOOM, CT, &[100, 0])
        .answer(GetRes, ZOOM, CT, &[1, 0])
        .answer(GetDef, ZOOM, CT, &[0, 0]);
    let r = camera(t).range(Control::ZoomAbsolute).unwrap();
    assert_eq!(
        (r.min, r.max, r.step, r.default),
        (
            Some(Value::Int(0)),
            Some(Value::Int(100)),
            Some(Value::Int(1)),
            Some(Value::Int(0))
        )
    );
}

#[test]
fn range_keeps_what_the_device_answers_when_some_requests_stall() {
    // AE mode only has to answer GET_RES (the supported-modes bitmap) and
    // GET_DEF; GET_MIN and GET_MAX may stall.
    let t = FakeTransport::default()
        .answer(GetRes, AE_MODE, CT, &[0b1011])
        .answer(GetDef, AE_MODE, CT, &[0b0010]);
    let r = camera(t).range(Control::AeMode).unwrap();
    assert_eq!((r.min, r.max), (None, None));
    assert_eq!(r.step, Some(Value::Int(0b1011)));
    assert_eq!(r.default, Some(Value::Int(0b0010)));
}

#[test]
fn info_reports_capability_bits() {
    let cam = camera(FakeTransport::default().answer(GetInfo, ZOOM, CT, &[0b0000_0111]));
    let info = cam.info(Control::ZoomAbsolute).unwrap();
    assert!(info.can_get() && info.can_set() && info.disabled_by_auto());
    assert!(!info.autoupdate() && !info.asynchronous());
    assert_eq!(info.bits(), 0b0000_0111);
}

#[test]
fn unsupported_control_is_refused_without_touching_the_device() {
    let cam = camera(FakeTransport::default());
    assert!(matches!(
        cam.set(Control::Privacy, Value::Int(1)),
        Err(CameraError::Unsupported(Control::Privacy))
    ));
    assert!(cam.transport().sent().is_empty());
}

#[test]
fn short_read_is_an_error() {
    let cam = camera(FakeTransport::default().answer(GetCur, PANTILT, CT, &[1, 2, 3]));
    assert!(matches!(
        cam.get(Control::PanTiltAbsolute),
        Err(CameraError::Value(_))
    ));
}

#[test]
fn value_of_wrong_shape_is_refused() {
    let cam = camera(FakeTransport::default());
    let v = Value::pan_tilt_from_degrees(0.0, 0.0).unwrap();
    assert!(cam.set(Control::ZoomAbsolute, v).is_err());
    assert!(cam.transport().sent().is_empty());
}

#[test]
fn out_of_width_scalar_is_refused() {
    let cam = camera(FakeTransport::default());
    // Zoom is an unsigned 16-bit value.
    assert!(cam.set(Control::ZoomAbsolute, Value::Int(70_000)).is_err());
    assert!(cam.set(Control::ZoomAbsolute, Value::Int(-1)).is_err());
    assert!(cam.transport().sent().is_empty());
}

#[test]
fn transport_errors_name_the_control_and_request() {
    let cam = camera(FakeTransport::default());
    let err = cam.get(Control::ZoomAbsolute).unwrap_err();
    assert!(matches!(
        err,
        CameraError::Request {
            control: Control::ZoomAbsolute,
            code: GetCur,
            source: TransportError::Stall,
        }
    ));
    assert_eq!(err.to_string(), "Zoom: GET_CUR failed");
}
