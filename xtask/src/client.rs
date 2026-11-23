//! The browser client's dist folder (`target/client-fast` or `target/client-release`): the
//! wasm split per app, its JS glue, the PDF renderer Web Worker (`dar-pdf-worker`), Dar's
//! stylesheet in cascade order, the installed-app SDK bundle, the microphone worklet and
//! index.html, with content-hashed names so the runtime can serve them as immutable. Release
//! builds also run wasm-opt and write gzip and brotli copies.
//!
//! Sources: `crates/client` (Leptos), `crates/pdf-worker` (`dar-pdf` in wasm32),
//! `crates/client/styles` (with `order.txt`) and the SDK bundle (`DAR_SDK_DIST`, default
//! `sdk/dist`).

use crate::{Profile, Result, cargo, client_dist, hash, repo, run, step, target, which};
use std::path::{Path, PathBuf};
use std::process::Command;

const MIC_WORKLET: &str = r#"class MicFrames extends AudioWorkletProcessor {
  frame = new Float32Array(960);
  filled = 0;
  process(inputs) {
    const input = inputs[0]?.[0];
    if (!input) return true;
    let offset = 0;
    while (offset < input.length) {
      const take = Math.min(input.length - offset, 960 - this.filled);
      this.frame.set(input.subarray(offset, offset + take), this.filled);
      this.filled += take;
      offset += take;
      if (this.filled === 960) {
        this.port.postMessage(this.frame, [this.frame.buffer]);
        this.frame = new Float32Array(960);
        this.filled = 0;
      }
    }
    return true;
  }
}
registerProcessor("dar-mic", MicFrames);
"#;

const PDF_WORKER_BOOTSTRAP: &str = r#"import init, { handle_message } from "./dar_pdf_worker.js";
const pending = [];
let ready = false;
self.onmessage = (event) => {
  if (ready) {
    handle_message(event.data);
  } else {
    pending.push(event.data);
  }
};
init({ module_or_path: new URL("./dar_pdf_worker_bg.wasm", import.meta.url) })
  .then(() => {
    ready = true;
    for (const message of pending.splice(0)) {
      handle_message(message);
    }
  })
  .catch((err) => {
    self.postMessage({ type: "fatal", message: String(err?.message ?? err ?? "PDF worker failed to start.") });
  });
"#;

fn io<T>(result: std::io::Result<T>, what: impl std::fmt::Display) -> Result<T> {
    result.map_err(|e| format!("{what}: {e}"))
}

pub fn build(profile: Profile) -> Result {
    let repo = repo();
    let wasm_profile = profile.wasm();
    step(&format!("client (wasm32, {wasm_profile})"), || {
        // Relocations let wasm-split-cli move each #[lazy] app (src/lazy_apps.rs) into its own chunk.
        let flags = std::env::var("RUSTFLAGS").ok().filter(|f| !f.is_empty()).map(|f| f + " ").unwrap_or_default();
        run(cargo().args(["build", "-q", "-p", "dar-client", "--bin", "dar-client", "--target", "wasm32-unknown-unknown", "--profile", wasm_profile])
            .env("RUSTFLAGS", format!("{flags}--cfg=erase_components -C link-args=--emit-relocs")))
    })?;
    step(&format!("pdf worker (wasm32, {wasm_profile})"), || {
        // SIMD128 makes hayro faster and its wasm a quarter smaller. Its own target directory
        // keeps these flags from rebuilding the client's shared dependencies.
        run(cargo().args(["build", "-q", "-p", "dar-pdf-worker", "--target", "wasm32-unknown-unknown", "--profile", wasm_profile])
            .env("CARGO_TARGET_DIR", target().join("pdf-worker"))
            .env("RUSTFLAGS", "-C target-feature=+simd128"))
    })?;
    let wasm = target().join("wasm32-unknown-unknown").join(wasm_profile).join("dar-client.wasm");
    let worker_wasm = target().join("pdf-worker").join("wasm32-unknown-unknown").join(wasm_profile).join("dar_pdf_worker.wasm");
    let sdk = std::env::var_os("DAR_SDK_DIST").map(PathBuf::from).unwrap_or_else(|| repo.join("sdk/dist"));
    let styles = repo.join("crates/client/styles");
    let stylesheet = stylesheet(&styles)?;
    // Everything the dist is made from; an unchanged stamp skips the post-processing.
    let mut inputs = hash::Sha256::default();
    inputs.update(io(hash::file(&wasm), wasm.display())?.as_bytes());
    inputs.update(io(hash::file(&worker_wasm), worker_wasm.display())?.as_bytes());
    inputs.update(hash::bytes(PDF_WORKER_BOOTSTRAP.as_bytes()).as_bytes());
    inputs.update(hash::bytes(stylesheet.as_bytes()).as_bytes());
    for file in listing(&sdk) {
        inputs.update(file.to_string_lossy().as_bytes());
        inputs.update(hash::file(&file).unwrap_or_default().as_bytes());
    }
    inputs.update(format!("{profile:?} 4").as_bytes());
    let stamp = hash::hex(&inputs.finish());
    let out = client_dist(profile);
    if std::fs::read_to_string(out.join(".dar-inputs")).is_ok_and(|s| s == stamp) && out.join("index.html").exists() {
        return Ok(());
    }
    step("client dist", || assemble(profile, &wasm, &worker_wasm, &sdk, &stylesheet, &out, &stamp))
}

