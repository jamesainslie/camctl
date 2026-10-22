use std::time::Duration;

use camctl_core::health::{ControlHealth, Verdict, reconnect_delay};
use camctl_uvc::{CameraError, Control, RequestCode, TransportError, ValueError};

fn failed(source: TransportError) -> Result<(), CameraError> {
    Err(CameraError::Request {
        control: Control::ZoomAbsolute,
        code: RequestCode::SetCur,
        source,
    })
}

#[test]
fn successes_and_stalls_keep_the_link() {
    let mut h = ControlHealth::default();
    assert_eq!(h.record(&Ok(())), Verdict::Fine);
    // A stall is the camera answering "no": it is still there.
    for _ in 0..10 {
        assert_eq!(h.record(&failed(TransportError::Stall)), Verdict::Fine);
    }
}

#[test]
fn a_disconnect_reconnects_at_once() {
    let mut h = ControlHealth::default();
    assert_eq!(
        h.record(&failed(TransportError::Disconnected)),
        Verdict::Reconnect
    );
}

#[test]
fn repeated_timeouts_or_io_errors_reconnect() {
    let mut h = ControlHealth::default();
    assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
    assert_eq!(
        h.record(&failed(TransportError::Other("io".into()))),
        Verdict::Fine
    );
    assert_eq!(
        h.record(&failed(TransportError::Timeout)),
        Verdict::Reconnect
    );
    // Counting starts over after a reconnect verdict.
    assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
}

#[test]
fn a_success_or_stall_between_failures_starts_the_count_over() {
    let mut h = ControlHealth::default();
    for _ in 0..5 {
        assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
        assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
        assert_eq!(h.record(&Ok(())), Verdict::Fine);
    }
    assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
    assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
    assert_eq!(h.record(&failed(TransportError::Stall)), Verdict::Fine);
    assert_eq!(h.record(&failed(TransportError::Timeout)), Verdict::Fine);
}

#[test]
fn errors_found_before_any_io_say_nothing_about_the_link() {
    let mut h = ControlHealth::default();
    for _ in 0..5 {
        let unsupported: Result<(), CameraError> =
            Err(CameraError::Unsupported(Control::ZoomAbsolute));
        assert_eq!(h.record(&unsupported), Verdict::Fine);
        let bad_value: Result<(), CameraError> = Err(CameraError::Value(ValueError::Short {
            expected: 2,
            got: 0,
        }));
        assert_eq!(h.record(&bad_value), Verdict::Fine);
    }
}

#[test]
fn reconnect_delays_back_off_and_level_out() {
    let delays: Vec<Duration> = (0..8).map(reconnect_delay).collect();
    assert_eq!(delays[0], Duration::from_millis(500));
    assert!(delays.windows(2).all(|w| w[0] <= w[1]), "{delays:?}");
    assert_eq!(delays[7], Duration::from_secs(5));
    assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(5));
}
