//! Renders every page of every `.pdf` file in a directory at scale 1.5 (or an optional custom
//! scale), printing per-page timings, failures, and a summary table.
//!
//! Usage:
//!   cargo run -p dar-pdf --release --example render -- <directory> [scale]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use dar_pdf::Document;

struct FileStats {
    name: String,
    pages: usize,
    median_ms: Option<f64>,
    max_ms: Option<f64>,
    failures: usize,
    open_error: Option<String>,
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(dir_arg) = args.next() else {
        eprintln!("Usage: render <pdf-directory> [scale]");
        return ExitCode::FAILURE;
    };
    let scale: f32 = match args.next() {
        Some(s) => match s.parse::<f32>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            _ => {
                eprintln!("Invalid scale: {s}");
                return ExitCode::FAILURE;
            }
        },
        None => 1.5,
    };

    let entries = match fs::read_dir(&dir_arg) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to read directory {dir_arg}: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut pdfs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        })
        .collect();
    pdfs.sort();

    if pdfs.is_empty() {
        eprintln!("No .pdf files found in {dir_arg}");
        return ExitCode::SUCCESS;
    }

    let mut summary = Vec::with_capacity(pdfs.len());

    for path in pdfs {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(err) => {
                eprintln!("{name}: FAILED to read: {err}");
                summary.push(FileStats {
                    name,
                    pages: 0,
                    median_ms: None,
                    max_ms: None,
                    failures: 1,
                    open_error: Some(err.to_string()),
                });
                continue;
            }
        };

        let open_start = Instant::now();
        let doc = match Document::open(bytes) {
            Ok(d) => d,
            Err(err) => {
                eprintln!("{name}: FAILED to open: {err}");
                summary.push(FileStats {
                    name,
                    pages: 0,
                    median_ms: None,
                    max_ms: None,
                    failures: 1,
                    open_error: Some(err.to_string()),
                });
                continue;
            }
        };
        let open_ms = open_start.elapsed().as_secs_f64() * 1000.0;
        let pages = doc.page_count();
        let meta = doc.metadata();
        let outline = doc.outline();
        let outline_len = outline.len();
        let outline_resolved = outline.iter().filter(|e| e.page.is_some()).count();

        println!(
            "=== {name}: {pages} pages, {outline_len} outline entries ({outline_resolved} resolved), open {open_ms:.1} ms (title={:?}) ===",
            meta.title
        );

        let mut page_times = Vec::with_capacity(pages);
        let mut failures = 0usize;

        for page_idx in 0..pages {
            let start = Instant::now();
            match doc.render(page_idx, scale) {
                Ok(bmp) => {
                    let ms = start.elapsed().as_secs_f64() * 1000.0;
                    page_times.push(ms);
                    println!(
                        "  page {:>4}/{pages}: {:>7.2} ms ({}x{})",
                        page_idx + 1,
                        ms,
                        bmp.width,
                        bmp.height
                    );
                }
                Err(err) => {
                    let ms = start.elapsed().as_secs_f64() * 1000.0;
                    failures += 1;
                    eprintln!(
                        "  page {:>4}/{pages}: FAILED after {ms:.2} ms: {err}",
                        page_idx + 1
                    );
                }
            }
        }

        page_times.sort_by(f64::total_cmp);
        let median_ms = median(&page_times);
        let max_ms = page_times.last().copied();
        summary.push(FileStats {
            name,
            pages,
            median_ms,
            max_ms,
            failures,
            open_error: None,
        });
    }

    println!("\nSummary (scale {scale}):");
    println!(
        "{:<40} {:>6} {:>12} {:>12} {:>8}",
        "File", "Pages", "Median (ms)", "Max (ms)", "Failures"
    );
    for s in &summary {
        let med = s
            .median_ms
            .map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
        let max = s
            .max_ms
            .map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
        if let Some(err) = &s.open_error {
            println!(
                "{:<40} {:>6} {:>12} {:>12} {:>8} (open: {err})",
                s.name, s.pages, med, max, s.failures
            );
        } else {
            println!(
                "{:<40} {:>6} {:>12} {:>12} {:>8}",
                s.name, s.pages, med, max, s.failures
            );
        }
    }

    ExitCode::SUCCESS
}

fn median(sorted: &[f64]) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    if n % 2 == 1 {
        Some(sorted[n / 2])
    } else {
        Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0)
    }
}
