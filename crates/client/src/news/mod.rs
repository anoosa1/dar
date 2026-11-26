//! News: feeds and folders, chronological day-grouped list and Magazine cards,
//! sanitized editorial reader with typography controls and podcast transport,
//! feed cadence inspector, subscription discovery, and News settings.

pub mod list;
pub mod model;
pub mod panes;
pub mod reader;
pub mod render;
pub mod settings;
pub mod sidebar;
pub mod workspace;

pub use workspace::NewsWorkspace;