/// Dar's stylesheets concatenated in the cascade order of `styles/order.txt`.
fn stylesheet(styles: &Path) -> Result<String> {
    let order = io(std::fs::read_to_string(styles.join("order.txt")), "styles/order.txt")?;
    let mut css = String::new();
    for sheet in order.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        css.push_str(&io(std::fs::read_to_string(styles.join(sheet)), format!("styles/{sheet}"))?);
        css.push('\n');
    }
    Ok(css)
}

/// Files under `dir`, sorted, as paths.
fn listing(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() { stack.push(path) } else { files.push(path) }
        }
    }
    files.sort();
    files
}

fn dir_hash(dir: &Path) -> Result<String> {
    let mut digest = hash::Sha256::default();
    for file in listing(dir) {
        digest.update(file.strip_prefix(dir).unwrap().to_string_lossy().as_bytes());
        digest.update(io(hash::file(&file), file.display())?.as_bytes());
    }
    Ok(hash::hex(&digest.finish()))
}

fn assemble(profile: Profile, wasm: &Path, worker_wasm: &Path, sdk: &Path, stylesheet: &str, out: &Path, stamp: &str) -> Result {
    let work = out.with_extension("tmp");
    let _ = std::fs::remove_dir_all(&work);
    let (split, app, pdf_worker) = (work.join("split"), work.join("app"), work.join("pdf-worker"));
    io(std::fs::create_dir_all(&split), split.display())?;
    io(std::fs::create_dir_all(&app), app.display())?;
    io(std::fs::create_dir_all(&pdf_worker), pdf_worker.display())?;
    for tool in ["wasm-split-cli", "wasm-bindgen"] {
        if !which(tool) {
            return Err(format!("{tool} is missing: use Dar's dev shell (nix develop)"));
        }
    }
    run(Command::new("wasm-split-cli").args(["--target", "web", "--out-name", "dar_client"]).arg(wasm).arg(&split))?;
    // The chunks reach the main module's function table through LLD's exports, which
    // wasm-bindgen drops unless told to keep them.
    run(Command::new("wasm-bindgen").args(["--target", "web", "--no-typescript", "--keep-lld-exports", "--out-dir"]).arg(&app)
        .args(["--out-name", "dar_client"]).arg(split.join("dar_client.wasm")))?;
    io(std::fs::remove_file(split.join("dar_client.wasm")), "split main wasm")?;
    // Per-app chunks (split_*) and the code they share (chunk_*).
    for file in listing(&split) {
        io(std::fs::rename(&file, app.join(file.file_name().unwrap())), file.display())?;
    }
    io(std::fs::remove_dir_all(&split), "split")?;

    // PDF renderer Web Worker (dar-pdf-worker): ES module glue, wasm and bootstrap.
    run(Command::new("wasm-bindgen").args(["--target", "web", "--no-typescript", "--out-dir"]).arg(&pdf_worker)
        .args(["--out-name", "dar_pdf_worker"]).arg(worker_wasm))?;
    io(std::fs::write(pdf_worker.join("worker.js"), PDF_WORKER_BOOTSTRAP), "pdf worker bootstrap")?;

    let mut wasm_files: Vec<PathBuf> = listing(&app).into_iter().filter(|f| f.extension().is_some_and(|e| e == "wasm")).collect();
    wasm_files.extend(listing(&pdf_worker).into_iter().filter(|f| f.extension().is_some_and(|e| e == "wasm")));
    if profile == Profile::Release && which("wasm-opt") {
        parallel(&wasm_files, |chunk| run(Command::new("wasm-opt").args(["-Oz", "--strip-debug", "--strip-producers", "--low-memory-unused", "--zero-filled-memory",
            "--enable-bulk-memory", "--enable-simd", "--enable-nontrapping-float-to-int", "--enable-sign-ext", "--enable-reference-types", "--enable-mutable-globals", "-o"]).arg(chunk).arg(chunk)))?;
    }
    io(std::fs::write(work.join("mic-worklet.js"), MIC_WORKLET), "mic worklet")?;
    // The installed-app SDK bundle (sdk.js, sdk.css, chunks).
    if sdk.join("sdk.js").exists() {
        for file in listing(sdk) {
            let relative = file.strip_prefix(sdk).unwrap();
            let name = relative.to_string_lossy();
            if (name.starts_with("sdk") && (name.ends_with(".js") || name.ends_with(".css")) && !name.contains('/')) || name.starts_with("chunks/") {
                let destination = work.join(relative);
                io(std::fs::create_dir_all(destination.parent().unwrap()), destination.display())?;
                io(std::fs::copy(&file, &destination), file.display())?;
            }
        }
    }
    // The glue, wasm, app chunks and snippets load each other by relative URL, so they share
    // one folder named after all of their contents; the PDF worker does the same.
    let app_dir = format!("app-{}", &dir_hash(&app)?[..12]);
    io(std::fs::rename(&app, work.join(&app_dir)), "app folder")?;
    let worker_dir = format!("pdf-worker-{}", &dir_hash(&pdf_worker)?[..12]);
    io(std::fs::rename(&pdf_worker, work.join(&worker_dir)), "pdf-worker folder")?;
    let css_file = format!("app.{}.css", &hash::bytes(stylesheet.as_bytes())[..12]);
    io(std::fs::write(work.join(&css_file), stylesheet), "stylesheet")?;
    // Also unhashed, for tools and tests that read the stylesheet.
    io(std::fs::write(work.join("app.css"), stylesheet), "app.css")?;
    let (js_file, wasm_file) = (format!("{app_dir}/dar_client.js"), format!("{app_dir}/dar_client_bg.wasm"));
    let worker_file = format!("{worker_dir}/worker.js");
    let start = format!("import init from \"/rust/{js_file}\";\ninit({{module_or_path:\"/rust/{wasm_file}\"}});\n");
    let start_file = format!("start.{}.js", &hash::bytes(start.as_bytes())[..12]);
    io(std::fs::write(work.join(&start_file), &start), "start.js")?;
    io(std::fs::write(work.join("index.html"), format!(
        "<!doctype html><html lang=\"en\" data-pdf-worker=\"/rust/{worker_file}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\"><title>Dar</title><link rel=\"stylesheet\" href=\"/rust/{css_file}\"><link rel=\"modulepreload\" href=\"/rust/{start_file}\"><link rel=\"modulepreload\" href=\"/rust/{js_file}\"><link rel=\"preload\" href=\"/rust/{wasm_file}\" as=\"fetch\" crossorigin=\"anonymous\"><script type=\"module\" src=\"/rust/{start_file}\"></script></head><body><div id=\"root\"><main class=\"startup-state\" aria-busy=\"true\"><div class=\"brand-mark\"></div><p role=\"status\">Opening your space…</p></main></div></body></html>\n")), "index.html")?;
    // Precompressed copies (release only; the suite's server sends the files as they are).
    if profile == Profile::Release {
        let compressible: Vec<PathBuf> = listing(&work).into_iter().filter(|f| f.extension().is_some_and(|e| ["wasm", "js", "css", "mjs"].contains(&e.to_string_lossy().as_ref()))).collect();
        let brotli = which("brotli");
        parallel(&compressible, |file| {
            run(Command::new("gzip").args(["-9", "-k", "-f"]).arg(file))?;
            if brotli { run(Command::new("brotli").args(["-q", "11", "-k", "-f"]).arg(file)) } else { Ok(()) }
        })?;
    }
    io(std::fs::write(work.join(".dar-inputs"), stamp), "stamp")?;
    let _ = std::fs::remove_dir_all(out);
    io(std::fs::rename(&work, out), out.display())?;
    Ok(())
}

/// Runs `work` over `items` on every core.
fn parallel(items: &[PathBuf], work: impl Fn(&Path) -> Result + Sync) -> Result {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failure = std::sync::Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..threads.min(items.len().max(1)) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(item) = items.get(index) else { break };
                if let Err(error) = work(item) {
                    *failure.lock().unwrap() = Some(error);
                }
            });
        }
    });
    failure.into_inner().unwrap().map_or(Ok(()), Err)
}
