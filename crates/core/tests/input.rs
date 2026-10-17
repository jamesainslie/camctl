use camctl_core::motion::{cover_offset, drag_stick};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn the_centre_of_the_view_is_the_centre_of_the_frame() {
    let (x, y) = cover_offset((560.0, 320.0), (640.0, 360.0), (280.0, 160.0)).unwrap();
    assert!(close(x, 0.0) && close(y, 0.0));
}

#[test]
fn cover_cropping_maps_edges_inside_the_frame() {
    // A 16:9 frame covering a wider-than-16:9 view is cropped top and
    // bottom, so the view's left edge is the frame's left edge but the
    // view's top edge is inside the frame.
    let view = (800.0, 300.0);
    let frame = (640.0, 360.0);
    let (x, _) = cover_offset(view, frame, (0.0, 150.0)).unwrap();
    assert!(close(x, -0.5));
    let (_, y) = cover_offset(view, frame, (400.0, 0.0)).unwrap();
    // Scale is 800/640 = 1.25, the frame is 450 high on screen, so the
    // view's top is 75 px into it: 75/450 - 0.5.
    assert!(close(y, 75.0 / 450.0 - 0.5), "y {y}");
}

#[test]
fn degenerate_sizes_give_nothing() {
    assert!(cover_offset((0.0, 300.0), (640.0, 360.0), (1.0, 1.0)).is_none());
    assert!(cover_offset((800.0, 300.0), (0.0, 360.0), (1.0, 1.0)).is_none());
}

#[test]
fn small_drags_are_gentle_and_large_ones_saturate() {
    let s = drag_stick(10.0, 0.0, 100.0);
    assert!(
        s.right > 0.0 && s.right < 0.05,
        "fine control near the centre"
    );
    let s = drag_stick(400.0, -400.0, 100.0);
    assert!(close(s.right, 1.0) && close(s.up, 1.0), "dragging up is up");
    let s = drag_stick(-50.0, 50.0, 100.0);
    assert!(s.right < 0.0 && s.up < 0.0);
    assert!(close(s.zoom_in, 0.0));
}

#[test]
fn a_dead_zone_ignores_jitter() {
    let s = drag_stick(2.0, -2.0, 100.0);
    assert!(close(s.right, 0.0) && close(s.up, 0.0));
}
