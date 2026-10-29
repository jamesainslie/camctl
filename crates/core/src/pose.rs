//! Where a camera points, kept across launches.
//!
//! Cameras forget: the OBSBOT Tiny 2 Lite, left alone, tips into its own
//! sleep, and wakes at a pose of its choosing. camctl remembers each
//! camera's last pose and sends it back when it opens the camera.

/// Pan and tilt in degrees, camera sign, and zoom in the camera's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pan: f64,
    pub tilt: f64,
    pub zoom: f64,
}

impl Pose {
    /// Whether every axis is a real number: only those are kept.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.pan.is_finite() && self.tilt.is_finite() && self.zoom.is_finite()
    }

    /// The text kept in user defaults: `pan -12.5 tilt 4 zoom 30`.
    #[must_use]
    pub fn encode(self) -> String {
        format!("pan {} tilt {} zoom {}", self.pan, self.tilt, self.zoom)
    }

    /// The pose [`Pose::encode`] wrote. `None` for anything else, or for a
    /// pose that is not finite.
    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let mut words = text.split_whitespace();
        let mut axis = |name: &str| {
            (words.next()? == name)
                .then(|| words.next()?.parse::<f64>().ok())
                .flatten()
        };
        let pose = Pose {
            pan: axis("pan")?,
            tilt: axis("tilt")?,
            zoom: axis("zoom")?,
        };
        (words.next().is_none() && pose.is_finite()).then_some(pose)
    }
}
