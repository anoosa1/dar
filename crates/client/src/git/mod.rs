//! Git app (`git/workspace.tsx`, `git/tracker.tsx`, `git/parts.tsx`, `git/graph.ts`, `diff.tsx`).

pub mod diff;
pub mod graph;
pub mod parts;
pub mod tracker;
pub mod workspace;

pub use workspace::GitWorkspace;
