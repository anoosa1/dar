//! News (RSS/Atom/JSON Feed) runtime, native HTTP API, and Nextcloud News API (`next/runtime/news/`).

pub mod feed;
pub mod html;
pub mod http;
pub mod nextcloud;
pub mod opml;
pub mod runtime;

pub use http::handle_http;
pub use nextcloud::{NEXTCLOUD_BASE, handle_nextcloud};
pub use runtime::NewsRuntime;
