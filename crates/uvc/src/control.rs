//! The standard UVC 1.1 camera terminal and processing unit controls.
//!
//! The analog-video processing unit controls (selectors 0x11, 0x12) and the
//! UVC 1.5 additions are left out on purpose: no webcam uses them.

/// Which kind of UVC unit a control belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitKind {
    CameraTerminal,
    ProcessingUnit,
}

/// Width of a scalar control on the wire. UVC only uses these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Width {
    One,
    Two,
    Four,
}

impl Width {
    /// Number of bytes.
    #[must_use]
    pub const fn bytes(self) -> u16 {
        match self {
            Width::One => 1,
            Width::Two => 2,
            Width::Four => 4,
        }
    }
}

/// How a control's wire bytes are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One little-endian integer.
    Scalar { width: Width, signed: bool },
    /// On/off, one byte.
    Bool,
    /// One byte holding one of a fixed set of values.
    Enum,
    /// One byte bitmap (auto-exposure mode).
    Bitmap,
    /// Two signed 32-bit values, pan then tilt, in arc-seconds.
    PanTilt,
    /// Pan direction, pan speed, tilt direction, tilt speed (four bytes).
    PanTiltRelative,
    /// Zoom direction, digital zoom flag, speed (three bytes).
    ZoomRelative,
    /// Direction then speed, two bytes (focus and roll relative).
    DirectionSpeed,
    /// White balance blue then red, two unsigned 16-bit values.
    WhiteBalanceComponent,
}

impl Shape {
    /// Number of bytes on the wire.
    #[must_use]
    pub const fn size(self) -> u16 {
        match self {
            Shape::Scalar { width, .. } => width.bytes(),
            Shape::Bool | Shape::Enum | Shape::Bitmap => 1,
            Shape::PanTilt => 8,
            Shape::PanTiltRelative | Shape::WhiteBalanceComponent => 4,
            Shape::ZoomRelative => 3,
            Shape::DirectionSpeed => 2,
        }
    }
}

/// A standard UVC control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Control {
    // Camera terminal.
    ScanningMode,
    AeMode,
    AePriority,
    ExposureTimeAbsolute,
    ExposureTimeRelative,
    FocusAbsolute,
    FocusRelative,
    FocusAuto,
    IrisAbsolute,
    IrisRelative,
    ZoomAbsolute,
    ZoomRelative,
    PanTiltAbsolute,
    PanTiltRelative,
    RollAbsolute,
    RollRelative,
    Privacy,
    // Processing unit.
    BacklightCompensation,
    Brightness,
    Contrast,
    Gain,
    PowerLineFrequency,
    Hue,
    Saturation,
    Sharpness,
    Gamma,
    WhiteBalanceTemperature,
    WhiteBalanceTemperatureAuto,
    WhiteBalanceComponent,
    WhiteBalanceComponentAuto,
    DigitalMultiplier,
    DigitalMultiplierLimit,
    HueAuto,
}

/// Static facts about a control from the UVC 1.1 specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlSpec {
    pub unit: UnitKind,
    /// Control selector (high byte of wValue).
    pub selector: u8,
    /// Bit in the unit descriptor's bmControls that advertises it.
    pub bit: u8,
    pub shape: Shape,
    pub name: &'static str,
}

const fn ct(selector: u8, bit: u8, shape: Shape, name: &'static str) -> ControlSpec {
    ControlSpec {
        unit: UnitKind::CameraTerminal,
        selector,
        bit,
        shape,
        name,
    }
}

const fn pu(selector: u8, bit: u8, shape: Shape, name: &'static str) -> ControlSpec {
    ControlSpec {
        unit: UnitKind::ProcessingUnit,
        selector,
        bit,
        shape,
        name,
    }
}

const U16: Shape = Shape::Scalar {
    width: Width::Two,
    signed: false,
};
const I16: Shape = Shape::Scalar {
    width: Width::Two,
    signed: true,
};
const U32: Shape = Shape::Scalar {
    width: Width::Four,
    signed: false,
};
const I8: Shape = Shape::Scalar {
    width: Width::One,
    signed: true,
};

impl Control {
    /// Every control, camera terminal first.
    pub const ALL: [Control; 33] = [
        Control::ScanningMode,
        Control::AeMode,
        Control::AePriority,
        Control::ExposureTimeAbsolute,
        Control::ExposureTimeRelative,
        Control::FocusAbsolute,
        Control::FocusRelative,
        Control::FocusAuto,
        Control::IrisAbsolute,
        Control::IrisRelative,
        Control::ZoomAbsolute,
        Control::ZoomRelative,
        Control::PanTiltAbsolute,
        Control::PanTiltRelative,
        Control::RollAbsolute,
        Control::RollRelative,
        Control::Privacy,
        Control::BacklightCompensation,
        Control::Brightness,
        Control::Contrast,
        Control::Gain,
        Control::PowerLineFrequency,
        Control::Hue,
        Control::Saturation,
        Control::Sharpness,
        Control::Gamma,
        Control::WhiteBalanceTemperature,
        Control::WhiteBalanceTemperatureAuto,
        Control::WhiteBalanceComponent,
        Control::WhiteBalanceComponentAuto,
        Control::DigitalMultiplier,
        Control::DigitalMultiplierLimit,
        Control::HueAuto,
    ];

