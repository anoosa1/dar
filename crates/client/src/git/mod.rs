//! Git app: repository browser, commit graph and issues/pull requests.

pub mod branches;
pub mod code;
pub mod commits;
pub mod diff;
pub mod graph;
pub mod model;
pub mod overview;
pub mod panes;
pub mod parts;
pub mod repo;
pub mod settings;
pub mod sidebar;
pub mod tracker;
pub mod workspace;

pub use workspace::GitWorkspace;
