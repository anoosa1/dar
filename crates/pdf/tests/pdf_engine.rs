use std::fmt::Write as _;

use dar_pdf::{Document, Error, MAX_PIXELS, Metadata, OutlineEntry, PageSize};

/// Minimal PDF builder that assembles numbered indirect objects with exact xref byte offsets.
struct PdfBuilder {
    objects: Vec<(u32, Vec<u8>)>,
    root: u32,
    info: Option<u32>,
    encrypt: Option<String>,
    id: Option<String>,
}

impl PdfBuilder {
    fn new(root: u32) -> Self {
        Self {
            objects: Vec::new(),
            root,
            info: None,
            encrypt: None,
            id: None,
        }
    }

    fn obj(&mut self, id: u32, body: impl Into<Vec<u8>>) -> &mut Self {
        self.objects.push((id, body.into()));
        self
    }

    fn info(&mut self, id: u32) -> &mut Self {
        self.info = Some(id);
        self
    }

    fn encrypt(&mut self, entry: impl Into<String>) -> &mut Self {
        self.encrypt = Some(entry.into());
        self
    }

    fn file_id(&mut self, id_hex: &str) -> &mut Self {
        self.id = Some(format!("[<{id_hex}><{id_hex}>]"));
        self
    }

    fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");

        let max_id = self.objects.iter().map(|(id, _)| *id).max().unwrap_or(0);
        let mut offsets: Vec<Option<usize>> = vec![None; (max_id + 1) as usize];

        for (id, body) in &self.objects {
            offsets[*id as usize] = Some(out.len());
            out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            if !body.ends_with(b"\n") {
                out.push(b'\n');
            }
            out.extend_from_slice(b"endobj\n");
        }

        let xref_offset = out.len();
        let size = max_id + 1;
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for off in &offsets[1..] {
            match off {
                Some(pos) => out.extend_from_slice(format!("{pos:010} 00000 n \n").as_bytes()),
                None => out.extend_from_slice(b"0000000000 65535 f \n"),
            }
        }

        let mut trailer = format!("trailer\n<< /Size {size} /Root {} 0 R", self.root);
        if let Some(info) = self.info {
            let _ = write!(trailer, " /Info {info} 0 R");
        }
        if let Some(enc) = &self.encrypt {
            let _ = write!(trailer, " /Encrypt {enc}");
        }
        if let Some(id) = &self.id {
            let _ = write!(trailer, " /ID {id}");
        }
        trailer.push_str(" >>\n");
        let _ = write!(trailer, "startxref\n{xref_offset}\n%%EOF\n");
        out.extend_from_slice(trailer.as_bytes());
        out
    }
}

fn stream_obj(content: &[u8]) -> Vec<u8> {
    let mut v = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    v.extend_from_slice(content);
    v.extend_from_slice(b"\nendstream");
    v
}

