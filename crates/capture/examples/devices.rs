//! List cameras and their formats as AVFoundation reports them.

fn main() {
    println!("authorization: {:?}", camctl_capture::authorization());
    for d in camctl_capture::devices() {
        println!(
            "{} [{}] usb={:?} in_use_elsewhere={}",
            d.name,
            d.unique_id,
            camctl_capture::usb_identity(&d.unique_id),
            d.in_use_by_another_app()
        );
        for c in d.candidates() {
            println!(
                "    {:?} {}x{} up to {} fps",
                c.encoding, c.width, c.height, c.max_fps
            );
        }
    }
}