    /// Selector, capability bit, wire shape and display name.
    #[must_use]
    pub const fn spec(self) -> ControlSpec {
        match self {
            Control::ScanningMode => ct(0x01, 0, Shape::Enum, "Scanning mode"),
            Control::AeMode => ct(0x02, 1, Shape::Bitmap, "Auto exposure mode"),
            Control::AePriority => ct(0x03, 2, Shape::Enum, "Auto exposure priority"),
            Control::ExposureTimeAbsolute => ct(0x04, 3, U32, "Exposure time"),
            Control::ExposureTimeRelative => ct(0x05, 4, I8, "Exposure time step"),
            Control::FocusAbsolute => ct(0x06, 5, U16, "Focus"),
            Control::FocusRelative => ct(0x07, 6, Shape::DirectionSpeed, "Focus move"),
            Control::FocusAuto => ct(0x08, 17, Shape::Bool, "Auto focus"),
            Control::IrisAbsolute => ct(0x09, 7, U16, "Iris"),
            Control::IrisRelative => ct(0x0a, 8, I8, "Iris step"),
            Control::ZoomAbsolute => ct(0x0b, 9, U16, "Zoom"),
            Control::ZoomRelative => ct(0x0c, 10, Shape::ZoomRelative, "Zoom move"),
            Control::PanTiltAbsolute => ct(0x0d, 11, Shape::PanTilt, "Pan and tilt"),
            Control::PanTiltRelative => ct(0x0e, 12, Shape::PanTiltRelative, "Pan and tilt move"),
            Control::RollAbsolute => ct(0x0f, 13, I16, "Roll"),
            Control::RollRelative => ct(0x10, 14, Shape::DirectionSpeed, "Roll move"),
            Control::Privacy => ct(0x11, 18, Shape::Bool, "Privacy shutter"),
            Control::BacklightCompensation => pu(0x01, 8, U16, "Backlight compensation"),
            Control::Brightness => pu(0x02, 0, I16, "Brightness"),
            Control::Contrast => pu(0x03, 1, U16, "Contrast"),
            Control::Gain => pu(0x04, 9, U16, "Gain"),
            Control::PowerLineFrequency => pu(0x05, 10, Shape::Enum, "Anti-flicker"),
            Control::Hue => pu(0x06, 2, I16, "Hue"),
            Control::Saturation => pu(0x07, 3, U16, "Saturation"),
            Control::Sharpness => pu(0x08, 4, U16, "Sharpness"),
            Control::Gamma => pu(0x09, 5, U16, "Gamma"),
            Control::WhiteBalanceTemperature => pu(0x0a, 6, U16, "White balance"),
            Control::WhiteBalanceTemperatureAuto => pu(0x0b, 12, Shape::Bool, "Auto white balance"),
            Control::WhiteBalanceComponent => pu(
                0x0c,
                7,
                Shape::WhiteBalanceComponent,
                "White balance (blue, red)",
            ),
            Control::WhiteBalanceComponentAuto => {
                pu(0x0d, 13, Shape::Bool, "Auto white balance (component)")
            }
            Control::DigitalMultiplier => pu(0x0e, 14, U16, "Digital zoom"),
            Control::DigitalMultiplierLimit => pu(0x0f, 15, U16, "Digital zoom limit"),
            Control::HueAuto => pu(0x10, 11, Shape::Bool, "Auto hue"),
        }
    }
}

/// Where to send requests for one control on one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlAddress {
    pub unit_id: u8,
    pub interface: u8,
    pub selector: u8,
}

impl Control {
    /// Stable kebab-case name, used by the CLI and in saved profiles.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Control::ScanningMode => "scanning-mode",
            Control::AeMode => "exposure-mode",
            Control::AePriority => "exposure-priority",
            Control::ExposureTimeAbsolute => "exposure",
            Control::ExposureTimeRelative => "exposure-step",
            Control::FocusAbsolute => "focus",
            Control::FocusRelative => "focus-move",
            Control::FocusAuto => "focus-auto",
            Control::IrisAbsolute => "iris",
            Control::IrisRelative => "iris-step",
            Control::ZoomAbsolute => "zoom",
            Control::ZoomRelative => "zoom-move",
            Control::PanTiltAbsolute => "pan-tilt",
            Control::PanTiltRelative => "pan-tilt-move",
            Control::RollAbsolute => "roll",
            Control::RollRelative => "roll-move",
            Control::Privacy => "privacy",
            Control::BacklightCompensation => "backlight",
            Control::Brightness => "brightness",
            Control::Contrast => "contrast",
            Control::Gain => "gain",
            Control::PowerLineFrequency => "anti-flicker",
            Control::Hue => "hue",
            Control::Saturation => "saturation",
            Control::Sharpness => "sharpness",
            Control::Gamma => "gamma",
            Control::WhiteBalanceTemperature => "white-balance",
            Control::WhiteBalanceTemperatureAuto => "white-balance-auto",
            Control::WhiteBalanceComponent => "white-balance-component",
            Control::WhiteBalanceComponentAuto => "white-balance-component-auto",
            Control::DigitalMultiplier => "digital-zoom",
            Control::DigitalMultiplierLimit => "digital-zoom-limit",
            Control::HueAuto => "hue-auto",
        }
    }

    /// The control named `key` (see [`Control::key`]).
    #[must_use]
    pub fn from_key(key: &str) -> Option<Control> {
        Control::ALL.into_iter().find(|c| c.key() == key)
    }
}

impl std::fmt::Display for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.spec().name)
    }
}
