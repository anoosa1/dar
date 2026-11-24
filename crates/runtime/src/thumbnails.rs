//! Image thumbnails for Files tiles and previews (`runtime/photos/media.ts`): auto-rotated
//! WebP from `vipsthumbnail`, at most 420 px (`s`) or 1800 px (`m`) on the long edge, cached
//! per blob. Without media tools the caller sends the original, as the Node runtime does.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Semaphore;

#[allow(dead_code)]
pub struct Thumbnails {
    cache: PathBuf,
    /// Image conversions are CPU-heavy; two run at once.
    slots: Semaphore,
}

#[allow(dead_code)]
impl Thumbnails {
    pub fn new(root: &Path) -> Thumbnails {
        Thumbnails {
            cache: root.join("cache").join("thumbnails"),
            slots: Semaphore::new(2),
        }
    }

    fn path(&self, blob_id: &str, size: &str) -> PathBuf {
        self.cache
            .join(&blob_id[..2])
            .join(format!("{blob_id}-{size}.webp"))
    }

    /// The thumbnail file for a stored image, made on first use; `None` if it cannot be made.
    pub async fn file(&self, source: &Path, blob_id: &str, size: &str) -> Option<PathBuf> {
        let (edge, quality) = if size == "m" { (1800, 82) } else { (420, 72) };
        let target = self.path(blob_id, size);
        if tokio::fs::metadata(&target)
            .await
            .is_ok_and(|m| m.len() > 0)
        {
            return Some(target);
        }
        let _slot = self.slots.acquire().await.ok()?;
        if tokio::fs::metadata(&target)
            .await
            .is_ok_and(|m| m.len() > 0)
        {
            return Some(target);
        }
        tokio::fs::create_dir_all(target.parent()?).await.ok()?;
        let work = self.cache.join(format!(".work-{}", crate::auth::uuid()));
        tokio::fs::create_dir_all(&work).await.ok()?;
        let out = work.join("out.webp");
        let status = Command::new("vipsthumbnail")
            .arg(source)
            .args(["--size", &format!("{edge}x{edge}>"), "-o"])
            .arg(format!("{}[Q={quality},strip]", out.display()))
            .kill_on_drop(true)
            .status();
        let made = matches!(tokio::time::timeout(Duration::from_secs(120), status).await, Ok(Ok(s)) if s.success());
        let result = if made && tokio::fs::metadata(&out).await.is_ok_and(|m| m.len() > 0) {
            tokio::fs::rename(&out, &target).await.ok().map(|_| target)
        } else {
            None
        };
        let _ = tokio::fs::remove_dir_all(&work).await;
        result
    }
}
