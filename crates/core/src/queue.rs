//! Control writes off the UI thread, latest value wins.
//!
//! A slider drag or a held key asks for far more writes than a USB control
//! pipe should carry. Each control keeps only its newest requested value;
//! one worker thread sends whatever is pending, so a burst collapses to the
//! write in flight plus the final value. Reads queue the same way and run
//! after the pending writes, so they see the result.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{JoinHandle, ThreadId};

use camctl_uvc::{Camera, CameraError, Control, Transport, Value};

/// Requested values not yet written, at most one per control, in the order
/// of each control's latest request. That keeps dependent writes in the
/// order they were asked for: auto focus off, then a manual focus, say.
#[derive(Debug, Default)]
pub struct Pending {
    order: Vec<Control>,
    values: Vec<Value>,
    reads: Vec<Control>,
}

impl Pending {
    /// Request `value` for `control`, replacing any earlier pending value
    /// and moving the control to the back.
    pub fn insert(&mut self, control: Control, value: Value) {
        if let Some(i) = self.order.iter().position(|&c| c == control) {
            self.order.remove(i);
            self.values.remove(i);
        }
        self.order.push(control);
        self.values.push(value);
    }

    /// Take everything pending.
    pub fn drain(&mut self) -> Vec<(Control, Value)> {
        self.order.drain(..).zip(self.values.drain(..)).collect()
    }

    /// Ask for a read of `control`; repeated requests collapse.
    pub fn request_read(&mut self, control: Control) {
        if !self.reads.contains(&control) {
            self.reads.push(control);
        }
    }

    /// Take the requested reads.
    pub fn drain_reads(&mut self) -> Vec<Control> {
        std::mem::take(&mut self.reads)
    }

    /// Whether nothing is waiting, neither writes nor reads.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.reads.is_empty()
    }
}

/// What happened to one write.
#[derive(Debug)]
pub struct WriteOutcome {
    pub control: Control,
    pub value: Value,
    pub result: Result<(), CameraError>,
}

/// What one read returned.
#[derive(Debug)]
pub struct ReadOutcome {
    pub control: Control,
    pub result: Result<Value, CameraError>,
}

/// Reported by the worker after each write or read.
#[derive(Debug)]
pub enum QueueEvent {
    Wrote(WriteOutcome),
    Read(ReadOutcome),
}

#[derive(Debug, Default)]
struct State {
    pending: Pending,
    shutdown: bool,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

/// Owns a camera on a worker thread and applies requested values to it.
#[derive(Debug)]
pub struct ControlQueue {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl ControlQueue {
    /// Move `camera` onto a worker thread. `on_event` runs on that thread
    /// after each write and read.
    ///
    /// # Panics
    ///
    /// If the OS cannot create a thread.
    pub fn spawn<T, F>(camera: Camera<T>, on_event: F) -> ControlQueue
    where
        T: Transport + Send + 'static,
        F: Fn(QueueEvent) + Send + 'static,
    {
        let shared = Arc::new(Shared::default());
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("camctl-controls".into())
            .spawn(move || run(&worker_shared, &camera, &on_event))
            .expect("spawning a thread only fails when the OS is out of resources");
        ControlQueue {
            shared,
            worker: Some(worker),
        }
    }

    /// Request `value` for `control`. Returns at once.
    pub fn set(&self, control: Control, value: Value) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.pending.insert(control, value);
        self.shared.ready.notify_one();
    }

    /// Read `control` once the pending writes are done; the value arrives
    /// as a [`QueueEvent::Read`]. Returns at once.
    pub fn read(&self, control: Control) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.pending.request_read(control);
        self.shared.ready.notify_one();
    }

    /// The worker's thread id while it runs.
    #[must_use]
    pub fn worker_id(&self) -> Option<ThreadId> {
        self.worker.as_ref().map(|w| w.thread().id())
    }
}

impl Drop for ControlQueue {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .shutdown = true;
        self.shared.ready.notify_one();
        if let Some(worker) = self.worker.take() {
            // A panic in the worker is already reported by the panic hook;
            // there is nothing more to do with it while dropping.
            let _ = worker.join();
        }
    }
}

fn run<T: Transport>(shared: &Shared, camera: &Camera<T>, on_event: &dyn Fn(QueueEvent)) {
    loop {
        let (writes, reads) = {
            let mut state = shared
                .ready
                .wait_while(
                    shared.state.lock().unwrap_or_else(PoisonError::into_inner),
                    |s| s.pending.is_empty() && !s.shutdown,
                )
                .unwrap_or_else(PoisonError::into_inner);
            if state.shutdown {
                return;
            }
            (state.pending.drain(), state.pending.drain_reads())
        };
        for (control, value) in writes {
            let result = camera.set(control, value);
            on_event(QueueEvent::Wrote(WriteOutcome {
                control,
                value,
                result,
            }));
        }
        for control in reads {
            let result = camera.get(control);
            on_event(QueueEvent::Read(ReadOutcome { control, result }));
        }
    }
}
