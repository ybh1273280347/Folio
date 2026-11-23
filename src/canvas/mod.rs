//! Native GPUI knowledge canvas supporting freeform spatial notes, mind-map hierarchies,
//! bidirectional document anchors, and transactional AI operations.

pub mod anchor;
pub mod history;
pub mod interaction;
pub mod model;
pub mod pane;

pub use pane::{CanvasEvent, CanvasPane, bind_shortcuts};
