//! Booster definitions: TOML files describing which sets a booster product
//! applies to and what goes in each slot. See `boosters/README.md`.

use crate::scryfall::SetInfo;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DIR_ENV: &str = "MTGBOOSTERSIM_BOOSTERS";
const DIR_NAME: &str = "boosters";

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// Product family, e.g. "collector". Among definitions sharing an id,
    /// only the most specific one that matches a set is offered.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Position among a set's booster choices; lower comes first.
    #[serde(default = "default_order")]
    pub order: i32,
    /// When an exclusive definition matches a set, non-exclusive ones are
    /// not offered for it.
    #[serde(default)]
    pub exclusive: bool,
    pub availability: Vec<Availability>,
    /// Chance that a `rare_mythic` slot holds a mythic. Default 1 in 8.
    pub mythic_odds: Option<f64>,
    /// Common / uncommon / rare / mythic weights for `wildcard` slots.
    pub wildcard_weights: Option<[f64; 4]>,
    pub theme: Option<Theme>,
    /// Number of cards, picked at random after the pack is built, that
    /// become foil.
    #[serde(default)]
    pub random_foils: usize,
    pub slots: Vec<Slot>,
    #[serde(skip)]
    pub file: String,
}

fn default_order() -> i32 {
    100
}

fn one() -> usize {
    1
}

fn yes() -> bool {
    true
}

/// One rule saying which sets a definition covers. A set must satisfy every
/// field that is present.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Availability {
    /// Informational: the first and last sets sold in this product.
    pub first_set: Option<String>,
    pub last_set: Option<String>,
    /// Release date range, `from` inclusive and `before` exclusive.
    pub from: Option<String>,
    pub before: Option<String>,
    /// Scryfall set types, e.g. "expansion", "core", "masters".
    pub set_types: Option<Vec<String>>,
    /// Explicit set codes. A rule naming a set outranks date-based rules.
    pub sets: Option<Vec<String>>,
    pub name_contains: Option<String>,
}

