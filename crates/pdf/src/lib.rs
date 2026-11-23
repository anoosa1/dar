//! PDF documents for Dar, on hayro (pure Rust): page sizes, the outline, metadata and pages
//! rendered to RGBA bitmaps. The browser viewer runs it in a worker (`dar-pdf-worker`); the
//! runtime uses it for book metadata and covers. Nothing here touches the network or disk.
//!
//! CONTRACT (agreed API; implementations fill it in, signatures stay):

/// Why a document cannot be read or a page drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a PDF, or too damaged to read.
    Damaged,
    /// Encrypted with a password (hayro does not decrypt).
    Encrypted,
    /// Readable, but this page or feature cannot be drawn.
    Unsupported(String),
    /// The page would exceed [`MAX_PIXELS`].
    TooLarge,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Damaged => f.write_str("This PDF is damaged or not a PDF."),
            Error::Encrypted => f.write_str("This PDF is password-protected."),
            Error::Unsupported(what) => write!(f, "This PDF uses something Dar cannot show yet ({what})."),
            Error::TooLarge => f.write_str("This page is too large to draw at this zoom."),
        }
    }
}

impl std::error::Error for Error {}

/// The largest bitmap [`Document::render`] produces (width × height).
pub const MAX_PIXELS: u64 = 40_000_000;

/// A page's size in PDF points (1/72 inch), after its `/Rotate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// One outline (bookmarks) entry, flattened in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    pub title: String,
    /// Zero-based page the entry points to, when its destination resolves.
    pub page: Option<usize>,
    /// Nesting depth, 0 for top-level entries.
    pub depth: usize,
}

/// The document information dictionary's title and author (empty when absent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub title: String,
    pub author: String,
}

/// A drawn page: straight (not premultiplied) RGBA8, row-major, `width * height * 4` bytes,
/// on an opaque white background (ready for `ImageData`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// An open PDF. Cheap to keep open; pages are interpreted when drawn.
pub struct Document {
    _private: (),
}

impl Document {
    /// Parses a PDF from its bytes.
    pub fn open(_bytes: Vec<u8>) -> Result<Self, Error> {
        Err(Error::Unsupported("not implemented".into()))
    }

    pub fn page_count(&self) -> usize {
        0
    }

    /// Every page's size, in page order.
    pub fn page_sizes(&self) -> Vec<PageSize> {
        Vec::new()
    }

    /// The outline (bookmarks), flattened depth-first with named and explicit destinations
    /// resolved to pages; empty when the PDF has none.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        Vec::new()
    }

    pub fn metadata(&self) -> Metadata {
        Metadata::default()
    }

    /// Draws page `page` (zero-based) at `scale` device pixels per PDF point.
    pub fn render(&self, _page: usize, _scale: f32) -> Result<Bitmap, Error> {
        Err(Error::Unsupported("not implemented".into()))
    }
}
