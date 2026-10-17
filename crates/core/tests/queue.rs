use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camctl_core::{ControlQueue, Pending, QueueEvent};
use camctl_uvc::{Camera, Control, Request, Transport, TransportError, Value, parse_configuration};

const TINY2_LITE: &[u8] = include_bytes!("../../uvc/tests/fixtures/obsbot-tiny-2-lite-config.bin");

#[test]
fn pending_keeps_only_the_latest_value_per_control() {
    let mut p = Pending::default();
    p.insert(Control::ZoomAbsolute, Value::Int(10));
    p.insert(Control::Gain, Value::Int(5));
    p.insert(Control::ZoomAbsolute, Value::Int(40));
    assert_eq!(
        p.drain(),
        vec![
            (Control::ZoomAbsolute, Value::Int(40)),
            (Control::Gain, Value::Int(5))
        ]
    );
    assert!(p.drain().is_empty());
}

#[test]
fn pending_drains_in_first_request_order() {
    let mut p = Pending::default();
    p.insert(Control::Gain, Value::Int(1));
    p.insert(Control::ZoomAbsolute, Value::Int(2));
    p.insert(Control::Gain, Value::Int(3));
    let order: Vec<Control> = p.drain().into_iter().map(|(c, _)| c).collect();
    assert_eq!(order, [Control::Gain, Control::ZoomAbsolute]);
}

/// A transport that blocks each write until the test releases it, so the
/// test can pile up requests behind a write in flight.
/// (wValue, data) of every completed write.
type WriteLog = Arc<Mutex<Vec<(u16, Vec<u8>)>>>;

struct GatedTransport {
    writes: WriteLog,
    /// Signalled as each write starts, before it blocks on the gate.
    started: Mutex<mpsc::Sender<()>>,
    gate: Mutex<mpsc::Receiver<()>>,
}

impl Transport for GatedTransport {
    fn control_in(&self, _req: &Request) -> Result<Vec<u8>, TransportError> {
        Err(TransportError::Stall)
    }

    fn control_out(&self, req: &Request, data: &[u8]) -> Result<(), TransportError> {
        // The test may have stopped listening; a lost signal is harmless.
        let _ = self.started.lock().expect("started lock").send(());
        self.gate
            .lock()
            .expect("gate lock")
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| TransportError::Timeout)?;
        self.writes
            .lock()
            .expect("writes lock")
            .push((req.value, data.to_vec()));
        Ok(())
    }
}

#[test]
fn the_queue_coalesces_writes_behind_one_in_flight() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let (open, gate) = mpsc::channel();
    let (started_tx, started) = mpsc::channel();
    let camera = Camera::new(
        parse_configuration(TINY2_LITE).unwrap(),
        GatedTransport {
            writes: writes.clone(),
            started: Mutex::new(started_tx),
            gate: Mutex::new(gate),
        },
    );
    let (done_tx, done) = mpsc::channel();
    let queue = ControlQueue::spawn(camera, move |event: QueueEvent| {
        if let QueueEvent::Wrote(outcome) = event {
            let _ = done_tx.send(outcome);
        }
    });

    queue.set(Control::ZoomAbsolute, Value::Int(1));
    // Wait until the first write is in flight, then queue three more while
    // it is blocked.
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    queue.set(Control::ZoomAbsolute, Value::Int(2));
    queue.set(Control::ZoomAbsolute, Value::Int(3));
    queue.set(Control::ZoomAbsolute, Value::Int(4));
    for _ in 0..2 {
        open.send(()).unwrap();
    }
    for _ in 0..2 {
        let outcome = done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(outcome.result.is_ok());
    }
    let zooms: Vec<u16> = writes
        .lock()
        .unwrap()
        .iter()
        .map(|(_, d)| u16::from_le_bytes([d[0], d[1]]))
        .collect();
    assert_eq!(zooms, [1, 4], "2 and 3 were superseded by 4");
}

