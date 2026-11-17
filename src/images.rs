//! Loads card images for the window, keeping them in the disk cache so each
//! image is downloaded from Scryfall only once.

use crate::cache::{self, Cached};
use eframe::egui::{
    self,
    load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError},
};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

const CACHE_KIND: &str = "images";
const USER_AGENT: &str = concat!("mtgboostersim/", env!("CARGO_PKG_VERSION"));
/// Larger than any card image; guards against a runaway download.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

type Entry = Poll<Result<Arc<[u8]>, String>>;

#[derive(Default)]
pub struct CachedImageLoader {
    loaded: Arc<Mutex<HashMap<String, Entry>>>,
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response.into_reader().take(MAX_BYTES).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn fetch(url: &str) -> Result<Arc<[u8]>, String> {
    // An image's URL changes when the image does, so cached copies never
    // go out of date.
    if let Cached::Fresh(bytes) | Cached::Stale(bytes) = cache::read(CACHE_KIND, url, None) {
        log::debug!("Image from cache: {url}");
        return Ok(bytes.into());
    }
    let started = std::time::Instant::now();
    match download(url) {
        Ok(bytes) => {
            log::debug!("Image downloaded in {} ms: {url}", started.elapsed().as_millis());
            cache::write(CACHE_KIND, url, &bytes);
            Ok(bytes.into())
        }
        Err(e) => {
            log::warn!("Could not download image {url}: {e}");
            Err(format!("Could not load image: {e}"))
        }
    }
}

impl BytesLoader for CachedImageLoader {
    fn id(&self) -> &str {
        egui::generate_loader_id!(CachedImageLoader)
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !uri.starts_with("https://") && !uri.starts_with("http://") {
            return Err(LoadError::NotSupported);
        }
        let mut loaded = self.loaded.lock().unwrap();
        match loaded.get(uri) {
            Some(Poll::Ready(Ok(bytes))) => {
                Ok(BytesPoll::Ready { size: None, bytes: Bytes::Shared(bytes.clone()), mime: None })
            }
            Some(Poll::Ready(Err(e))) => Err(LoadError::Loading(e.clone())),
            Some(Poll::Pending) => Ok(BytesPoll::Pending { size: None }),
            None => {
                loaded.insert(uri.to_string(), Poll::Pending);
                let (uri, ctx, loaded) = (uri.to_string(), ctx.clone(), self.loaded.clone());
                std::thread::spawn(move || {
                    let result = fetch(&uri);
                    // Skip the result if the image was forgotten meanwhile.
                    if let Some(entry) = loaded.lock().unwrap().get_mut(&uri) {
                        *entry = Poll::Ready(result);
                    }
                    ctx.request_repaint();
                });
                Ok(BytesPoll::Pending { size: None })
            }
        }
    }

    fn forget(&self, uri: &str) {
        self.loaded.lock().unwrap().remove(uri);
    }

    fn forget_all(&self) {
        self.loaded.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.loaded
            .lock()
            .unwrap()
            .values()
            .map(|entry| match entry {
                Poll::Ready(Ok(bytes)) => bytes.len(),
                Poll::Ready(Err(e)) => e.len(),
                Poll::Pending => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.loaded.lock().unwrap().values().any(Poll::is_pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads a real image; run with `cargo test -- --ignored` and
    /// `MTGBOOSTERSIM_CACHE` pointing at a scratch folder.
    #[test]
    #[ignore = "needs the network"]
    fn images_are_downloaded_once() {
        let url = "https://cards.scryfall.io/normal/front/b/d/bd8fa327-dd41-4737-8f19-2cf5eb1f7cdd.jpg";
        assert!(matches!(cache::read(CACHE_KIND, url, None), Cached::Missing));
        let first = fetch(url).unwrap();
        assert!(first.starts_with(&[0xFF, 0xD8]), "a JPEG");
        assert!(matches!(cache::read(CACHE_KIND, url, None), Cached::Fresh(b) if b[..] == first[..]));
        assert_eq!(fetch(url).unwrap()[..], first[..]);
    }
}
