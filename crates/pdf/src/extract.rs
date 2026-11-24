//! Text extraction: a hayro `Device` that records each shown glyph's Unicode text and its place
//! on the page, then groups glyphs into runs on one baseline.
//!
//! hayro calls `draw_glyph_run` for every text-showing operator, whatever the rendering mode
//! (invisible OCR text included), with the glyphs' text-space transforms and the current
//! transformation matrix. A glyph's Unicode comes from `Glyph::as_unicode` (the font's
//! `ToUnicode` CMap, then glyph names through the Adobe Glyph List); glyphs without one are
//! skipped. Advances come from the font's widths. Glyph boxes are not measured: a run's box is
//! `ASCENT` above and `DESCENT` below its baseline, in ems.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Affine, BezPath, Point, Rect, Vec2};

use crate::{TextChar, TextRun};

/// Share of the em above the baseline in a run's box.
pub(crate) const ASCENT: f32 = 0.8;
/// Share of the em below the baseline in a run's box.
pub(crate) const DESCENT: f32 = 0.2;

/// A gap wider than this (in ems) between two glyphs separates words.
const WORD_GAP: f64 = 0.18;
/// A gap wider than this (in ems) starts a new run (table columns, side notes).
const RUN_GAP: f64 = 2.5;

/// One shown glyph in page space.
struct Placed {
    text: String,
    origin: Point,
    /// Unit vector along the baseline.
    dir: Vec2,
    size: f64,
    advance: f64,
}

#[derive(Default)]
struct Collector {
    glyphs: Vec<Placed>,
    /// The previous call's glyph count and first transform, to skip the second pass of
    /// fill-and-stroke text, which hayro reports twice.
    last_call: Option<(usize, [f64; 6])>,
}

impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_clip_rect(&mut self, _: &Rect) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_rect(&mut self, _: &Rect, _: DrawProps<'a>, _: &DrawMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        let glyphs = run.glyphs();
        let Some(first) = glyphs.first() else { return };
        let signature = (
            glyphs.len(),
            (props.transform * first.transform()).as_coeffs(),
        );
        if self.last_call == Some(signature) {
            return;
        }
        self.last_call = Some(signature);

        for glyph in glyphs {
            let Some(text) = glyph.as_unicode().map(|u| match u {
                BfString::Char(c) => c.to_string(),
                BfString::String(s) => s,
            }) else {
                continue;
            };
            let text: String = text
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect();
            if text.is_empty() {
                continue;
            }
            // Glyph space has 1000 units per em.
            let m = props.transform * glyph.transform();
            let origin = m * Point::ORIGIN;
            let across = m * Point::new(1000.0, 0.0) - origin;
            let up = m * Point::new(0.0, 1000.0) - origin;
            let size = up.hypot();
            let width = across.hypot();
            if !size.is_finite() || !width.is_finite() || size < 1e-3 || width < 1e-6 {
                continue;
            }
            let dir = across / width;
            let units = match &**glyph {
                Glyph::Outline(g) => g.advance_width(),
                Glyph::Type3(g) => g.advance_width(),
            };
            let mut advance = units.map_or(-1.0, |u| f64::from(u) / 1000.0 * width);
            if !advance.is_finite() || advance <= 0.0 || advance > size * 3.0 {
                advance = if text.trim().is_empty() {
                    size * 0.25
                } else {
                    size * 0.5
                };
            }
            self.glyphs.push(Placed {
                text,
                origin,
                dir,
                size,
                advance,
            });
        }
    }
}

/// A run being built: its start, direction, size and characters.
struct Building {
    origin: Point,
    dir: Vec2,
    size: f64,
    chars: Vec<TextChar>,
    /// End of the last character along the baseline.
    end: f64,
}

impl Building {
    fn new(g: Placed) -> Self {
        let advance = g.advance;
        Self {
            origin: g.origin,
            dir: g.dir,
            size: g.size,
            chars: vec![TextChar {
                text: g.text,
                offset: 0.0,
                advance: advance as f32,
            }],
            end: advance,
        }
    }

