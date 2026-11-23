//! The published crates the workspace patches (`[patch.crates-io]` in Cargo.toml), prepared
//! in `target/patches` from their crates.io archives, checked against their checksums. Set
//! `DAR_CRATE_DIR` to a folder with the `.crate` files to work offline (the Nix build does).

use crate::{Result, hash, repo, run, target};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Patch {
    name: &'static str,
    version: &'static str,
    sha256: &'static str,
    /// Bumped when the edits change, so an older prepared copy is redone.
    revision: u32,
    edits: &'static [(&'static str, &'static str, &'static str)],
    append: Option<(&'static str, &'static str)>,
}

const PATCHES: &[Patch] = &[
    // SpacetimeDB 2.14.1's derive names a product field after `ident.to_string()`, so a raw
    // identifier keeps its `r#` (`r#type`) and the module is rejected; two tables need a column
    // named `type`. Index accessors are recorded in camelCase (`fileId`), as the TypeScript
    // module named them, so a TypeScript database upgrades in place; Rust methods stay snake_case.
    Patch {
        name: "spacetimedb-bindings-macro",
        version: "2.14.1",
        sha256: "12687a386f7994d79a66b5e483335ffa3a4d4b4793ad29dbd640e09113c6b188",
        revision: 2,
        edits: &[
            ("src/sats.rs", "name: field.ident.as_ref().map(syn::Ident::to_string),", "name: field.ident.as_ref().map(|i| syn::ext::IdentExt::unraw(i).to_string()),"),
            ("src/table.rs", "let accessor_name = self.accessor_name.to_string();",
                "let accessor_name = { let mut out = String::new(); let mut upper = false; for c in syn::ext::IdentExt::unraw(self.accessor_name).to_string().chars() { if c == '_' { upper = true; } else if upper { out.extend(c.to_uppercase()); upper = false; } else { out.push(c); } } out };"),
        ],
        append: None,
    },
    // The SDK depends on brotli 3.5 with its default features, which include the C API
    // (`ffi-api`); its exported functions kept the whole encoder in the client (a third of the
    // download) although the SDK only decompresses.
    Patch {
        name: "brotli",
        version: "3.5.0",
        sha256: "d640d25bc63c50fb1f0b545ffd80207d2e10a4c965530809b40ba3386825c391",
        revision: 1,
        edits: &[("Cargo.toml", "default = [\n    \"std\",\n    \"ffi-api\",\n]", "default = [\"std\"]")],
        // Its own lints are not ours to fix.
        append: Some(("Cargo.toml", "\n[lints.rust]\nunexpected_cfgs = \"allow\"\ndead_code = \"allow\"\nmismatched_lifetime_syntaxes = \"allow\"\n")),
    },
];

pub fn prepare() -> Result {
    let dir = target().join("patches");
    for patch in PATCHES {
        let out = dir.join(format!("{}-{}", patch.name, patch.version));
        let marker = out.join(format!(".dar-patched-{}", patch.revision));
        if marker.exists() {
            continue;
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let archive = fetch(patch)?;
        let _ = std::fs::remove_dir_all(&out);
        run(Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&dir))?;
        for (file, from, to) in patch.edits {
            let path = out.join(file);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !text.contains(from) {
                return Err(format!("{} {}: the patched text is missing from {file}", patch.name, patch.version));
            }
            std::fs::write(&path, text.replacen(from, to, 1)).map_err(|e| e.to_string())?;
        }
        if let Some((file, text)) = patch.append {
            let path = out.join(file);
            let mut contents = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            contents.push_str(text);
            std::fs::write(&path, contents).map_err(|e| e.to_string())?;
        }
        std::fs::write(&marker, "").map_err(|e| e.to_string())?;
        eprintln!("xtask: prepared {} {} in {}", patch.name, patch.version, out.strip_prefix(repo()).unwrap_or(&out).display());
    }
    Ok(())
}

/// The crate archive, from `DAR_CRATE_DIR` or crates.io, with its checksum verified.
fn fetch(patch: &Patch) -> Result<PathBuf> {
    let file = format!("{}-{}.crate", patch.name, patch.version);
    let path = match std::env::var_os("DAR_CRATE_DIR") {
        Some(dir) => Path::new(&dir).join(&file),
        None => {
            let path = target().join("patches").join(&file);
            if !path.exists() {
                let url = format!("https://static.crates.io/crates/{}/{file}", patch.name);
                run(Command::new("curl").args(["-sSfL", &url, "-o"]).arg(&path))?;
            }
            path
        }
    };
    let digest = hash::file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if digest != patch.sha256 {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{file}: checksum {digest} does not match crates.io's {}", patch.sha256));
    }
    Ok(path)
}
