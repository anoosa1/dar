//! Installed apps (`apps/registry.ts`, `apps/install.ts`, `apps/http.ts`): packages are read
//! once, file by file, and served from memory under their digest; per-user app data lives in
//! the module (`service_app_data`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PERMISSIONS: [&str; 3] = ["ownUserData:read", "ownUserData:write", "regional:read"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageInput {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: Value,
    pub license: Value,
    pub client: String,
    pub styles: Vec<String>,
    pub files: Vec<String>,
    pub permissions: Vec<String>,
    pub server: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Package {
    #[allow(dead_code)]
    pub root: PathBuf,
    pub manifest: Manifest,
    pub manifest_text: Vec<u8>,
    pub digest: String,
    pub assets: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// The `InstalledApp` descriptor the client and module see.
    pub fn descriptor(&self) -> Value {
        let base = format!("/appAssets/{}/{}/", self.manifest.id, self.digest);
        json!({ "id": self.manifest.id, "name": self.manifest.name, "version": self.manifest.version, "digest": self.digest,
            "clientUrl": format!("{base}{}", self.manifest.client), "styles": self.manifest.styles.iter().map(|s| format!("{base}{s}")).collect::<Vec<_>>(),
            "permissions": self.manifest.permissions })
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::other(message.to_owned())
}

pub fn valid_app_id(id: &str) -> bool {
    if id.len() > 120 {
        return false;
    }
    let parts: Vec<&str> = id.split('.').collect();
    (3..=8).contains(&parts.len()) && parts.iter().all(|p| p.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
}

fn safe_file(name: &str) -> bool {
    if name.len() > 180 || name.split('/').any(|p| p == "." || p == "..") {
        return false;
    }
    let Some((stem, extension)) = name.rsplit_once('.') else { return false };
    if !["js", "css", "svg", "png", "webp", "json"].contains(&extension) {
        return false;
    }
    // [a-zA-Z0-9_-]+ separated by single / or .
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    !stem.is_empty() && stem.split(['/', '.']).all(|p| !p.is_empty() && p.chars().all(word))
}

/// Reads a regular file below `root`, refusing links on the way.
fn regular(root: &Path, name: &str, max: u64) -> io::Result<Vec<u8>> {
    let mut current = root.to_path_buf();
    for part in name.split('/') {
        current.push(part);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(invalid("Package symlinks are not allowed."));
        }
    }
    let m = fs::symlink_metadata(&current)?;
    if !m.is_file() || m.len() > max {
        return Err(invalid("Invalid package file."));
    }
    fs::read(current)
}

/// `readAppPackage`: only regular, declared files; their bytes fix the digest.
pub fn read_package(input: &PackageInput) -> io::Result<Package> {
    if !Path::new(&input.path).is_absolute() {
        return Err(invalid("Package paths must be absolute."));
    }
    let root = fs::canonicalize(&input.path)?;
    let raw = regular(&root, "dar-app.json", 32768)?;
    let m: Manifest = serde_json::from_slice(&raw).map_err(|_| invalid("Invalid app manifest or unsupported API."))?;
    let unique = |v: &[String]| v.iter().collect::<std::collections::HashSet<_>>().len() == v.len();
    let version_ok = m.version.split('.').count() == 3 && m.version.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let valid = valid_app_id(&m.id) && !m.name.trim().is_empty() && m.name.chars().count() <= 60 && version_ok && m.api_version == json!(1)
        && m.license.as_str().is_some_and(|l| l.chars().count() <= 100) && m.files.len() <= 200 && unique(&m.files) && m.files.iter().all(|f| safe_file(f))
        && m.files.contains(&m.client) && m.client.ends_with(".js") && m.styles.iter().all(|s| m.files.contains(s) && s.ends_with(".css"))
        && m.permissions.iter().all(|p| PERMISSIONS.contains(&p.as_str())) && unique(&m.permissions)
        && m.server.as_ref().is_none_or(|s| m.files.contains(s) && s.ends_with(".js"));
    if !valid {
        return Err(invalid("Invalid app manifest or unsupported API."));
    }
    let mut hash = Sha256::new();
    hash.update(&raw);
    let mut assets = BTreeMap::new();
    let mut size = 0usize;
    let mut names = m.files.clone();
    names.sort();
    for name in names {
        let bytes = regular(&root, &name, 8 * 1024 * 1024)?;
        size += bytes.len();
        if size > 32 * 1024 * 1024 {
            return Err(invalid("Package exceeds 32 MiB."));
        }
        hash.update(serde_json::to_string(&name).expect("name").as_bytes());
        hash.update(&bytes);
        assets.insert(name, bytes);
    }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if input.digest.as_ref().is_some_and(|d| *d != digest) {
        return Err(invalid("Package digest mismatch."));
    }
    Ok(Package { root, manifest: m, manifest_text: raw, digest, assets })
}

#[derive(Clone, Debug, Serialize)]
pub struct InventoryItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub permissions: Vec<String>,
}