#[test]
fn page_count_and_sizes_including_rotate_and_crop_box() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 >>")
        // Standard US Letter portrait
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>")
        // Rotated 90 degrees -> width and height swap
        .obj(4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 400] /Rotate 90 >>")
        // CropBox smaller than MediaBox -> uses CropBox
        .obj(5, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [50 100 350 500] >>")
        // CropBox + Rotate 270 -> cropped dimensions swapped
        .obj(6, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [0 0 300 500] /Rotate 270 >>");

    let doc = Document::open(b.build()).expect("valid pdf");
    assert_eq!(doc.page_count(), 4);
    assert_eq!(
        doc.page_sizes(),
        vec![
            PageSize { width: 612.0, height: 792.0 },
            PageSize { width: 400.0, height: 200.0 },
            PageSize { width: 300.0, height: 400.0 },
            PageSize { width: 500.0, height: 300.0 },
        ]
    );
}

#[test]
fn outline_nested_explicit_and_named_destinations() {
    let mut b = PdfBuilder::new(1);
    b.obj(
        1,
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /Dests 20 0 R /Names << /Dests 21 0 R >> >>",
    )
    .obj(2, b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>")
    .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
    .obj(4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
    .obj(5, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
    // PDF 1.1 /Dests dictionary: one direct array, one dict with /D
    .obj(
        20,
        b"<< /Intro [3 0 R /Fit] /Appendix << /D [5 0 R /XYZ 0 100 0] >> >>",
    )
    // PDF 1.2+ /Names /Dests name tree with /Kids
    .obj(21, b"<< /Kids [22 0 R 23 0 R] >>")
    .obj(22, b"<< /Limits [(ch1) (ch2)] /Names [(ch1) [3 0 R /Fit] (ch2) << /D [4 0 R /FitH 50] >>] >>")
    .obj(23, b"<< /Limits [(ch3) (ch3)] /Names [(ch3) 24 0 R] >>")
    .obj(24, b"[5 0 R /Fit]")
    // Outlines root
    .obj(10, b"<< /Type /Outlines /First 11 0 R /Last 15 0 R /Count 6 >>")
    // Depth 0: explicit /Dest -> page 0
    .obj(
        11,
        b"<< /Title (Chapter 1) /Parent 10 0 R /First 12 0 R /Last 13 0 R /Next 14 0 R /Dest [3 0 R /XYZ 0 100 0] >>",
    )
    // Depth 1: named dest via Name in /Dests dict -> page 2
    .obj(
        12,
        b"<< /Title (Section 1.1 \\204 Overview) /Parent 11 0 R /Next 13 0 R /Dest /Appendix >>",
    )
    // Depth 1: named dest via /A GoTo String in /Names /Dests tree -> page 1
    .obj(
        13,
        b"<< /Title <FEFF00530065006300200031002E0032002000E9> /Parent 11 0 R /A << /S /GoTo /D (ch2) >> >>",
    )
    // Depth 0: indirect destination in /Names /Dests leaf -> page 2
    .obj(
        14,
        b"<< /Title (Chapter 2) /Parent 10 0 R /Next 15 0 R /Dest (ch3) >>",
    )
    // Depth 0: external URI action (not GoTo) -> None, with child pointing to unresolved name -> None
    .obj(
        15,
        b"<< /Title (External Link) /Parent 10 0 R /First 16 0 R /A << /S /URI /URI (https://example.com) >> >>",
    )
    .obj(
        16,
        b"<< /Title (Broken Dest) /Parent 15 0 R /Dest (does-not-exist) >>",
    );

    let doc = Document::open(b.build()).expect("valid pdf");
    let outline = doc.outline();
    assert_eq!(
        outline,
        vec![
            OutlineEntry {
                title: "Chapter 1".into(),
                page: Some(0),
                depth: 0,
            },
            OutlineEntry {
                title: "Section 1.1 \u{2014} Overview".into(),
                page: Some(2),
                depth: 1,
            },
            OutlineEntry {
                title: "Sec 1.2 \u{00E9}".into(),
                page: Some(1),
                depth: 1,
            },
            OutlineEntry {
                title: "Chapter 2".into(),
                page: Some(2),
                depth: 0,
            },
            OutlineEntry {
                title: "External Link".into(),
                page: None,
                depth: 0,
            },
            OutlineEntry {
                title: "Broken Dest".into(),
                page: None,
                depth: 1,
            },
        ]
    );
}

#[test]
fn outline_cycle_and_excessive_depth_are_guarded() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
        .obj(10, b"<< /Type /Outlines /First 11 0 R >>")
        // 11 -> Next 12 -> Next 11 (sibling cycle), and 12 -> First 11 (ancestor cycle)
        .obj(11, b"<< /Title (A) /Next 12 0 R /Dest [3 0 R /Fit] >>")
        .obj(12, b"<< /Title (B) /Next 11 0 R /First 11 0 R /Dest [3 0 R /Fit] >>");

    let doc = Document::open(b.build()).expect("valid pdf");
    assert_eq!(
        doc.outline(),
        vec![
            OutlineEntry {
                title: "A".into(),
                page: Some(0),
                depth: 0,
            },
            OutlineEntry {
                title: "B".into(),
                page: Some(0),
                depth: 0,
            },
        ]
    );

    // Build a chain of 5500 outline siblings to verify the entry cap (~5000).
    let mut big = PdfBuilder::new(1);
    big.obj(1, b"<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
        .obj(10, b"<< /Type /Outlines /First 100 0 R >>");
    for i in 0..5_500u32 {
        let obj_id = 100 + i;
        let next = if i + 1 < 5_500 {
            format!(" /Next {} 0 R", obj_id + 1)
        } else {
            String::new()
        };
        big.obj(obj_id, format!("<< /Title (Item {i}){next} >>"));
    }
    let big_doc = Document::open(big.build()).expect("valid pdf");
    assert_eq!(big_doc.outline().len(), 5_000);
}

#[test]
fn metadata_pdf_doc_encoding_and_utf16be() {
    // PDFDocEncoding: 0x84 = em dash, 0xA0 = euro sign, 0xE9 = é
    let mut b1 = PdfBuilder::new(1);
    b1.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
        .obj(
            4,
            b"<< /Title (Notes \\204 \\24010) /Author (Ren\\351 Descartes) >>",
        )
        .info(4);

    let doc1 = Document::open(b1.build()).expect("valid pdf");
    assert_eq!(
        doc1.metadata(),
        Metadata {
            title: "Notes \u{2014} \u{20AC}10".into(),
            author: "Ren\u{00E9} Descartes".into(),
        }
    );

    // UTF-16BE with BOM: "Σύνοψη" and "山田"
    let mut b2 = PdfBuilder::new(1);
    b2.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
        .obj(
            4,
            b"<< /Title <FEFF03A303CD03BD03BF03C803B7> /Author <FEFF5C717530> >>",
        )
        .info(4);

    let doc2 = Document::open(b2.build()).expect("valid pdf");
    assert_eq!(
        doc2.metadata(),
        Metadata {
            title: "Σύνοψη".into(),
            author: "山田".into(),
        }
    );

    // Absent /Info -> empty strings
    let mut b3 = PdfBuilder::new(1);
    b3.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>");
    let doc3 = Document::open(b3.build()).expect("valid pdf");
    assert_eq!(doc3.metadata(), Metadata::default());
}

#[test]
fn render_filled_rectangle_on_white_background() {
    // 100x100pt page with a red filled rectangle at PDF coords x=20..60, y=20..60.
    // Because PDF y goes upward from bottom-left, y=20..60 maps to top-down y=40..80.
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>",
        )
        .obj(4, stream_obj(b"1 0 0 rg 20 20 40 40 re f"));

    let doc = Document::open(b.build()).expect("valid pdf");

    let bmp = doc.render(0, 1.0).expect("render at 1.0");
    assert_eq!(bmp.width, 100);
    assert_eq!(bmp.height, 100);
    assert_eq!(bmp.rgba.len(), 100 * 100 * 4);

    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * bmp.width + x) * 4) as usize;
        [bmp.rgba[i], bmp.rgba[i + 1], bmp.rgba[i + 2], bmp.rgba[i + 3]]
    };

    // Corners must be opaque white background
    assert_eq!(pixel(5, 5), [255, 255, 255, 255]);
    assert_eq!(pixel(95, 95), [255, 255, 255, 255]);
    // Inside the red rectangle (x in 20..60, top-down y in 40..80)
    assert_eq!(pixel(40, 60), [255, 0, 0, 255]);
    // Above the rectangle (top-down y=15) must be white
    assert_eq!(pixel(40, 15), [255, 255, 255, 255]);

    // Render again at scale 2.0 to exercise scaling and cache reset
    let bmp2 = doc.render(0, 2.0).expect("render at 2.0");
    assert_eq!(bmp2.width, 200);
    assert_eq!(bmp2.height, 200);
    let i = ((120 * bmp2.width + 80) * 4) as usize;
    assert_eq!(&bmp2.rgba[i..i + 4], &[255, 0, 0, 255]);
}