impl Availability {
    /// 0 for no match, 1 for a general match, 2 when the set is named.
    fn specificity(&self, set: &SetInfo) -> u8 {
        let released = set.released_at.as_deref().unwrap_or("");
        let ok = self.from.as_deref().is_none_or(|d| released >= d)
            && self.before.as_deref().is_none_or(|d| released < d)
            && self.set_types.as_ref().is_none_or(|t| t.contains(&set.set_type))
            && self.sets.as_ref().is_none_or(|s| s.contains(&set.code))
            && self.name_contains.as_deref().is_none_or(|n| set.name.contains(n));
        match (ok, &self.sets) {
            (false, _) => 0,
            (true, Some(_)) => 2,
            (true, None) => 1,
        }
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Every card in the pack shares one color.
    Color,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Common,
    Uncommon,
    Rare,
    Mythic,
    RareMythic,
    CommonUncommon,
    /// Any rarity, weighted by `wildcard_weights`.
    Wildcard,
    BasicLand,
    /// A full-art or special-treatment basic land.
    SpecialLand,
    Token,
    ArtCard,
    /// A Special Guests card released with the set.
    SpecialGuest,
    /// A random card from The List.
    List,
    /// No card: shows the slot's `placeholder` text (ad cards and the like).
    Placeholder,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Treatment {
    /// The ordinary booster printing.
    #[default]
    Regular,
    /// Any showcase, borderless or extended-art printing.
    Any,
    Extended,
    /// Showcase or borderless, but not extended-art.
    Showcase,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub label: String,
    pub source: Source,
    #[serde(default = "one")]
    pub count: usize,
    /// Random count instead of `count`: `{ 1 = 15, 2 = 45 }` means one card
    /// with weight 15, two with weight 45.
    pub count_weights: Option<BTreeMap<String, u32>>,
    /// Count is this number minus the count of the slot before it.
    pub complement_of_previous: Option<usize>,
    #[serde(default)]
    pub foil: bool,
    /// Chance the slot is foil, when `foil` is not set.
    #[serde(default)]
    pub foil_odds: f64,
    #[serde(default)]
    pub treatment: Treatment,
    /// Chance a regular card is swapped for a special printing of itself.
    #[serde(default)]
    pub treatment_odds: f64,
    /// Shown when the source has no card to offer.
    pub placeholder: Option<String>,
    /// All cards in the slot are the same card.
    #[serde(default)]
    pub identical: bool,
    /// Whether the definition's `random_foils` may land on this slot.
    #[serde(default = "yes")]
    pub random_foil_eligible: bool,
    /// Alternatives that may take the place of one card in this slot.
    #[serde(default)]
    pub replace: Vec<Replace>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    pub odds: f64,
    /// Label for the replacement card; defaults to the slot's label.
    pub label: Option<String>,
    pub source: Source,
    #[serde(default)]
    pub treatment: Treatment,
    #[serde(default)]
    pub foil: bool,
    pub placeholder: Option<String>,
    /// Only applies to sets released before this date.
    pub set_before: Option<String>,
}

impl Definition {
    pub fn uses(&self, source: Source) -> bool {
        self.slots
            .iter()
            .any(|s| s.source == source || s.replace.iter().any(|r| r.source == source))
    }

    /// One line describing which sets the definition covers.
    pub fn availability_text(&self) -> String {
        self.availability
            .iter()
            .filter_map(|a| match (&a.first_set, &a.last_set) {
                (Some(first), Some(last)) => Some(format!("{first} – {last}")),
                (Some(first), None) => Some(format!("{first} onward")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn validate(&self) -> Result<(), String> {
        let odds = |what: &str, p: f64| {
            (0.0..=1.0).contains(&p).then_some(()).ok_or(format!("{what} must be between 0 and 1"))
        };
        if self.slots.is_empty() {
            return Err("has no slots".into());
        }
        if self.availability.is_empty() {
            return Err("has no [[availability]] rule".into());
        }
        odds("mythic_odds", self.mythic_odds.unwrap_or(0.0))?;
        for slot in &self.slots {
            let at = |msg: &str| format!("slot \"{}\": {msg}", slot.label);
            odds("foil_odds", slot.foil_odds).map_err(|e| at(&e))?;
            odds("treatment_odds", slot.treatment_odds).map_err(|e| at(&e))?;
            odds("the sum of replace odds", slot.replace.iter().map(|r| r.odds).sum())
                .map_err(|e| at(&e))?;
            if let Some(weights) = &slot.count_weights {
                if weights.keys().any(|k| k.parse::<usize>().is_err()) {
                    return Err(at("count_weights keys must be whole numbers"));
                }
                if weights.values().sum::<u32>() == 0 {
                    return Err(at("count_weights must have a positive weight"));
                }
            }
            let placeholder_missing = |source, text: &Option<String>| {
                source == Source::Placeholder && text.is_none()
            };
            if placeholder_missing(slot.source, &slot.placeholder)
                || slot.replace.iter().any(|r| placeholder_missing(r.source, &r.placeholder))
            {
                return Err(at("a placeholder source needs placeholder text"));
            }
        }
        Ok(())
    }
}

fn parse(file: &str, text: &str) -> Result<Definition, String> {
    let mut def: Definition = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
    def.file = file.to_string();
    def.validate().map_err(|e| format!("{file}: {e}"))?;
    Ok(def)
}

/// The directory definitions are read from: the path in
/// `MTGBOOSTERSIM_BOOSTERS`, else `boosters/` in the working directory or
/// beside the executable.
fn definitions_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    let beside_exe = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join(DIR_NAME)));
    [Some(PathBuf::from(DIR_NAME)), beside_exe].into_iter().flatten().find(|p| p.is_dir())
}

/// Load every definition from the definitions directory.
pub fn load() -> Result<Vec<Definition>, String> {
    let dir = definitions_dir().ok_or_else(|| {
        format!(
            "No booster definitions found. Run the app from the folder containing `{DIR_NAME}/`, \
             put `{DIR_NAME}/` beside the executable, or set {DIR_ENV} to its path."
        )
    })?;
    let defs = load_from(&dir)?;
    log::info!("Loaded {} booster definitions from {}", defs.len(), dir.display());
    Ok(defs)
}

pub fn load_from(dir: &Path) -> Result<Vec<Definition>, String> {
    let read_err = |e: std::io::Error| format!("{}: {e}", dir.display());
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(read_err)?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{}: no .toml booster definitions found", dir.display()));
    }
    paths
        .iter()
        .map(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            let text = std::fs::read_to_string(p).map_err(|e| format!("{name}: {e}"))?;
            let def = parse(&name, &text)?;
            log::debug!("Definition {name}: id \"{}\", \"{}\", {} slots", def.id, def.name, def.slots.len());
            Ok(def)
        })
        .collect()
}

/// The definitions shipped in the repository, for tests.
#[cfg(test)]
pub fn repo_definitions() -> Vec<Definition> {
    load_from(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/boosters")))
        .expect("the repository's definitions are valid")
}

/// The boosters offered for a set, in display order.
pub fn options_for<'a>(defs: &'a [Definition], set: &SetInfo) -> Vec<&'a Definition> {
    let mut matched: Vec<(u8, &Definition)> = defs
        .iter()
        .filter_map(|d| {
            let best = d.availability.iter().map(|a| a.specificity(set)).max()?;
            (best > 0).then_some((best, d))
        })
        .collect();
    if matched.iter().any(|(_, d)| d.exclusive) {
        matched.retain(|(_, d)| d.exclusive);
    }
    let mut options: Vec<(u8, &Definition)> = Vec::new();
    for (score, def) in matched {
        match options.iter_mut().find(|(_, d)| d.id == def.id) {
            Some(existing) if score > existing.0 => *existing = (score, def),
            Some(_) => {}
            None => options.push((score, def)),
        }
    }
    options.sort_by_key(|(_, d)| d.order);
    let options: Vec<&Definition> = options.into_iter().map(|(_, d)| d).collect();
    log::debug!(
        "Boosters for {} ({}): {}",
        set.name,
        set.code,
        options.iter().map(|d| d.file.as_str()).collect::<Vec<_>>().join(", ")
    );
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        id = "x"
        name = "X"
        [[availability]]
        [[slots]]
        label = "Common"
        source = "common"
    "#;

    #[test]
    fn built_in_definitions_load() {
        assert!(repo_definitions().len() >= 10);
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(parse("x.toml", VALID).is_ok());
        let misspelled = VALID.replace("source = \"common\"", "source = \"common\"\nfiol = true");
        assert!(parse("x.toml", &misspelled).unwrap_err().contains("fiol"));
        let bad_odds = format!("{VALID}foil_odds = 2.0\n");
        assert!(parse("x.toml", &bad_odds).unwrap_err().contains("foil_odds"));
        let bad_source = VALID.replace("\"common\"", "\"comon\"");
        assert!(parse("x.toml", &bad_source).is_err());
    }
}
