//! Minimal blocking client for the Scryfall API (https://scryfall.com/docs/api).

use crate::cache::{self, Cached};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const API: &str = "https://api.scryfall.com";
const USER_AGENT: &str = concat!("mtgboostersim/", env!("CARGO_PKG_VERSION"));
/// Scryfall asks clients to stay under ten requests per second.
const MIN_GAP: Duration = Duration::from_millis(110);
const RATE_LIMIT_RETRIES: u32 = 3;
const RATE_LIMIT_MAX_WAIT_SECS: u64 = 30;

const CACHE_KIND: &str = "api";
/// How long cached answers are reused. The list of sets changes when a set
/// is announced; a set's cards change only in their prices.
const KEEP_SETS: Duration = Duration::from_secs(24 * 60 * 60);
const KEEP_CARDS: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Stored in the cache for a request Scryfall answered with "no results".
const NOT_FOUND: &[u8] = b"not found";

static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);
static CACHE_HITS: AtomicUsize = AtomicUsize::new(0);
static DOWNLOADS: AtomicUsize = AtomicUsize::new(0);

#[derive(Deserialize, Clone, Debug)]
pub struct SetInfo {
    pub code: String,
    pub name: String,
    pub set_type: String,
    pub released_at: Option<String>,
    pub parent_set_code: Option<String>,
    #[serde(default)]
    pub digital: bool,
    #[serde(default)]
    pub card_count: u32,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct ImageUris {
    pub normal: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Face {
    pub image_uris: Option<ImageUris>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Prices {
    pub usd: Option<String>,
    pub usd_foil: Option<String>,
    pub usd_etched: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Card {
    #[serde(default)]
    pub id: String,
    pub oracle_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub rarity: String,
    pub type_line: Option<String>,
    #[serde(default)]
    pub set: String,
    #[serde(default)]
    pub collector_number: String,
    #[serde(default)]
    pub layout: String,
    pub image_uris: Option<ImageUris>,
    pub card_faces: Option<Vec<Face>>,
    /// True when the printing appears in the set's ordinary boosters.
    #[serde(default)]
    pub booster: bool,
    #[serde(default)]
    pub full_art: bool,
    #[serde(default)]
    pub frame_effects: Vec<String>,
    #[serde(default)]
    pub promo_types: Vec<String>,
    pub border_color: Option<String>,
    #[serde(default)]
    pub color_identity: Vec<String>,
    #[serde(default)]
    pub finishes: Vec<String>,
    pub prices: Option<Prices>,
    /// The card's page on scryfall.com.
    pub scryfall_uri: Option<String>,
}

impl Card {
    /// Front-face image URL, also for double-faced cards and art cards.
    pub fn image_url(&self) -> Option<&str> {
        self.image_uris
            .as_ref()
            .and_then(|i| i.normal.as_deref())
            .or_else(|| {
                self.card_faces
                    .as_ref()?
                    .first()?
                    .image_uris
                    .as_ref()?
                    .normal
                    .as_deref()
            })
    }

    pub fn is_token(&self) -> bool {
        matches!(self.layout.as_str(), "token" | "double_faced_token" | "emblem")
    }

    pub fn is_art_card(&self) -> bool {
        self.layout == "art_series"
    }

    pub fn is_basic_land(&self) -> bool {
        self.type_line
            .as_deref()
            .is_some_and(|t| t.starts_with("Basic") && t.contains("Land"))
    }

    pub fn has_finish(&self, finish: &str) -> bool {
        self.finishes.iter().any(|f| f == finish)
    }

    pub fn has_promo(&self, promo: &str) -> bool {
        self.promo_types.iter().any(|p| p == promo)
    }

    /// Showcase, borderless, extended-art and similar "Booster Fun" printings.
    pub fn is_special_treatment(&self) -> bool {
        !self.has_promo("serialized")
            && (self.has_promo("boosterfun")
                || self.border_color.as_deref() == Some("borderless")
                || self
                    .frame_effects
                    .iter()
                    .any(|f| f == "showcase" || f == "extendedart"))
    }
}

#[derive(Deserialize)]
struct List<T> {
    data: Vec<T>,
    #[serde(default)]
    has_more: bool,
    next_page: Option<String>,
}

/// Wait until the next request is allowed under the rate limit.
fn pace() {
    let mut last = LAST_REQUEST.lock().unwrap();
    if let Some(t) = *last {
        let since = t.elapsed();
        if since < MIN_GAP {
            std::thread::sleep(MIN_GAP - since);
        }
    }
    *last = Some(Instant::now());
}

/// How many API answers came from the disk cache and from the network.
pub fn request_counts() -> (usize, usize) {
    (CACHE_HITS.load(Ordering::Relaxed), DOWNLOADS.load(Ordering::Relaxed))
}

/// Fetch a response body from Scryfall. A 404, which Scryfall uses for "no
/// results", yields the `NOT_FOUND` marker; a 429 (too many requests) is
/// waited out and retried.
fn download(req: &ureq::Request) -> Result<Vec<u8>, String> {
    let url = req.url().to_string();
    for attempt in 0..=RATE_LIMIT_RETRIES {
        pace();
        let started = Instant::now();
        match req.clone().call() {
            Ok(response) => {
                let mut body = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut body)
                    .map_err(|e| format!("Could not read Scryfall's response: {e}"))?;
                DOWNLOADS.fetch_add(1, Ordering::Relaxed);
                log::debug!(
                    "GET {url}: {} KB in {} ms",
                    body.len() / 1024,
                    started.elapsed().as_millis()
                );
                return Ok(body);
            }
            Err(ureq::Error::Status(404, _)) => {
                DOWNLOADS.fetch_add(1, Ordering::Relaxed);
                log::debug!("GET {url}: no results (404)");
                return Ok(NOT_FOUND.to_vec());
            }
            Err(ureq::Error::Status(429, r)) if attempt < RATE_LIMIT_RETRIES => {
                let wait = r.header("Retry-After").and_then(|s| s.parse::<u64>().ok());
                let wait = wait.unwrap_or(2 << attempt).min(RATE_LIMIT_MAX_WAIT_SECS);
                log::warn!("Scryfall is limiting requests; waiting {wait} s before retrying {url}");
                std::thread::sleep(Duration::from_secs(wait));
            }
            Err(ureq::Error::Status(429, _)) => {
                return Err("Scryfall is limiting requests right now. Try again in a minute.".into());
            }
            Err(ureq::Error::Status(code, _)) => return Err(format!("Scryfall returned HTTP {code}")),
            Err(e) => return Err(format!("Could not reach Scryfall: {e}")),
        }
    }
    unreachable!("the last attempt always returns")
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<Option<T>, String> {
    if body == NOT_FOUND {
        return Ok(None);
    }
    serde_json::from_slice(body)
        .map(Some)
        .map_err(|e| format!("Unexpected response from Scryfall: {e}"))
}

/// Get and decode a Scryfall resource, `Ok(None)` meaning "no results".
/// With `keep_for`, the answer is served from the disk cache while it is
/// younger than that, and an older copy is used if Scryfall is unreachable.
fn get<T: DeserializeOwned>(req: ureq::Request, keep_for: Option<Duration>) -> Result<Option<T>, String> {
    let req = req
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(30));
    let url = req.url().to_string();

    let mut stale = None;
    if keep_for.is_some() {
        match cache::read(CACHE_KIND, &url, keep_for) {
            Cached::Fresh(body) => match decode(&body) {
                Ok(value) => {
                    CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                    log::debug!("From cache: {url}");
                    return Ok(value);
                }
                Err(e) => log::warn!("Ignoring unreadable cached copy of {url}: {e}"),
            },
            Cached::Stale(body) => stale = Some(body),
            Cached::Missing => {}
        }
    }

    match download(&req) {
        Ok(body) => {
            let value = decode(&body)?;
            if keep_for.is_some() {
                cache::write(CACHE_KIND, &url, &body);
            }
            Ok(value)
        }
        Err(e) => match stale.map(|body| decode(&body)) {
            Some(Ok(value)) => {
                log::warn!("{e} Using the older cached copy of {url}.");
                Ok(value)
            }
            _ => {
                log::error!("GET {url} failed: {e}");
                Err(e)
            }
        },
    }
}

pub fn fetch_sets() -> Result<Vec<SetInfo>, String> {
    let list: Option<List<SetInfo>> = get(ureq::get(&format!("{API}/sets")), Some(KEEP_SETS))?;
    Ok(list.map(|l| l.data).unwrap_or_default())
}

/// Every printing matching a Scryfall search query, following pagination.
pub fn search(query: &str) -> Result<Vec<Card>, String> {
    let mut cards = Vec::new();
    let first = ureq::get(&format!("{API}/cards/search"))
        .query("q", query)
        .query("unique", "prints")
        .query("order", "set");
    let mut page: Option<List<Card>> = get(first, Some(KEEP_CARDS))?;
    while let Some(p) = page {
        cards.extend(p.data);
        page = match (p.has_more, p.next_page) {
            (true, Some(url)) => get(ureq::get(&url), Some(KEEP_CARDS))?,
            _ => None,
        };
    }
    log::debug!("Search \"{query}\": {} cards", cards.len());
    Ok(cards)
}

/// One random card matching a query. Never cached.
pub fn random(query: &str) -> Result<Option<Card>, String> {
    get(ureq::get(&format!("{API}/cards/random")).query("q", query), None)
}

/// A specific printing by set code and collector number.
pub fn card_by_number(set: &str, number: &str) -> Result<Option<Card>, String> {
    get(ureq::get(&format!("{API}/cards/{set}/{number}")), Some(KEEP_CARDS))
}

/// A card by exact name, optionally restricted to one set.
pub fn card_named(name: &str, set: Option<&str>) -> Result<Option<Card>, String> {
    let mut req = ureq::get(&format!("{API}/cards/named")).query("exact", name);
    if let Some(set) = set {
        req = req.query("set", set);
    }
    get(req, Some(KEEP_CARDS))
}