#[derive(Default)]
pub struct Registry {
    /// Enabled, readable packages by app id.
    pub apps: HashMap<String, Package>,
    /// Every package, enabled or not, for the operator's list.
    pub packages: HashMap<String, Package>,
    pub inventory: Vec<InventoryItem>,
}

/// `loadApps`: a missing registry file is an empty registry.
pub fn load(filename: &Path) -> io::Result<Registry> {
    let mut registry = Registry::default();
    if std::env::var("DAR_DISABLE_APPS").as_deref() == Ok("1") {
        return Ok(registry);
    }
    let inputs: Vec<PackageInput> = serde_json::from_slice(&fs::read(filename)?).map_err(|_| invalid("Invalid registry."))?;
    if inputs.len() > 100 {
        return Err(invalid("App registry must contain up to 100 package entries."));
    }
    for input in inputs {
        match read_package(&input) {
            Ok(app) if !registry.inventory.iter().any(|i| i.id == app.manifest.id) => {
                registry.inventory.push(InventoryItem {
                    id: app.manifest.id.clone(),
                    name: app.manifest.name.clone(),
                    version: app.manifest.version.clone(),
                    status: if input.enabled == Some(false) { "Disabled" } else { "Ready" }.into(),
                    permissions: app.manifest.permissions.clone(),
                });
                registry.packages.insert(app.manifest.id.clone(), app.clone());
                if input.enabled != Some(false) {
                    registry.apps.insert(app.manifest.id.clone(), app);
                }
            }
            _ => registry.inventory.push(InventoryItem {
                id: format!("invalid-{}", registry.inventory.len() + 1),
                name: "Unavailable package".into(),
                version: String::new(),
                status: "Invalid package, duplicate ID or unsupported API. Check host configuration.".into(),
                permissions: Vec::new(),
            }),
        }
    }
    Ok(registry)
}

/// The registry at startup: a broken file leaves an empty registry with one warning row.
pub fn load_or_report(filename: &Path) -> Registry {
    match load(filename) {
        Ok(registry) => registry,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Registry::default(),
        Err(_) => {
            eprintln!("Installed app registry is unavailable; starting without external apps.");
            Registry { inventory: vec![InventoryItem { id: "registry".into(), name: "App registry".into(), version: String::new(), status: "Invalid registry. Check host configuration.".into(), permissions: Vec::new() }], ..Default::default() }
        }
    }
}