#[test]
fn render_rejects_too_large_and_invalid_inputs() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1000 1000] >>");

    let doc = Document::open(b.build()).expect("valid pdf");

    // 1000 * 10 = 10_000 -> 100_000_000 pixels > MAX_PIXELS (40_000_000)
    const { assert!(10_000u64 * 10_000u64 > MAX_PIXELS) };
    assert_eq!(doc.render(0, 10.0), Err(Error::TooLarge));
    assert_eq!(doc.render(0, f32::INFINITY), Err(Error::TooLarge));

    // Invalid scale / out-of-range page never panic
    assert!(matches!(doc.render(0, 0.0), Err(Error::Unsupported(_))));
    assert!(matches!(doc.render(0, -1.0), Err(Error::Unsupported(_))));
    assert!(matches!(doc.render(0, f32::NAN), Err(Error::Unsupported(_))));
    assert!(matches!(doc.render(5, 1.0), Err(Error::Unsupported(_))));
}

#[test]
fn render_rotated_page_swaps_bitmap_dimensions() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] /Rotate 90 /Contents 4 0 R >>",
        )
        // Fill bottom-left corner of unrotated page (x=0..40, y=0..40) in blue.
        // Rotating 90 deg CW moves unrotated bottom-left to rotated top-left.
        .obj(4, stream_obj(b"0 0 1 rg 0 0 40 40 re f"));

    let doc = Document::open(b.build()).expect("valid pdf");
    let bmp = doc.render(0, 1.0).expect("render rotated page");
    assert_eq!(bmp.width, 200);
    assert_eq!(bmp.height, 100);

    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * bmp.width + x) * 4) as usize;
        [bmp.rgba[i], bmp.rgba[i + 1], bmp.rgba[i + 2], bmp.rgba[i + 3]]
    };

    assert_eq!(pixel(20, 20), [0, 0, 255, 255]);
    assert_eq!(pixel(180, 80), [255, 255, 255, 255]);
}

