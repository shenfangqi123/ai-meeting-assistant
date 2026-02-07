pub mod config;
pub mod manager;
pub mod speaker;
pub mod writer;
pub mod wasapi;

pub use manager::{CaptureManager, SegmentInfo};
