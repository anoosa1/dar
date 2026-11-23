//! PDF documents for Dar, on hayro (pure Rust): page sizes, the outline, metadata and pages
//! rendered to RGBA bitmaps. The browser viewer runs it in a worker (`dar-pdf-worker`); the
//! runtime uses it for book metadata and covers. Nothing here touches the network or disk.
//!
//! CONTRACT (agreed API; implementations fill it in, signatures stay):

mod outline;
mod text;

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};

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
    // SAFETY: `cache` stores font/glyph references borrowed from `pdf`. It MUST be declared
    // before `pdf` so Rust drops `cache` before `pdf`, and `pdf` is held in an `Arc` so moving
    // `Document` never moves or invalidates the pointed-to `Pdf`.
    cache: RefCell<RenderCache<'static>>,
    last_scale_bits: Cell<u32>,
    interpreter_settings: InterpreterSettings,
    render_settings: RenderSettings,
    pdf: Arc<Pdf>,
}

impl Document {
    /// Parses a PDF from its bytes.
    pub fn open(bytes: Vec<u8>) -> Result<Self, Error> {
        let parsed = catch_unwind(AssertUnwindSafe(|| Pdf::new(bytes)))
            .map_err(|_| Error::Damaged)?;

        let pdf = match parsed {
            Ok(pdf) => pdf,
            Err(LoadPdfError::Decryption(_)) => return Err(Error::Encrypted),
            Err(LoadPdfError::Invalid) => return Err(Error::Damaged),
        };

        Ok(Self {
            cache: RefCell::new(RenderCache::new()),
            last_scale_bits: Cell::new(0),
            interpreter_settings: InterpreterSettings::default(),
            render_settings: RenderSettings::default(),
            pdf: Arc::new(pdf),
        })
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Every page's size, in page order.
    pub fn page_sizes(&self) -> Vec<PageSize> {
        self.pdf
            .pages()
            .iter()
            .map(|page| {
                let (width, height) = page.render_dimensions();
                PageSize { width, height }
            })
            .collect()
    }

    /// The outline (bookmarks), flattened depth-first with named and explicit destinations
    /// resolved to pages; empty when the PDF has none.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        catch_unwind(AssertUnwindSafe(|| outline::extract_outline(&self.pdf))).unwrap_or_default()
    }

    pub fn metadata(&self) -> Metadata {
        let meta = self.pdf.metadata();
        Metadata {
            title: meta
                .title
                .as_deref()
                .map(text::decode_pdf_text_string)
                .unwrap_or_default(),
            author: meta
                .author
                .as_deref()
                .map(text::decode_pdf_text_string)
                .unwrap_or_default(),
        }
    }

    /// Draws page `page` (zero-based) at `scale` device pixels per PDF point.
    pub fn render(&self, page: usize, scale: f32) -> Result<Bitmap, Error> {
        if scale.is_infinite() && scale.is_sign_positive() {
            return Err(Error::TooLarge);
        }
        if !scale.is_finite() || scale <= 0.0 {
            return Err(Error::Unsupported("invalid render scale".into()));
        }

        let pdf_page = self
            .pdf
            .pages()
            .get(page)
            .ok_or_else(|| Error::Unsupported(format!("page {page} out of range")))?;

        let (pt_w, pt_h) = pdf_page.render_dimensions();
        if !pt_w.is_finite() || !pt_h.is_finite() || pt_w <= 0.0 || pt_h <= 0.0 {
            return Err(Error::Damaged);
        }

        let (width_u16, height_u16) = scaled_dimensions(pt_w, pt_h, scale)?;

        // Reset the glyph/outline cache when the zoom level changes so cached glyph bitmaps
        // from earlier scales do not accumulate unboundedly in a long-lived worker.
        let scale_bits = scale.to_bits();
        if self.last_scale_bits.replace(scale_bits) != scale_bits {
            *self.cache.borrow_mut() = RenderCache::new();
        }

        // SAFETY: `pdf_page` borrows from `self.pdf` (behind `Arc<Pdf>`), which is never
        // moved or mutated while `self` lives, and `self.cache` is dropped before `self.pdf`.
        let static_page: &'static Page<'static> =
            unsafe { std::mem::transmute::<&Page<'_>, &'static Page<'static>>(pdf_page) };

        let transform =
            Affine::scale(f64::from(scale)) * static_page.initial_transform(true).to_kurbo();

        let rendered = catch_unwind(AssertUnwindSafe(|| {
            let cache = self.cache.borrow();
            let mut ctx = RenderContext::new(width_u16, height_u16);
            render_into(
                static_page,
                &cache,
                &self.interpreter_settings,
                &self.render_settings,
                &mut ctx,
                transform,
            );
            ctx.flush();

            let mut pixmap = Pixmap::new(width_u16, height_u16);
            ctx.render_with(
                &mut pixmap,
                &mut Resources::default(),
                RasterizerSettings {
                    target_init: TargetInit::Clear(WHITE),
                    ..Default::default()
                },
            );

            pixmap.take_rgba8(ImageAlphaType::Alpha)
        }));

        match rendered {
            Ok(rgba) => Ok(Bitmap {
                width: u32::from(width_u16),
                height: u32::from(height_u16),
                rgba,
            }),
            Err(_) => {
                // Discard cache entries that may have been left in a partial state before unwinding.
                *self.cache.borrow_mut() = RenderCache::new();
                Err(Error::Unsupported("renderer failed on this page".into()))
            }
        }
    }
}

fn scaled_dimensions(pt_w: f32, pt_h: f32, scale: f32) -> Result<(u16, u16), Error> {
    let raw_w = (f64::from(pt_w) * f64::from(scale)).round();
    let raw_h = (f64::from(pt_h) * f64::from(scale)).round();

    if !raw_w.is_finite()
        || !raw_h.is_finite()
        || raw_w > f64::from(u16::MAX)
        || raw_h > f64::from(u16::MAX)
    {
        return Err(Error::TooLarge);
    }

    let width = (raw_w as u32).max(1);
    let height = (raw_h as u32).max(1);

    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > MAX_PIXELS {
        return Err(Error::TooLarge);
    }

    let width_u16 = u16::try_from(width).map_err(|_| Error::TooLarge)?;
    let height_u16 = u16::try_from(height).map_err(|_| Error::TooLarge)?;
    Ok((width_u16, height_u16))
}
