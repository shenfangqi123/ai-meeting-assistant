pub mod config;
pub mod manager;
pub mod speaker;
pub mod wasapi;
pub mod writer;

pub use manager::{CaptureManager, SegmentInfo};
