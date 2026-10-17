//! Which camera format to stream for a preview of a given size.
//!
//! Decoding is the expensive part of a preview: MJPEG frames go through
//! VideoToolbox, uncompressed frames need none. So uncompressed wins
//! whenever stretching it to the window still looks fine, and otherwise the
//! smallest MJPEG frame that covers the window does. 4K is never chosen for
//! a preview unless the cap allows it.

/// How a format's frames arrive from the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Raw YUV (YUY2, NV12): no decoding.
    Uncompressed,
    /// Motion JPEG: decoded per frame.
    Mjpeg,
    /// Anything else (H.264, depth, ...): not used for previews.
    Other,
}

/// One format the camera offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub encoding: Encoding,
    pub width: u32,
    pub height: u32,
    pub max_fps: f64,
}

/// The preview's size in device pixels, and the widest frame to consider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    pub width: u32,
    pub height: u32,
    pub max_width: u32,
}

/// Frames slower than this look choppy in a live preview.
const MIN_FPS: f64 = 24.0;
/// How far an uncompressed frame may be stretched: free to decode, so a
/// little softness is a good trade.
const UNCOMPRESSED_STRETCH: f64 = 1.5;
/// How far a compressed frame may be stretched.
const COMPRESSED_STRETCH: f64 = 1.1;

fn stretch(encoding: Encoding) -> f64 {
    match encoding {
        Encoding::Uncompressed => UNCOMPRESSED_STRETCH,
        _ => COMPRESSED_STRETCH,
    }
}

fn encoding_rank(encoding: Encoding) -> u8 {
    match encoding {
        Encoding::Uncompressed => 0,
        Encoding::Mjpeg => 1,
        Encoding::Other => 2,
    }
}

/// Difference between the candidate's and the preview's aspect ratios.
fn aspect_error(c: &Candidate, want: Want) -> f64 {
    if c.height == 0 || want.height == 0 {
        return f64::MAX;
    }
    (f64::from(c.width) / f64::from(c.height) - f64::from(want.width) / f64::from(want.height))
        .abs()
}

/// Index of the format to stream, or `None` when nothing is usable.
#[must_use]
pub fn choose(candidates: &[Candidate], want: Want) -> Option<usize> {
    let eligible = candidates.iter().enumerate().filter(|(_, c)| {
        c.encoding != Encoding::Other && c.max_fps >= MIN_FPS && c.width <= want.max_width
    });
    let covers = |c: &Candidate| f64::from(c.width) * stretch(c.encoding) >= f64::from(want.width);
    let cheapest_cover = eligible
        .clone()
        .filter(|(_, c)| covers(c))
        .min_by(|(_, a), (_, b)| {
            encoding_rank(a.encoding)
                .cmp(&encoding_rank(b.encoding))
                .then(a.width.cmp(&b.width))
                .then(aspect_error(a, want).total_cmp(&aspect_error(b, want)))
        });
    if let Some((i, _)) = cheapest_cover {
        return Some(i);
    }
    // Nothing covers the window: the biggest frame there is, preferring
    // uncompressed, then the closest aspect ratio.
    eligible
        .max_by(|(_, a), (_, b)| {
            a.width
                .cmp(&b.width)
                .then(encoding_rank(b.encoding).cmp(&encoding_rank(a.encoding)))
                .then(aspect_error(b, want).total_cmp(&aspect_error(a, want)))
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The OBSBOT Tiny 2 Lite's formats as AVFoundation lists them.
    fn tiny2_lite() -> Vec<Candidate> {
        let c = |encoding, width, height, max_fps| Candidate {
            encoding,
            width,
            height,
            max_fps,
        };
        vec![
            c(Encoding::Mjpeg, 1920, 1080, 60.0),
            c(Encoding::Mjpeg, 3840, 2160, 30.0),
            c(Encoding::Mjpeg, 1280, 720, 60.0),
            c(Encoding::Uncompressed, 640, 360, 30.0),
            c(Encoding::Uncompressed, 640, 480, 30.0),
        ]
    }

    fn want(width: u32, height: u32) -> Want {
        Want {
            width,
            height,
            max_width: 1920,
        }
    }

    fn pick(width: u32, height: u32) -> (u32, u32, Encoding) {
        let all = tiny2_lite();
        let c = all[choose(&all, want(width, height)).unwrap()];
        (c.width, c.height, c.encoding)
    }

    #[test]
    fn a_small_retina_window_streams_uncompressed_360p() {
        // 480x270 points at 2x.
        assert_eq!(pick(960, 540), (640, 360, Encoding::Uncompressed));
    }

    #[test]
    fn a_4_by_3_window_gets_the_4_by_3_raw_format() {
        assert_eq!(pick(800, 600), (640, 480, Encoding::Uncompressed));
    }

    #[test]
    fn a_medium_window_streams_mjpeg_720p() {
        assert_eq!(pick(1280, 720), (1280, 720, Encoding::Mjpeg));
    }

    #[test]
    fn a_large_window_streams_1080p_but_never_4k() {
        assert_eq!(pick(2000, 1125), (1920, 1080, Encoding::Mjpeg));
        assert_eq!(pick(5000, 2800), (1920, 1080, Encoding::Mjpeg));
    }

    #[test]
    fn the_cap_can_allow_4k() {
        let all = tiny2_lite();
        let i = choose(
            &all,
            Want {
                width: 3000,
                height: 1700,
                max_width: 3840,
            },
        )
        .unwrap();
        assert_eq!((all[i].width, all[i].height), (3840, 2160));
    }

    #[test]
    fn slow_and_unusable_formats_are_skipped() {
        let all = vec![
            Candidate {
                encoding: Encoding::Uncompressed,
                width: 1920,
                height: 1080,
                max_fps: 5.0,
            },
            Candidate {
                encoding: Encoding::Other,
                width: 1280,
                height: 720,
                max_fps: 30.0,
            },
        ];
        assert_eq!(choose(&all, want(960, 540)), None);
        assert_eq!(choose(&[], want(960, 540)), None);
    }
}
