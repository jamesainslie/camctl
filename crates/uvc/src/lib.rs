//! USB Video Class (UVC) descriptor parsing and camera control.
//!
//! Transport-agnostic: nothing here does I/O. Parse a configuration
//! descriptor into a [`Topology`], then drive controls through a
//! [`Transport`] implementation with [`Camera`].

mod camera;
mod control;
mod descriptor;
mod transport;
mod value;

pub use camera::{Camera, CameraError, ControlInfo, ControlRange};
pub use control::{Control, ControlAddress, ControlSpec, Shape, UnitKind, Width};
pub use descriptor::{
    CameraTerminal, ControlBitmap, ExtensionUnit, Format, FormatKind, Frame, FrameIntervals,
    ParseError, ProcessingUnit, Topology, parse_configuration,
};
pub use transport::{Request, RequestCode, Transport, TransportError};
pub use value::{ARC_SECONDS_PER_DEGREE, Value, ValueError};
