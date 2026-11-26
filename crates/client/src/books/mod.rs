//! Books: a personal library and reading studio over a Files folder with EPUB, PDF, and comic readers.

pub mod catalog;
pub mod comic;
pub mod cover;
pub mod details;
pub mod epub;
pub mod highlights;
pub mod home;
pub mod model;
pub mod pdf;
pub mod pdf_text;
pub mod reader;
pub mod settings;
pub mod sidebar;
pub mod workspace;

pub use model::{
    BookDetail, BookItem, BookLibrary, BookLibraryFolder, BookManifest, BookMark, BookPosition,
    BookState, BookTocEntry, DEFAULT_BOOKS_FOLDER, SpineItem, book_format, book_format_label,
    is_comic,
};
pub use pdf::PdfView;
pub use workspace::BooksWorkspace;
