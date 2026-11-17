//! A disk cache for what the app downloads from Scryfall: API responses and
//! card images, one file per URL.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DIR_ENV: &str = "MTGBOOSTERSIM_CACHE";
const APP_DIR: &str = "mtgboostersim";
/// Longest readable part of a cache file's name; a hash keeps names unique.
const NAME_LENGTH: usize = 120;

static REFRESH: AtomicBool = AtomicBool::new(false);

/// Ignore cached API data for this run and download everything again. What
/// is downloaded still replaces the cached copies.
pub fn set_refresh(refresh: bool) {
    REFRESH.store(refresh, Ordering::Relaxed);
}

/// The cache directory: `MTGBOOSTERSIM_CACHE`, else the platform's usual
/// place for caches.
pub fn dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var(DIR_ENV) {
        return Some(dir);
    }
    let base = if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA")
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    Some(base?.join(APP_DIR))
}

pub enum Cached {
    /// Present and younger than the requested age.
    Fresh(Vec<u8>),
    /// Present but too old, or a refresh was asked for. Still usable when
    /// the download fails.
    Stale(Vec<u8>),
    Missing,
}

/// FNV-1a, so file names stay the same from one build to the next.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn file_in(base: &Path, kind: &str, url: &str) -> PathBuf {
    let readable: String = url
        .trim_start_matches("https://")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .take(NAME_LENGTH)
        .collect();
    base.join(kind).join(format!("{readable}_{:016x}", hash(url)))
}

fn read_in(base: &Path, kind: &str, url: &str, max_age: Option<Duration>) -> Cached {
    let path = file_in(base, kind, url);
    let Ok(bytes) = std::fs::read(&path) else { return Cached::Missing };
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    match (max_age, age) {
        (None, _) => Cached::Fresh(bytes),
        (Some(max), Some(age)) if age <= max => Cached::Fresh(bytes),
        _ => Cached::Stale(bytes),
    }
}

fn write_in(base: &Path, kind: &str, url: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = file_in(base, kind, url);
    std::fs::create_dir_all(path.parent().unwrap_or(base))?;
    // Write beside the target and rename, so a crash never leaves half a file.
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, &path)
}

/// Look a URL up. `kind` names a subfolder ("api", "images"); `max_age` of
/// `None` means the data never goes out of date.
pub fn read(kind: &str, url: &str, max_age: Option<Duration>) -> Cached {
    let Some(base) = dir() else { return Cached::Missing };
    match read_in(&base, kind, url, max_age) {
        Cached::Fresh(bytes) if max_age.is_some() && REFRESH.load(Ordering::Relaxed) => {
            Cached::Stale(bytes)
        }
        other => other,
    }
}

pub fn write(kind: &str, url: &str, bytes: &[u8]) {
    let Some(base) = dir() else { return };
    if let Err(e) = write_in(&base, kind, url, bytes) {
        log::warn!("Could not cache {url} in {}: {e}", base.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mtgboostersim-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_and_expiry() {
        let base = temp_dir("cache");
        let url = "https://api.scryfall.com/cards/search?q=e%3Amkm&unique=prints";
        assert!(matches!(read_in(&base, "api", url, None), Cached::Missing));

        write_in(&base, "api", url, b"hello").unwrap();
        assert!(matches!(read_in(&base, "api", url, None), Cached::Fresh(b) if b == b"hello"));
        let day = Duration::from_secs(86_400);
        assert!(matches!(read_in(&base, "api", url, Some(day)), Cached::Fresh(_)));
        assert!(matches!(read_in(&base, "api", url, Some(Duration::ZERO)), Cached::Stale(b) if b == b"hello"));
        // Kinds and URLs do not collide.
        assert!(matches!(read_in(&base, "images", url, None), Cached::Missing));
        assert!(matches!(read_in(&base, "api", &format!("{url}&page=2"), None), Cached::Missing));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn long_urls_get_distinct_short_names() {
        let base = Path::new("/cache");
        let long = format!("https://example.com/{}", "a".repeat(500));
        let a = file_in(base, "api", &format!("{long}1"));
        let b = file_in(base, "api", &format!("{long}2"));
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().len() < 150);
    }
}
