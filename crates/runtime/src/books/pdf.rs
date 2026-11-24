//! Child-process PDF inspection and cover rendering backed by `dar-pdf` (hayro).
//!
//! Hostile PDFs must never hang, panic, or exhaust the runtime process, so `BooksRuntime` invokes
//! the current `dar-runtime` executable with the hidden `pdf-info` and `pdf-cover` subcommands
//! under the runtime's concurrency slots, wall-clock timeouts, output caps, and (on 64-bit Linux)
//! an `RLIMIT_AS` address-space cap.

use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use dar_pdf::{Bitmap, Document, Error as PdfError};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Longer-edge render size for PDF covers before `vipsthumbnail --size 320x480>` encodes the WebP.
/// Every aspect ratio produces at least the target bounding dimension, avoiding upscaling.
pub(crate) const PDF_COVER_MAX_PX: u32 = 480;

/// Maximum input PDF size read into memory by the child helper.
const MAX_PDF_BYTES: u64 = 512 * 1024 * 1024;

/// Maximum `<max-px>` accepted by `pdf-cover`.
const MAX_COVER_PX_ARG: u32 = 16_384;

/// Virtual address space cap (1 GiB) applied to PDF child processes on 64-bit Linux.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
const PDF_CHILD_ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PdfInfo {
    pub pages: u64,
    pub title: String,
    pub author: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PdfInfoError {
    error: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PdfInfoOutcome {
    Ok(PdfInfo),
    Encrypted,
    Damaged,
}

/// Parses stdout and exit status from `dar-runtime pdf-info <path>`.
pub(crate) fn parse_info_output(ok: bool, stdout: &str) -> PdfInfoOutcome {
    if ok && let Ok(info) = serde_json::from_str::<PdfInfo>(stdout) {
        return PdfInfoOutcome::Ok(info);
    }
    if let Ok(err) = serde_json::from_str::<PdfInfoError>(stdout)
        && err.error == "encrypted"
    {
        return PdfInfoOutcome::Encrypted;
    }
    PdfInfoOutcome::Damaged
}

/// Applies Linux `RLIMIT_AS` in `pre_exec` so a hostile PDF cannot exhaust host memory.
/// Non-Linux targets rely on the wall-clock timeout, input byte cap, and `dar_pdf::MAX_PIXELS`.
pub(crate) fn configure_child_limits(cmd: &mut Command) {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        // SAFETY: `apply_address_space_limit` only invokes Linux's `getrlimit`/`setrlimit`
        // syscall wrappers with stack-allocated structs (no allocations or locks).
        unsafe {
            cmd.pre_exec(apply_address_space_limit);
        }
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    {
        let _ = cmd;
    }
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn apply_address_space_limit() -> io::Result<()> {
    #[repr(C)]
    struct Rlimit {
        rlim_cur: u64,
        rlim_max: u64,
    }
    // `RLIMIT_AS` is 9 across Linux architectures (`asm-generic/resource.h`).
    const RLIMIT_AS: std::ffi::c_int = 9;
    unsafe extern "C" {
        fn getrlimit(resource: std::ffi::c_int, rlim: *mut Rlimit) -> std::ffi::c_int;
        fn setrlimit(resource: std::ffi::c_int, rlim: *const Rlimit) -> std::ffi::c_int;
    }

    let mut current = Rlimit {
        rlim_cur: u64::MAX,
        rlim_max: u64::MAX,
    };
    // SAFETY: `current` matches Linux's 64-bit `struct rlimit` layout.
    if unsafe { getrlimit(RLIMIT_AS, &mut current) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let max = current.rlim_max.min(PDF_CHILD_ADDRESS_SPACE_BYTES);
    let cur = current.rlim_cur.min(max);
    let limit = Rlimit {
        rlim_cur: cur,
        rlim_max: max,
    };
    // SAFETY: `limit` is valid for the duration of the syscall.
    if unsafe { setrlimit(RLIMIT_AS, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Hidden child-process subcommands invoked by `BooksRuntime`.
pub fn command(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str)? {
        "pdf-info" => Some(cmd_pdf_info(&args[1..])),
        "pdf-cover" => Some(cmd_pdf_cover(&args[1..])),
        _ => None,
    }
}

fn read_pdf_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("could not stat PDF: {e}"))?;
    if !meta.is_file() {
        return Err("PDF path is not a regular file.".into());
    }
    if meta.len() > MAX_PDF_BYTES {
        return Err("PDF exceeds maximum supported file size.".into());
    }
    std::fs::read(path).map_err(|e| format!("could not read PDF: {e}"))
}

pub(crate) fn inspect_pdf(path: &Path) -> Result<PdfInfo, (&'static str, String)> {
    let bytes = read_pdf_bytes(path).map_err(|e| ("damaged", e))?;
    let doc = Document::open(bytes).map_err(|err| {
        let kind = match err {
            PdfError::Encrypted => "encrypted",
            PdfError::Damaged | PdfError::Unsupported(_) | PdfError::TooLarge => "damaged",
        };
        (kind, err.to_string())
    })?;
    let meta = doc.metadata();
    Ok(PdfInfo {
        pages: doc.page_count() as u64,
        title: meta.title,
        author: meta.author,
    })
}

pub(crate) fn render_cover_ppm(path: &Path, out_path: &Path, max_px: u32) -> Result<(), String> {
    if max_px == 0 || max_px > MAX_COVER_PX_ARG {
        return Err(format!("max-px must be between 1 and {MAX_COVER_PX_ARG}."));
    }
    let bytes = read_pdf_bytes(path)?;
    let doc = Document::open(bytes).map_err(|err| err.to_string())?;
    let sizes = doc.page_sizes();
    let Some(first) = sizes.first() else {
        return Err("PDF has no pages.".into());
    };
    if !first.width.is_finite()
        || !first.height.is_finite()
        || first.width <= 0.0
        || first.height <= 0.0
    {
        return Err("PDF page 0 has invalid dimensions.".into());
    }
    let longer = first.width.max(first.height);
    let scale = (f64::from(max_px) / f64::from(longer)) as f32;
    if !scale.is_finite() || scale <= 0.0 {
        return Err("PDF page 0 cannot be scaled.".into());
    }
    let bitmap = doc.render(0, scale).map_err(|err| err.to_string())?;
    write_ppm_p6(out_path, &bitmap).map_err(|e| format!("could not write PPM: {e}"))
}

fn write_ppm_p6(path: &Path, bitmap: &Bitmap) -> io::Result<()> {
    if bitmap.width == 0 || bitmap.height == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty bitmap"));
    }
    let expected = (bitmap.width as usize)
        .checked_mul(bitmap.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bitmap size overflow"))?;
    if bitmap.rgba.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bitmap length mismatch",
        ));
    }

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    let mut writer = BufWriter::new(file);
    write!(writer, "P6\n{} {}\n255\n", bitmap.width, bitmap.height)?;
    for px in bitmap.rgba.chunks_exact(4) {
        let [r, g, b, a] = [px[0], px[1], px[2], px[3]];
        if a == 255 {
            writer.write_all(&[r, g, b])?;
        } else {
            let alpha = u16::from(a);
            let inv = 255 - alpha;
            let rgb = [
                ((u16::from(r) * alpha + 255 * inv + 127) / 255) as u8,
                ((u16::from(g) * alpha + 255 * inv + 127) / 255) as u8,
                ((u16::from(b) * alpha + 255 * inv + 127) / 255) as u8,
            ];
            writer.write_all(&rgb)?;
        }
    }
    writer.flush()
}

fn cmd_pdf_info(args: &[String]) -> i32 {
    if args.len() != 1 {
        eprintln!("Usage: dar-runtime pdf-info <path>");
        return 2;
    }
    match inspect_pdf(Path::new(&args[0])) {
        Ok(info) => {
            println!(
                "{}",
                serde_json::to_string(&info).expect("serialize pdf info")
            );
            0
        }
        Err((kind, msg)) => {
            println!(
                "{}",
                serde_json::to_string(&PdfInfoError { error: kind.into() })
                    .expect("serialize pdf error")
            );
            eprintln!("{msg}");
            1
        }
    }
}

fn cmd_pdf_cover(args: &[String]) -> i32 {
    if args.len() != 3 {
        eprintln!("Usage: dar-runtime pdf-cover <path> <out.ppm> <max-px>");
        return 2;
    }
    let Ok(max_px) = args[2].parse::<u32>() else {
        eprintln!("max-px must be a positive integer.");
        return 2;
    };
    if max_px == 0 || max_px > MAX_COVER_PX_ARG {
        eprintln!("max-px must be between 1 and {MAX_COVER_PX_ARG}.");
        return 2;
    }
    match render_cover_ppm(Path::new(&args[0]), Path::new(&args[1]), max_px) {
        Ok(()) => 0,
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dar-pdf-test-{label}-{}", crate::auth::uuid()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn build_pdf(objects: &[String], trailer_extra: &str) -> Vec<u8> {
        let mut body = String::from("%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            offsets.push(body.len());
            body.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref = body.len();
        body.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            body.push_str(&format!("{offset:010} 00000 n \n"));
        }
        body.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra}>>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        body.into_bytes()
    }

    fn sample_pdf() -> Vec<u8> {
        let draw = "0.2 0.4 0.8 rg 10 10 280 380 re f";
        let stream = format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len());
        build_pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R >>".into(),
                stream,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>".into(),
                "<< /Title (A Fixture Paper) /Author (Grace Columns; Ada Page) >>".into(),
            ],
            "/Info 6 0 R ",
        )
    }

    fn encrypted_pdf() -> Vec<u8> {
        build_pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".into(),
                "<< /Filter /Standard /V 1 /R 2 /O <000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F> /U <1111111111111111111111111111111111111111111111111111111111111111> /P -4 >>".into(),
            ],
            "/Encrypt 4 0 R /ID [<0123456789ABCDEF0123456789ABCDEF> <0123456789ABCDEF0123456789ABCDEF>] ",
        )
    }

    #[test]
    fn inspect_pdf_extracts_pages_title_and_author() {
        let dir = temp_dir("info");
        let file = dir.join("sample.pdf");
        std::fs::write(&file, sample_pdf()).unwrap();

        let info = inspect_pdf(&file).expect("valid pdf");
        assert_eq!(
            info,
            PdfInfo {
                pages: 2,
                title: "A Fixture Paper".into(),
                author: "Grace Columns; Ada Page".into(),
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_pdf_distinguishes_encrypted_and_damaged() {
        let dir = temp_dir("errors");
        let enc = dir.join("locked.pdf");
        let bad = dir.join("broken.pdf");
        std::fs::write(&enc, encrypted_pdf()).unwrap();
        std::fs::write(&bad, b"not a pdf").unwrap();

        assert_eq!(inspect_pdf(&enc).unwrap_err().0, "encrypted");
        assert_eq!(inspect_pdf(&bad).unwrap_err().0, "damaged");
        assert_eq!(
            inspect_pdf(&dir.join("missing.pdf")).unwrap_err().0,
            "damaged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_cover_ppm_scales_longer_side_and_writes_valid_p6() {
        let dir = temp_dir("cover");
        let pdf_path = dir.join("sample.pdf");
        let ppm_path = dir.join("cover.ppm");
        std::fs::write(&pdf_path, sample_pdf()).unwrap();

        render_cover_ppm(&pdf_path, &ppm_path, PDF_COVER_MAX_PX).expect("render cover");
        let bytes = std::fs::read(&ppm_path).unwrap();
        // 300x400 scaled to max_px=480 -> 360x480.
        let header = b"P6\n360 480\n255\n";
        assert!(bytes.starts_with(header), "unexpected PPM header");
        let pixels = &bytes[header.len()..];
        assert_eq!(pixels.len(), 360 * 480 * 3);
        // Corner pixel (0, 0) is outside the blue rectangle -> white background.
        assert_eq!(&pixels[0..3], &[255, 255, 255]);
        // Center pixel (180, 240) is inside the filled rectangle -> blue-ish (0.2, 0.4, 0.8).
        let center = (240 * 360 + 180) * 3;
        assert!(
            pixels[center + 2] > pixels[center],
            "expected blue rectangle at center"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_info_output_maps_ok_encrypted_and_damaged() {
        assert_eq!(
            parse_info_output(true, r#"{"pages":3,"title":"T","author":"A"}"#),
            PdfInfoOutcome::Ok(PdfInfo {
                pages: 3,
                title: "T".into(),
                author: "A".into()
            })
        );
        assert_eq!(
            parse_info_output(false, r#"{"error":"encrypted"}"#),
            PdfInfoOutcome::Encrypted
        );
        assert_eq!(
            parse_info_output(false, r#"{"error":"damaged"}"#),
            PdfInfoOutcome::Damaged
        );
        assert_eq!(parse_info_output(false, ""), PdfInfoOutcome::Damaged);
        assert_eq!(parse_info_output(true, "not json"), PdfInfoOutcome::Damaged);
    }
}
