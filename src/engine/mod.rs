//! Control plane engine: queue watching, focus lifecycle, work retirement.

pub mod control;
pub mod focus;

pub use control::{ControlConfig, ControlPlane};
pub use focus::Focus;