fn sync(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

/// Offline operator commands (`manageApps`); the root lock means Dar must be stopped.
pub fn manage(root: &Path, command: &str, argument: Option<&str>) -> io::Result<String> {
    let root = crate::root::open_root(root)?;
    let _lock = crate::root::RootLock::acquire(&root)?;
    let file = root.join("apps.json");
    let mut inputs: Vec<PackageInput> = match fs::symlink_metadata(&file) {
        Ok(m) => {
            use std::os::unix::fs::MetadataExt;
            if !m.is_file() || m.mode() & 0o077 != 0 {
                return Err(invalid("Unsafe app registry."));
            }
            serde_json::from_slice(&fs::read(&file)?).map_err(|_| invalid("Invalid registry."))?
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    match (command, argument) {
        ("apps-list", _) => {
            let mut list = Vec::new();
            for input in &inputs {
                let app = read_package(input)?;
                list.push(json!({ "id": app.manifest.id, "version": app.manifest.version, "enabled": input.enabled != Some(false), "digest": app.digest }));
            }
            return Ok(serde_json::to_string_pretty(&list).expect("list"));
        }
        ("apps-install", Some(source)) => {
            let source = std::path::absolute(source)?;
            let app = read_package(&PackageInput { path: source.to_string_lossy().into_owned(), digest: None, enabled: None })?;
            let packages = root.join("appPackages");
            match fs::create_dir(&packages) {
                Ok(()) => {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&packages, fs::Permissions::from_mode(0o700))?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            let m = fs::symlink_metadata(&packages)?;
            if !m.is_dir() || m.file_type().is_symlink() {
                return Err(invalid("Unsafe package directory."));
            }
            let destination = packages.join(&app.digest);
            let staging = packages.join(format!(".{}.tmp", crate::root::uuid()));
            let staged = (|| -> io::Result<()> {
                fs::create_dir(&staging)?;
                for (name, bytes) in &app.assets {
                    let target = staging.join(name);
                    fs::create_dir_all(target.parent().expect("parent"))?;
                    crate::root::create_durable_file(&target, bytes)?;
                }
                crate::root::create_durable_file(&staging.join("dar-app.json"), &app.manifest_text)?;
                read_package(&PackageInput { path: staging.to_string_lossy().into_owned(), digest: Some(app.digest.clone()), enabled: None })?;
                sync(&staging)?;
                match fs::rename(&staging, &destination) {
                    Ok(()) => {}
                    Err(_) if destination.is_dir() => {
                        read_package(&PackageInput { path: destination.to_string_lossy().into_owned(), digest: Some(app.digest.clone()), enabled: None })?;
                    }
                    Err(e) => return Err(e),
                }
                sync(&packages)
            })();
            let _ = fs::remove_dir_all(&staging);
            staged?;
            let mut retained = Vec::new();
            for input in inputs {
                if read_package(&input)?.manifest.id != app.manifest.id {
                    retained.push(input);
                }
            }
            retained.push(PackageInput { path: destination.to_string_lossy().into_owned(), digest: Some(app.digest.clone()), enabled: Some(true) });
            inputs = retained;
        }
        ("apps-enable" | "apps-disable" | "apps-uninstall", Some(id)) => {
            let mut found = false;
            let mut updated = Vec::new();
            for input in inputs {
                if read_package(&input)?.manifest.id != id {
                    updated.push(input);
                    continue;
                }
                found = true;
                if command != "apps-uninstall" {
                    updated.push(PackageInput { enabled: Some(command == "apps-enable"), ..input });
                }
            }
            if !found {
                return Err(invalid("App not found."));
            }
            inputs = updated;
        }
        _ => return Err(invalid("Usage: apps-list, apps-install <built-package-directory>, apps-enable|apps-disable|apps-uninstall <app-id>")),
    }
    crate::root::write_durable_file(&file, serde_json::to_string_pretty(&inputs).expect("registry").as_bytes())?;
    Ok("App configuration saved. Restart Dar to apply it. App data is retained.".into())
}

/// The content type of a served package file.
pub fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e) {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/json",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_files() {
        assert!(valid_app_id("org.dar.prayer-times"));
        assert!(!valid_app_id("org.dar"));
        assert!(!valid_app_id("Org.dar.x"));
        assert!(safe_file("assets/app.js") && safe_file("style.css") && safe_file("a-b_c/d.min.js"));
        assert!(!safe_file("../x.js") && !safe_file("x.exe") && !safe_file("a//b.js") && !safe_file(".js"));
    }

    #[test]
    fn install_list_disable_uninstall() {
        let base = std::env::temp_dir().join(format!("dar-apps-{}", crate::root::uuid()));
        let package = base.join("package");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("dar-app.json"), r#"{"id":"org.example.test","name":"Test","version":"1.0.0","apiVersion":1,"license":"MIT","client":"app.js","styles":["app.css"],"files":["app.js","app.css"],"permissions":["ownUserData:read"]}"#).unwrap();
        fs::write(package.join("app.js"), "export {}").unwrap();
        fs::write(package.join("app.css"), "").unwrap();
        let root = base.join("root");
        manage(&root, "apps-install", Some(package.to_str().unwrap())).unwrap();
        let registry = load(&root.join("apps.json")).unwrap();
        let app = &registry.apps["org.example.test"];
        assert_eq!(app.descriptor()["clientUrl"], format!("/appAssets/org.example.test/{}/app.js", app.digest));
        manage(&root, "apps-disable", Some("org.example.test")).unwrap();
        let registry = load(&root.join("apps.json")).unwrap();
        assert!(registry.apps.is_empty() && registry.inventory[0].status == "Disabled");
        assert!(manage(&root, "apps-list", None).unwrap().contains("\"enabled\": false"));
        manage(&root, "apps-uninstall", Some("org.example.test")).unwrap();
        assert!(load(&root.join("apps.json")).unwrap().inventory.is_empty());
        assert!(manage(&root, "apps-enable", Some("org.example.test")).is_err());
        fs::remove_dir_all(&base).unwrap();
    }
}