    /// Where `g` lies relative to this run: its distance along and across the baseline.
    fn place(&self, g: &Placed) -> Option<(f64, f64)> {
        if self.dir.dot(g.dir) < 0.999 || (g.size - self.size).abs() > self.size * 0.15 {
            return None;
        }
        let delta = g.origin - self.origin;
        let along = delta.dot(self.dir);
        let across = delta.cross(self.dir).abs();
        Some((along, across))
    }

    /// Adds `g` when it continues this run's line; otherwise hands it back.
    fn push(&mut self, g: Placed) -> Result<(), Placed> {
        let Some((along, across)) = self.place(&g) else {
            return Err(g);
        };
        let gap = along - self.end;
        if across > self.size * 0.25 || gap < -self.size * 0.5 || gap > self.size * RUN_GAP {
            return Err(g);
        }
        let last_space = self
            .chars
            .last()
            .is_some_and(|c| c.text.ends_with(char::is_whitespace));
        if gap > self.size * WORD_GAP && !last_space && !g.text.starts_with(char::is_whitespace) {
            self.chars.push(TextChar {
                text: " ".into(),
                offset: self.end as f32,
                advance: gap as f32,
            });
        }
        let offset = along.max(0.0);
        self.end = self.end.max(offset + g.advance);
        self.chars.push(TextChar {
            text: g.text,
            offset: offset as f32,
            advance: g.advance as f32,
        });
        Ok(())
    }

    fn finish(self) -> TextRun {
        let size = self.size;
        let normal = Vec2::new(-self.dir.y, self.dir.x);
        // `normal` points below the baseline in page space (y grows downwards).
        let corners = [
            self.origin - normal * size * f64::from(ASCENT),
            self.origin + normal * size * f64::from(DESCENT),
            self.origin + self.dir * self.end - normal * size * f64::from(ASCENT),
            self.origin + self.dir * self.end + normal * size * f64::from(DESCENT),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for c in corners {
            x0 = x0.min(c.x);
            y0 = y0.min(c.y);
            x1 = x1.max(c.x);
            y1 = y1.max(c.y);
        }
        TextRun {
            text: self.chars.iter().map(|c| c.text.as_str()).collect(),
            x: x0 as f32,
            y: y0 as f32,
            width: (x1 - x0) as f32,
            height: (y1 - y0) as f32,
            font_size: size as f32,
            baseline_x: self.origin.x as f32,
            baseline_y: self.origin.y as f32,
            angle: self.dir.y.atan2(self.dir.x) as f32,
            length: self.end as f32,
            chars: self.chars,
            ends_line: true,
        }
    }
}

/// Groups glyphs, in the order they were shown, into runs; marks each run that ends a line.
fn group(glyphs: Vec<Placed>) -> Vec<TextRun> {
    let mut built: Vec<Building> = Vec::new();
    for g in glyphs {
        let g = match built.last_mut() {
            Some(run) => match run.push(g) {
                Ok(()) => continue,
                Err(g) => g,
            },
            None => g,
        };
        built.push(Building::new(g));
    }
    let same_line: Vec<bool> = built
        .windows(2)
        .map(|w| continues_line(&w[0], &w[1]))
        .chain([false])
        .collect();
    built
        .into_iter()
        .zip(same_line)
        .map(|(b, same_line)| {
            let mut run = b.finish();
            run.ends_line = !same_line;
            run
        })
        .collect()
}

/// The text of `page`, in page points with the origin at the top left of the drawn page.
pub(crate) fn page_text(page: &Page<'_>, settings: &InterpreterSettings) -> Vec<TextRun> {
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let transform: Affine = page.initial_transform(true).to_kurbo();
    let mut context = Context::new(
        transform,
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        settings.clone(),
    );
    let mut collector = Collector::default();
    interpret_page(page, &mut context, &mut collector);
    group(collector.glyphs)
}

/// Whether `next` starts further along `run`'s line (whatever its size), so no line break
/// separates them.
fn continues_line(run: &Building, next: &Building) -> bool {
    if run.dir.dot(next.dir) < 0.999 {
        return false;
    }
    let delta = next.origin - run.origin;
    let size = run.size.min(next.size);
    delta.cross(run.dir).abs() <= size * 0.5 && delta.dot(run.dir) >= run.end - size * 0.5
}