#[test]
fn damaged_inputs_return_error_damaged() {
    assert_eq!(Document::open(Vec::new()).err(), Some(Error::Damaged));
    assert_eq!(
        Document::open(b"this is definitely not a PDF file".to_vec()).err(),
        Some(Error::Damaged)
    );
    assert_eq!(
        Document::open(b"%PDF-1.7\ntruncated garbage".to_vec()).err(),
        Some(Error::Damaged)
    );
}

#[test]
fn encrypted_trailer_returns_error_encrypted() {
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>")
        // Standard RC4 encryption dictionary with non-matching /U (user password required)
        .obj(
            4,
            b"<< /Filter /Standard /V 1 /R 2 /O <000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F> /U <1111111111111111111111111111111111111111111111111111111111111111> /P -4 >>",
        )
        .encrypt("4 0 R")
        .file_id("0123456789ABCDEF0123456789ABCDEF");

    assert_eq!(Document::open(b.build()).err(), Some(Error::Encrypted));
}

// ---- Text ------------------------------------------------------------------------------------

/// A one-page PDF whose page dictionary has `page_extra` and whose resources hold the standard
/// fonts `/F1` (Helvetica) and `/F2` (Times-Bold) plus `fonts_extra`, showing `content`.
fn text_pdf(page_extra: &str, content: &str, fonts_extra: &str, more: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(
            3,
            format!(
                "<< /Type /Page /Parent 2 0 R {page_extra} /Resources << /Font << /F1 5 0 R /F2 6 0 R {fonts_extra} >> >> /Contents 4 0 R >>"
            ),
        )
        .obj(4, stream_obj(content.as_bytes()))
        .obj(5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
        .obj(6, b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold /Encoding /WinAnsiEncoding >>");
    for (id, body) in more {
        b.obj(*id, body.clone());
    }
    b.build()
}

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

#[test]
fn text_runs_have_unicode_and_page_geometry() {
    let pdf = text_pdf("/MediaBox [0 0 612 792]", "BT /F1 12 Tf 72 720 Td (Hello world) Tj ET", "", &[]);
    let doc = Document::open(pdf).expect("valid pdf");
    let runs = doc.text(0).expect("text");
    assert_eq!(runs.len(), 1, "{runs:?}");
    let run = &runs[0];
    assert_eq!(run.text, "Hello world");
    assert!(close(run.font_size, 12.0, 0.01), "{run:?}");
    assert!(close(run.baseline_x, 72.0, 0.01) && close(run.baseline_y, 72.0, 0.01), "{run:?}");
    assert!(close(run.angle, 0.0, 1e-4));
    // Helvetica's widths for "Hello world" add up to 4945/1000 em.
    assert!(close(run.length, 59.34, 0.05), "{run:?}");
    assert!(close(run.x, 72.0, 0.01) && close(run.y, 72.0 - 9.6, 0.01), "{run:?}");
    assert!(close(run.width, run.length, 0.01) && close(run.height, 12.0, 0.01), "{run:?}");
    assert!(run.ends_line);
    assert_eq!(run.chars.len(), 11);
    assert_eq!(run.chars.iter().map(|c| c.text.as_str()).collect::<String>(), run.text);
    // "H" is 722 units wide, so "e" starts 8.664 points along the baseline.
    assert!(close(run.chars[1].offset, 8.664, 0.01), "{:?}", run.chars[1]);
    assert!(run.chars.windows(2).all(|w| w[0].offset < w[1].offset));
    let last = run.chars.last().unwrap();
    assert!(close(last.offset + last.advance, run.length, 0.01));

    assert!(matches!(doc.text(1), Err(Error::Unsupported(_))));
}

#[test]
fn text_joins_fonts_on_a_line_and_separates_words_and_lines() {
    let content = "BT /F1 12 Tf 72 720 Td (Plain ) Tj /F2 12 Tf (Bold) Tj ET \
                   BT /F1 12 Tf 72 700 Td [(Word) -600 (Gap)] TJ ET \
                   BT /F2 10 Tf 400 700 Td (Column) Tj ET \
                   BT /F1 12 Tf 72 680 Td [(Ke) 40 (rned)] TJ ET";
    let doc = Document::open(text_pdf("/MediaBox [0 0 612 792]", content, "", &[])).expect("valid pdf");
    let runs = doc.text(0).expect("text");
    let texts: Vec<(&str, bool)> = runs.iter().map(|r| (r.text.as_str(), r.ends_line)).collect();
    assert_eq!(
        texts,
        vec![("Plain Bold", true), ("Word Gap", false), ("Column", true), ("Kerned", true)],
        "{runs:?}"
    );
    // The word gap is a space character covering the TJ adjustment.
    let gap = &runs[1].chars[4];
    assert_eq!(gap.text, " ");
    assert!(close(gap.advance, 7.2, 0.05), "{gap:?}");
    assert!(close(runs[2].font_size, 10.0, 0.01));
    assert!(close(runs[3].baseline_y, 112.0, 0.01));
}

/// Whether the bitmap has a dark pixel inside the box (in points at scale 1).
fn has_ink(bmp: &dar_pdf::Bitmap, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    let (x0, y0) = (x0.max(0.0) as u32, y0.max(0.0) as u32);
    let (x1, y1) = ((x1.ceil() as u32).min(bmp.width), (y1.ceil() as u32).min(bmp.height));
    (y0..y1).any(|y| (x0..x1).any(|x| bmp.rgba[((y * bmp.width + x) * 4) as usize] < 128))
}

#[test]
fn text_follows_rotate_and_crop_box_like_render() {
    // Rotated a quarter turn clockwise: text running right in user space runs down the page.
    let rotated = text_pdf("/MediaBox [0 0 200 400] /Rotate 90", "BT /F1 20 Tf 20 380 Td (MMMM) Tj ET", "", &[]);
    let doc = Document::open(rotated).expect("valid pdf");
    let run = doc.text(0).expect("text").remove(0);
    assert_eq!(run.text, "MMMM");
    assert!(close(run.angle, std::f32::consts::FRAC_PI_2, 1e-3), "{run:?}");
    assert!(close(run.baseline_x, 380.0, 0.01) && close(run.baseline_y, 20.0, 0.01), "{run:?}");
    assert!(close(run.height, run.length, 0.01) && close(run.width, 20.0, 0.01), "{run:?}");
    let bmp = doc.render(0, 1.0).expect("render");
    assert!(has_ink(&bmp, run.x, run.y, run.x + run.width, run.y + run.height));
    assert!(!has_ink(&bmp, 0.0, 0.0, run.x - 2.0, 200.0));
    assert!(!has_ink(&bmp, run.x + run.width + 2.0, 0.0, 400.0, 200.0));

    // A crop box moves the origin to its own top-left corner.
    let cropped = text_pdf("/MediaBox [0 0 600 800] /CropBox [100 100 400 500]", "BT /F2 24 Tf 150 450 Td (WW) Tj ET", "", &[]);
    let doc = Document::open(cropped).expect("valid pdf");
    let run = doc.text(0).expect("text").remove(0);
    assert!(close(run.baseline_x, 50.0, 0.01) && close(run.baseline_y, 50.0, 0.01), "{run:?}");
    let bmp = doc.render(0, 1.0).expect("render");
    assert!(has_ink(&bmp, run.x, run.y, run.x + run.width, run.y + run.height));
    assert!(!has_ink(&bmp, run.x + run.width + 2.0, 0.0, 300.0, 400.0));
    assert!(!has_ink(&bmp, 0.0, run.y + run.height + 2.0, 300.0, 400.0));

    // A scaled and rotated text matrix (45 degrees, 2x) keeps size and angle.
    let turned = text_pdf("/MediaBox [0 0 300 300]", "BT /F1 10 Tf 1.41421 1.41421 -1.41421 1.41421 100 100 Tm (Up) Tj ET", "", &[]);
    let run = Document::open(turned).expect("valid pdf").text(0).expect("text").remove(0);
    assert!(close(run.font_size, 20.0, 0.01), "{run:?}");
    assert!(close(run.angle, -std::f32::consts::FRAC_PI_4, 1e-3), "{run:?}");
}

#[test]
fn text_uses_to_unicode_and_skips_duplicate_and_keeps_invisible_text() {
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
/CMapName /Test def /CMapType 2 def\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
2 beginbfchar\n<41> <00E9>\n<42> <00660069>\nendbfchar\n\
endcmap CMapName currentdict /CMap defineresource pop end end";
    let content = "BT /F3 12 Tf 72 720 Td (AB) Tj ET \
                   BT 2 Tr /F1 12 Tf 72 700 Td (Twice) Tj ET \
                   BT 3 Tr /F1 12 Tf 72 680 Td (Hidden OCR) Tj ET";
    let pdf = text_pdf(
        "/MediaBox [0 0 612 792]",
        content,
        "/F3 7 0 R",
        &[
            (7, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 8 0 R >>".to_vec()),
            (8, stream_obj(cmap)),
        ],
    );
    let runs = Document::open(pdf).expect("valid pdf").text(0).expect("text");
    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, vec!["\u{e9}fi", "Twice", "Hidden OCR"], "{runs:?}");
    assert_eq!(runs[0].chars.len(), 2);
    assert_eq!(runs[0].chars[1].text, "fi");
}