#[test]
fn failed_writes_are_reported_with_their_control() {
    struct Broken;
    impl Transport for Broken {
        fn control_in(&self, _req: &Request) -> Result<Vec<u8>, TransportError> {
            Err(TransportError::Disconnected)
        }
        fn control_out(&self, _req: &Request, _data: &[u8]) -> Result<(), TransportError> {
            Err(TransportError::Disconnected)
        }
    }
    let camera = Camera::new(parse_configuration(TINY2_LITE).unwrap(), Broken);
    let (tx, rx) = mpsc::channel();
    let queue = ControlQueue::spawn(camera, move |event| {
        if let QueueEvent::Wrote(outcome) = event {
            let _ = tx.send(outcome);
        }
    });
    queue.set(Control::Gain, Value::Int(3));
    let outcome = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(outcome.control, Control::Gain);
    assert!(outcome.result.is_err());
}

#[test]
fn dropping_the_queue_stops_its_worker_and_releases_the_camera() {
    /// Holds a token for as long as the camera (and so the worker) lives.
    struct Tokened(#[expect(dead_code, reason = "held for its drop")] Arc<()>);
    impl Transport for Tokened {
        fn control_in(&self, _req: &Request) -> Result<Vec<u8>, TransportError> {
            Err(TransportError::Stall)
        }
        fn control_out(&self, _req: &Request, _data: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
    }
    let token = Arc::new(());
    let camera = Camera::new(
        parse_configuration(TINY2_LITE).unwrap(),
        Tokened(token.clone()),
    );
    let queue = ControlQueue::spawn(camera, |_| {});
    assert_eq!(Arc::strong_count(&token), 2, "the worker holds the camera");
    drop(queue);
    assert_eq!(
        Arc::strong_count(&token),
        1,
        "the worker exited and dropped it"
    );
}

/// Answers `GET_CUR` for zoom with 42, records the order of requests, and
/// holds each write until the test opens the gate.
struct GatedRecording {
    log: Arc<Mutex<Vec<&'static str>>>,
    started: Mutex<mpsc::Sender<()>>,
    gate: Mutex<mpsc::Receiver<()>>,
}

impl Transport for GatedRecording {
    fn control_in(&self, _req: &Request) -> Result<Vec<u8>, TransportError> {
        self.log.lock().expect("log lock").push("read");
        Ok(vec![42, 0])
    }
    fn control_out(&self, _req: &Request, _data: &[u8]) -> Result<(), TransportError> {
        // The test may have stopped listening; a lost signal is harmless.
        let _ = self.started.lock().expect("started lock").send(());
        self.gate
            .lock()
            .expect("gate lock")
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| TransportError::Timeout)?;
        self.log.lock().expect("log lock").push("write");
        Ok(())
    }
}

#[test]
fn reads_run_after_the_writes_pending_with_them() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (open, gate) = mpsc::channel();
    let (started_tx, started) = mpsc::channel();
    let camera = Camera::new(
        parse_configuration(TINY2_LITE).unwrap(),
        GatedRecording {
            log: log.clone(),
            started: Mutex::new(started_tx),
            gate: Mutex::new(gate),
        },
    );
    let (tx, rx) = mpsc::channel();
    let queue = ControlQueue::spawn(camera, move |event| {
        if let QueueEvent::Read(outcome) = event {
            let _ = tx.send(outcome);
        }
    });
    // Hold a first write in flight, then queue a read before a second write.
    queue.set(Control::ZoomAbsolute, Value::Int(7));
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    queue.read(Control::ZoomAbsolute);
    queue.set(Control::Gain, Value::Int(3));
    open.send(()).unwrap();
    open.send(()).unwrap();
    let outcome = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(outcome.control, Control::ZoomAbsolute);
    assert_eq!(outcome.result.unwrap(), Value::Int(42));
    assert_eq!(*log.lock().unwrap(), ["write", "write", "read"]);
}

#[test]
fn repeated_reads_of_one_control_collapse() {
    let mut p = Pending::default();
    p.request_read(Control::PanTiltAbsolute);
    p.request_read(Control::ZoomAbsolute);
    p.request_read(Control::PanTiltAbsolute);
    assert_eq!(
        p.drain_reads(),
        vec![Control::PanTiltAbsolute, Control::ZoomAbsolute]
    );
    assert!(p.drain_reads().is_empty());
}
