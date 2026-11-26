//! Native GPUI knowledge canvas supporting freeform spatial notes, mind-map hierarchies,
//! bidirectional document anchors, and transactional AI operations.

pub mod anchor;
pub mod export;
pub mod history;
pub mod index;
pub mod interaction;
pub mod layout;
pub mod model;
pub mod pane;
mod routing;

pub use anchor::KnowledgeAnchor;
pub use index::CanvasBacklink;
pub use model::TreeLayoutMode;
pub use pane::{CanvasEvent, CanvasPane, bind_shortcuts};
