//! Dar's Books runtime (`next/runtime/books/`): ZIP/ZIP64 archive reading, EPUB/PDF/comic
//! metadata & cover extraction, caching under `book-cache/`, and the `/api/v1/books` HTTP surface.

mod formats;
mod http;
mod pdf;
mod runtime;
mod zip;

pub use http::handle_http;
pub use pdf::command;
pub use runtime::BooksRuntime;
