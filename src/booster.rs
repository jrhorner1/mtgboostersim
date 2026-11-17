//! Booster pack generation, driven by the definition files in `boosters/`.

use crate::definition::{Definition, Slot, Source, Theme, Treatment};
use crate::scryfall::{self, Card, SetInfo};

const DEFAULT_MYTHIC_ODDS: f64 = 1.0 / 8.0;
/// Common / uncommon / rare / mythic weights for wildcard slots.
const DEFAULT_WILDCARD_WEIGHTS: [f64; 4] = [0.55, 0.30, 0.125, 0.025];
/// A color needs this many cards to serve as a pack theme.
const THEME_MIN_CARDS: usize = 12;
const COLORS: [&str; 5] = ["W", "U", "B", "R", "G"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Finish {
    Nonfoil,
    Foil,
    Etched,
}

#[derive(Clone, Debug)]
pub enum Item {
    Card(Card),
    /// A slot Scryfall has no card data for (ad cards, missing art cards).
    Placeholder(String),
}

#[derive(Clone, Debug)]
pub struct PackCard {
    pub slot: String,
    pub item: Item,
    pub finish: Finish,
}

impl PackCard {
    pub fn name(&self) -> &str {
        match &self.item {
            Item::Card(c) => &c.name,
            Item::Placeholder(p) => p,
        }
    }

    /// Market price in USD for this card in this finish, if Scryfall has one.
    pub fn price(&self) -> Option<f64> {
        let Item::Card(c) = &self.item else { return None };
        let p = c.prices.as_ref()?;
        match self.finish {
            Finish::Nonfoil => p.usd.as_ref(),
            Finish::Foil => p.usd_foil.as_ref(),
            Finish::Etched => p.usd_etched.as_ref(),
        }?
        .parse()
        .ok()
    }
}

#[derive(Clone, Debug)]
pub struct Pack {
    pub booster_name: String,
    pub set_name: String,
    pub cards: Vec<PackCard>,
}

impl Pack {
    pub fn total_price(&self) -> f64 {
        self.cards.iter().filter_map(PackCard::price).sum()
    }
}

/// Everything needed to open boosters of one set.
#[derive(Debug)]
pub struct SetData {
    pub info: SetInfo,
    /// Every printing in the main set, including special treatments.
    pub cards: Vec<Card>,
    pub tokens: Vec<Card>,
    pub art: Vec<Card>,
    /// Special Guests cards released alongside this set.
    pub guests: Vec<Card>,
}

/// Fetch a set's cards. `with_guests` also fetches its Special Guests cards,
/// for sets with a booster that can contain them.
pub fn load_set(info: &SetInfo, all_sets: &[SetInfo], with_guests: bool) -> Result<SetData, String> {
    log::info!("Loading card data for {} ({})", info.name, info.code);
    let (cached_before, downloaded_before) = scryfall::request_counts();
    let cards = scryfall::search(&format!("e:{}", info.code))?;
    if cards.is_empty() {
        return Err(format!("Scryfall has no cards for {} yet.", info.name));
    }
    let children = || {
        all_sets
            .iter()
            .filter(|s| s.parent_set_code.as_deref() == Some(info.code.as_str()))
    };
    let token_code = format!("t{}", info.code);
    let token_set = children()
        .filter(|s| s.set_type == "token")
        .max_by_key(|s| s.code == token_code);
    let art_set = children().find(|s| s.name.ends_with("Art Series"));

    let tokens = match token_set {
        Some(s) => scryfall::search(&format!("e:{}", s.code))?,
        None => Vec::new(),
    };
    let art = match art_set {
        Some(s) => scryfall::search(&format!("e:{}", s.code))?,
        None => Vec::new(),
    };
    let guests = if with_guests {
        scryfall::search(&format!("e:spg date={}", info.code))?
    } else {
        Vec::new()
    };
    let (cached, downloaded) = scryfall::request_counts();
    log::info!(
        "{}: {} printings, {} tokens, {} art cards, {} Special Guests ({} requests downloaded, {} from cache)",
        info.code,
        cards.len(),
        tokens.len(),
        art.len(),
        guests.len(),
        downloaded - downloaded_before,
        cached - cached_before
    );
    log::debug!(
        "{}: token set {:?}, art set {:?}",
        info.code,
        token_set.map(|s| &s.code),
        art_set.map(|s| &s.code)
    );
    Ok(SetData { info: info.clone(), cards, tokens, art, guests })
}

fn tier_of(card: &Card) -> usize {
    match card.rarity.as_str() {
        "common" => 0,
        "uncommon" => 1,
        "mythic" => 3,
        _ => 2, // rare, plus the odd "special" and "bonus" rarities
    }
}

fn is_extended(card: &Card) -> bool {
    card.frame_effects.iter().any(|f| f == "extendedart")
}

fn random_of<'a>(list: &[&'a Card]) -> Option<&'a Card> {
    (!list.is_empty()).then(|| list[rand::random_range(0..list.len())])
}

/// A set's cards sorted into the groups slots draw from.
struct Pool<'a> {
    /// Regular booster printings by rarity: common, uncommon, rare, mythic.
    base: [Vec<&'a Card>; 4],
    /// Showcase / borderless / extended-art printings by rarity.
    treated: [Vec<&'a Card>; 4],
    /// Basic lands found in ordinary boosters.
    basics: Vec<&'a Card>,
    all_basics: Vec<&'a Card>,
    /// Full-art and special-treatment basic lands.
    special_lands: Vec<&'a Card>,
}

impl<'a> Pool<'a> {
    fn new(set: &'a SetData) -> Self {
        // Some sets have no printings flagged as booster cards; treat every
        // regular printing as one in that case.
        let any_flagged = set.cards.iter().any(|c| c.booster);
        let mut pool = Pool {
            base: Default::default(),
            treated: Default::default(),
            basics: vec![],
            all_basics: vec![],
            special_lands: vec![],
        };
        for c in &set.cards {
            let in_booster = if any_flagged { c.booster } else { !c.is_special_treatment() };
            if c.is_basic_land() {
                pool.all_basics.push(c);
                if c.full_art || c.is_special_treatment() {
                    pool.special_lands.push(c);
                }
                if in_booster {
                    pool.basics.push(c);
                }
                continue;
            }
            if c.is_special_treatment() {
                pool.treated[tier_of(c)].push(c);
            }
            if in_booster {
                pool.base[tier_of(c)].push(c);
            }
        }
        pool
    }

    /// Restrict the pool to one color, chosen among those with enough cards.
    fn apply_color_theme(&mut self) {
        let mono = |c: &Card, color: &str| c.color_identity.len() == 1 && c.color_identity[0] == color;
        let colors: Vec<&str> = COLORS
            .into_iter()
            .filter(|col| self.base.iter().flatten().filter(|c| mono(c, col)).count() >= THEME_MIN_CARDS)
            .collect();
        if colors.is_empty() {
            return;
        }
        let color = colors[rand::random_range(0..colors.len())];
        log::debug!("Theme color {color}, chosen from {colors:?}");
        let lists = self
            .base
            .iter_mut()
            .chain(&mut self.treated)
            .chain([&mut self.basics, &mut self.all_basics, &mut self.special_lands]);
        for list in lists {
            list.retain(|c| mono(c, color));
        }
    }

    fn union(lists: &[Vec<&'a Card>; 4], tiers: &[usize]) -> Vec<&'a Card> {
        tiers.iter().flat_map(|&t| lists[t].iter().copied()).collect()
    }
}

/// How a rarity-based source maps onto the pool's rarity tiers.
struct Tiers {
    /// The rarities the source nominally draws from.
    primary: Vec<usize>,
    /// Groups of tiers to try in turn until one has cards.
    order: Vec<Vec<usize>>,
}

struct Generator<'a> {
    set: &'a SetData,
    def: &'a Definition,
    pool: Pool<'a>,
    /// Scryfall ids already in the pack, to avoid duplicates.
    used: Vec<String>,
    fetch_list_card: &'a dyn Fn() -> Option<Card>,
}

impl<'a> Generator<'a> {
    /// A random card from the list, preferring foil-capable cards when the
    /// slot is foil, and cards not yet in the pack.
    fn pick(&mut self, list: &[&'a Card], want_foil: bool) -> Option<&'a Card> {
        let foil_ok = |c: &Card| !want_foil || c.has_finish("foil");
        let unused = |c: &Card| !self.used.contains(&c.id);
        let subset = |keep: &dyn Fn(&Card) -> bool| -> Vec<&'a Card> {
            list.iter().copied().filter(|c| keep(c)).collect()
        };
        let mut candidates = subset(&|c| foil_ok(c) && unused(c));
        if candidates.is_empty() {
            candidates = subset(&foil_ok);
        }
        if candidates.is_empty() {
            candidates = subset(&unused);
        }
        if candidates.is_empty() {
            candidates = list.to_vec();
        }
        let card = random_of(&candidates)?;
        self.used.push(card.id.clone());
        Some(card)
    }

    fn tiers(&self, source: Source) -> Option<Tiers> {
        let singles = |tiers: [usize; 4]| tiers.iter().map(|&t| vec![t]).collect();
        Some(match source {
            Source::Common => Tiers { primary: vec![0], order: singles([0, 1, 2, 3]) },
            Source::Uncommon => Tiers { primary: vec![1], order: singles([1, 0, 2, 3]) },
            Source::Rare => Tiers { primary: vec![2], order: singles([2, 3, 1, 0]) },
            Source::Mythic => Tiers { primary: vec![3], order: singles([3, 2, 1, 0]) },
            Source::RareMythic => {
                let odds = self.def.mythic_odds.unwrap_or(DEFAULT_MYTHIC_ODDS);
                let order = if rand::random_bool(odds) { [3, 2, 1, 0] } else { [2, 3, 1, 0] };
                Tiers { primary: vec![2, 3], order: singles(order) }
            }
            Source::CommonUncommon => {
                Tiers { primary: vec![0, 1], order: vec![vec![0, 1], vec![2], vec![3]] }
            }
            Source::Wildcard => {
                let weights = self.def.wildcard_weights.unwrap_or(DEFAULT_WILDCARD_WEIGHTS);
                let total: f64 = weights.iter().sum();
                let mut roll = if total > 0.0 { rand::random_range(0.0..total) } else { 0.0 };
                let mut tier = 0;
                for (i, w) in weights.iter().enumerate() {
                    tier = i;
                    if roll < *w {
                        break;
                    }
                    roll -= w;
                }
                let order = (0..=tier).rev().chain(tier + 1..4).map(|t| vec![t]).collect();
                Tiers { primary: vec![tier], order }
            }
            _ => return None,
        })
    }

    /// Draw one card for a source, or `None` when the set has nothing for it.
    fn draw(
        &mut self,
        source: Source,
        treatment: Treatment,
        treatment_odds: f64,
        want_foil: bool,
    ) -> Option<Card> {
        if let Some(tiers) = self.tiers(source) {
            // A special-treatment slot falls back to any treatment of the
            // right rarity, then to regular printings.
            if treatment != Treatment::Regular {
                let any = Pool::union(&self.pool.treated, &tiers.primary);
                let exact: Vec<&Card> = any
                    .iter()
                    .copied()
                    .filter(|c| match treatment {
                        Treatment::Extended => is_extended(c),
                        Treatment::Showcase => !is_extended(c),
                        _ => true,
                    })
                    .collect();
                for list in [exact, any] {
                    if let Some(card) = self.pick(&list, want_foil) {
                        return Some(card.clone());
                    }
                }
            }
            for group in &tiers.order {
                let list = Pool::union(&self.pool.base, group);
                if let Some(card) = self.pick(&list, want_foil) {
                    return Some(self.maybe_upgrade(card, treatment_odds, want_foil).clone());
                }
            }
            return None;
        }

        let lists: Vec<Vec<&'a Card>> = match source {
            Source::BasicLand => vec![self.pool.basics.clone()],
            Source::SpecialLand => vec![self.pool.special_lands.clone(), self.pool.all_basics.clone()],
            Source::Token => vec![self.set.tokens.iter().collect()],
            Source::ArtCard => vec![self.set.art.iter().collect()],
            Source::SpecialGuest => vec![self.set.guests.iter().collect()],
            Source::List => return (self.fetch_list_card)(),
            _ => return None,
        };
        for list in lists {
            if let Some(card) = self.pick(&list, want_foil) {
                return Some(card.clone());
            }
        }
        // Sets without basic lands fill a land slot with a common.
        match source {
            Source::BasicLand | Source::SpecialLand => {
                self.draw(Source::Common, Treatment::Regular, 0.0, want_foil)
            }
            _ => None,
        }
    }

    /// Swap a card for one of its special-treatment printings, by chance.
    fn maybe_upgrade(&self, card: &'a Card, odds: f64, want_foil: bool) -> &'a Card {
        if odds <= 0.0 || card.oracle_id.is_none() || !rand::random_bool(odds) {
            return card;
        }
        let alts: Vec<&Card> = self
            .pool
            .treated
            .iter()
            .flatten()
            .copied()
            .filter(|t| t.oracle_id == card.oracle_id && (!want_foil || t.has_finish("foil")))
            .collect();
        random_of(&alts).unwrap_or(card)
    }

    fn draw_for_slot(&mut self, slot: &Slot, foil: bool) -> Option<Item> {
        self.draw(slot.source, slot.treatment, slot.treatment_odds, foil)
            .map(Item::Card)
            .or_else(|| slot.placeholder.clone().map(Item::Placeholder))
    }

    /// The cards for one slot as (label, item, foil wanted).
    fn fill(&mut self, slot: &Slot, count: usize) -> Vec<(String, Item, bool)> {
        let foil = slot.foil || (slot.foil_odds > 0.0 && rand::random_bool(slot.foil_odds));
        let items: Vec<Item> = if slot.identical {
            self.draw_for_slot(slot, foil).map(|item| vec![item; count]).unwrap_or_default()
        } else {
            (0..count).filter_map(|_| self.draw_for_slot(slot, foil)).collect()
        };
        let mut out: Vec<_> = items.into_iter().map(|i| (slot.label.clone(), i, foil)).collect();

        // At most one replacement per slot takes the place of its last card.
        let mut roll: f64 = rand::random_range(0.0..1.0);
        let released = self.set.info.released_at.as_deref().unwrap_or("");
        for rep in &slot.replace {
            if roll >= rep.odds {
                roll -= rep.odds;
                continue;
            }
            if count == 0 || rep.set_before.as_deref().is_some_and(|d| released >= d) {
                break;
            }
            let item = match rep.source {
                Source::Placeholder => rep.placeholder.clone().map(Item::Placeholder),
                source => self
                    .draw(source, rep.treatment, 0.0, rep.foil)
                    // A foil replacement needs a card printed in foil.
                    .filter(|c| !rep.foil || c.has_finish("foil"))
                    .map(Item::Card),
            };
            if let Some(item) = item {
                log::debug!(
                    "Slot \"{}\": last card replaced by {:?} ({:.1}% chance)",
                    slot.label,
                    rep.source,
                    rep.odds * 100.0
                );
                if out.len() == count {
                    out.pop();
                }
                out.push((rep.label.clone().unwrap_or_else(|| slot.label.clone()), item, rep.foil));
            }
            break;
        }
        out
    }
}

fn finish_for(item: &Item, want_foil: bool) -> Finish {
    let Item::Card(card) = item else { return Finish::Nonfoil };
    let has = |f| card.has_finish(f);
    if want_foil {
        if has("foil") {
            Finish::Foil
        } else if has("etched") {
            Finish::Etched
        } else {
            Finish::Nonfoil
        }
    } else if has("nonfoil") || card.finishes.is_empty() {
        Finish::Nonfoil
    } else if has("foil") {
        Finish::Foil
    } else {
        Finish::Etched
    }
}

fn slot_count(slot: &Slot, previous: usize) -> usize {
    if let Some(total) = slot.complement_of_previous {
        return total.saturating_sub(previous);
    }
    let Some(weights) = &slot.count_weights else { return slot.count };
    let mut roll = rand::random_range(0..weights.values().sum::<u32>().max(1));
    for (count, weight) in weights {
        if roll < *weight {
            return count.parse().unwrap_or(slot.count);
        }
        roll -= weight;
    }
    slot.count
}

/// Open one booster. `fetch_list_card` is called only when a slot rolls a
/// card from The List, which is too large to preload.
pub fn generate(set: &SetData, def: &Definition, fetch_list_card: &dyn Fn() -> Option<Card>) -> Pack {
    let mut pool = Pool::new(set);
    if def.theme == Some(Theme::Color) {
        pool.apply_color_theme();
    }
    let mut generator = Generator { set, def, pool, used: Vec::new(), fetch_list_card };

    let mut cards: Vec<PackCard> = Vec::new();
    let mut foil_eligible: Vec<usize> = Vec::new();
    let mut previous = 0;
    log::debug!(
        "Generating {} ({}) for {}: base pool {:?} by rarity, treatments {:?}, {} basic lands",
        def.name,
        def.file,
        set.info.code,
        generator.pool.base.each_ref().map(Vec::len),
        generator.pool.treated.each_ref().map(Vec::len),
        generator.pool.basics.len()
    );
    for slot in &def.slots {
        let count = slot_count(slot, previous);
        previous = count;
        let filled = generator.fill(slot, count);
        log::debug!(
            "Slot \"{}\" ({:?}, {} wanted): {}",
            slot.label,
            slot.source,
            count,
            filled
                .iter()
                .map(|(_, item, _)| match item {
                    Item::Card(c) => format!("{} [{} {}]", c.name, c.set, c.collector_number),
                    Item::Placeholder(p) => format!("<{p}>"),
                })
                .collect::<Vec<_>>()
                .join("; ")
        );
        for (label, item, want_foil) in filled {
            let finish = finish_for(&item, want_foil);
            let can_foil = matches!(&item, Item::Card(c) if c.has_finish("foil"));
            if slot.random_foil_eligible && finish == Finish::Nonfoil && can_foil {
                foil_eligible.push(cards.len());
            }
            cards.push(PackCard { slot: label, item, finish });
        }
    }
    for _ in 0..def.random_foils {
        if foil_eligible.is_empty() {
            break;
        }
        let index = foil_eligible.swap_remove(rand::random_range(0..foil_eligible.len()));
        cards[index].finish = Finish::Foil;
        log::debug!("Random foil: {} in slot \"{}\"", cards[index].name(), cards[index].slot);
    }

    let pack = Pack { booster_name: def.name.clone(), set_name: set.info.name.clone(), cards };
    log::info!(
        "Opened a {} for {}: {} cards, ${:.2}",
        pack.booster_name,
        pack.set_name,
        pack.cards.len(),
        pack.total_price()
    );
    pack
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{self, options_for};

    fn card(name: String, rarity: &str, type_line: &str) -> Card {
        Card {
            id: name.clone(),
            oracle_id: Some(name.clone()),
            name,
            rarity: rarity.into(),
            type_line: Some(type_line.into()),
            booster: true,
            finishes: vec!["nonfoil".into(), "foil".into()],
            ..Default::default()
        }
    }

    fn set_of(code: &str, name: &str, set_type: &str, released: &str) -> SetData {
        let mut cards = Vec::new();
        for (rarity, n) in [("common", 80), ("uncommon", 60), ("rare", 40), ("mythic", 10)] {
            cards.extend((0..n).map(|i| card(format!("{rarity} {i}"), rarity, "Creature")));
        }
        cards.extend((0..5).map(|i| card(format!("land {i}"), "common", "Basic Land — Island")));
        for (i, c) in cards.iter_mut().enumerate() {
            c.color_identity = vec![COLORS[i % 5].to_string()];
        }
        SetData {
            info: SetInfo {
                code: code.into(),
                name: name.into(),
                set_type: set_type.into(),
                released_at: Some(released.into()),
                parent_set_code: None,
                digital: false,
                card_count: cards.len() as u32,
            },
            cards,
            tokens: vec![card("token".into(), "common", "Token Creature")],
            art: vec![],
            guests: vec![],
        }
    }

    fn expansion(released: &str) -> SetData {
        set_of("tst", "Test", "expansion", released)
    }

    fn defs() -> Vec<Definition> {
        definition::repo_definitions()
    }

    /// The files offered for a set, in display order.
    fn files(defs: &[Definition], set: &SetData) -> Vec<String> {
        options_for(defs, &set.info).iter().map(|d| d.file.clone()).collect()
    }

    fn open(defs: &[Definition], set: &SetData, file: &str) -> Pack {
        let def = options_for(defs, &set.info)
            .into_iter()
            .find(|d| d.file == file)
            .unwrap_or_else(|| panic!("{file} is not offered for this set"));
        generate(set, def, &|| None)
    }

    fn count(pack: &Pack, slot: &str) -> usize {
        pack.cards.iter().filter(|c| c.slot.contains(slot)).count()
    }

    fn foils(pack: &Pack) -> usize {
        pack.cards.iter().filter(|c| c.finish == Finish::Foil).count()
    }

    #[test]
    fn options_follow_availability() {
        let d = defs();
        let offered = |date| files(&d, &expansion(date));
        assert_eq!(offered("2024-02-09"), ["play.toml", "collector.toml"]);
        assert_eq!(offered("2023-11-17"), ["draft.toml", "set.toml", "collector_2019.toml"]);
        assert_eq!(offered("2020-09-25"), ["draft.toml", "set.toml", "collector_2019.toml"]);
        assert_eq!(offered("2020-07-03"), ["draft.toml", "collector_2019.toml"]);
        assert_eq!(offered("2019-07-12"), ["draft.toml"]);
        assert_eq!(offered("2008-07-25"), ["draft_2007.toml"]);
        assert_eq!(offered("2007-05-04"), ["draft_early.toml"]);

        let masters = |date| files(&d, &set_of("tst", "Test", "masters", date));
        assert_eq!(masters("2018-12-07"), ["masters.toml"]);
        assert_eq!(masters("2023-01-13"), ["masters.toml", "collector_2019.toml"]);
        assert_eq!(masters("1995-07-01"), ["draft_early.toml"]);
        assert_eq!(masters("2025-01-24"), ["play.toml", "collector.toml"]);

        // A definition naming a set replaces the general one with its id.
        let double = set_of("2x2", "Double Masters 2022", "masters", "2022-07-08");
        assert_eq!(files(&d, &double), ["masters_double.toml", "collector_2019.toml"]);
        // An exclusive definition hides the others.
        let jump = set_of("j22", "Jumpstart 2022", "draft_innovation", "2022-12-02");
        assert_eq!(files(&d, &jump), ["jumpstart.toml"]);
    }

    #[test]
    fn pack_sizes_and_slots() {
        let d = defs();
        let new = expansion("2025-01-01");
        let old = expansion("2021-01-01");
        for _ in 0..300 {
            let p = open(&d, &new, "play.toml");
            assert_eq!(p.cards.len(), 15);
            assert_eq!(count(&p, "Common"), 7);
            assert_eq!(count(&p, "Uncommon"), 3);
            assert_eq!(count(&p, "Rare / Mythic"), 1);
            assert_eq!(count(&p, "Wildcard"), 2);
            assert_eq!(foils(&p), 1);

            let c = open(&d, &new, "collector.toml");
            assert_eq!(c.cards.len(), 16); // 15 cards + foil token
            assert_eq!(count(&c, "Common"), 4);
            assert!((4..=5).contains(&count(&c, "Uncommon")));
            assert!((5..=6).contains(&count(&c, "Rare / Mythic")));
            assert_eq!(count(&c, "Uncommon") + count(&c, "Rare / Mythic"), 10);
            assert!((10..=12).contains(&(foils(&c) - 1)), "{} foils", foils(&c) - 1);

            let dr = open(&d, &old, "draft.toml");
            assert_eq!(dr.cards.len(), 16);
            assert_eq!(count(&dr, "Common") + count(&dr, "Foil"), 10);
            assert_eq!(count(&dr, "Basic Land"), 1);

            let s = open(&d, &old, "set.toml");
            assert_eq!(s.cards.len(), 13);
            assert_eq!(count(&s, "Common") + count(&s, "Uncommon"), 6);
            assert_eq!(foils(&s), 1);
            assert_eq!(s.cards.last().unwrap().finish, Finish::Nonfoil);

            let c19 = open(&d, &old, "collector_2019.toml");
            assert_eq!(c19.cards.len(), 16);
            assert_eq!(count(&c19, "Rare / Mythic"), 4);
            assert_eq!(foils(&c19), 12);
        }
    }

    #[test]
    fn older_and_special_boosters() {
        let d = defs();
        let uma = set_of("uma", "Ultimate Masters", "masters", "2018-12-07");
        let j22 = set_of("j22", "Jumpstart 2022", "draft_innovation", "2022-12-02");
        for _ in 0..300 {
            let e = open(&d, &expansion("2005-01-01"), "draft_early.toml");
            assert_eq!(e.cards.len(), 15);
            assert!(e.cards[0].name().starts_with("rare"));
            assert_eq!(count(&e, "Common") + count(&e, "Foil"), 11);

            let t = open(&d, &expansion("2008-01-01"), "draft_2007.toml");
            assert_eq!(t.cards.len(), 16);
            assert_eq!(count(&t, "Token / Tips"), 1);

            let m = open(&d, &uma, "masters.toml");
            assert_eq!(m.cards.len(), 15);
            assert_eq!(foils(&m), 1);
            assert_eq!(count(&m, "Basic Land"), 0);

            let j = open(&d, &j22, "jumpstart.toml");
            assert_eq!(j.cards.len(), 20);
            assert_eq!(count(&j, "Basic Land"), 8);
            assert!((1..=2).contains(&count(&j, "Rare / Mythic")));
            let Item::Card(first) = &j.cards[0].item else { panic!() };
            assert!(j.cards.iter().all(|c| match &c.item {
                Item::Card(c) => c.color_identity == first.color_identity,
                _ => false,
            }));
        }
        let double = set_of("2x2", "Double Masters 2022", "masters", "2022-07-08");
        let dm = open(&d, &double, "masters_double.toml");
        assert_eq!(dm.cards.len(), 15);
        assert_eq!(count(&dm, "Rare / Mythic"), 2);
        assert_eq!(count(&dm, "Foil"), 2);
    }

    #[test]
    fn no_duplicate_cards_outside_foil_slots() {
        let d = defs();
        for _ in 0..100 {
            let p = open(&d, &expansion("2025-01-01"), "play.toml");
            let mut names: Vec<_> =
                p.cards.iter().filter(|c| c.finish == Finish::Nonfoil).map(|c| c.name()).collect();
            let total = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), total);
        }
    }

    #[test]
    fn list_card_fills_the_seventh_common_slot() {
        let d = defs();
        let early = expansion("2024-03-01");
        let play = options_for(&d, &early.info)[0];
        let listed = || Some(card("listed".into(), "rare", "Creature"));
        let hit = (0..2000)
            .map(|_| generate(&early, play, &listed))
            .find(|p| count(p, "The List") == 1)
            .expect("a List card within 2000 packs");
        assert_eq!(hit.cards.len(), 15);
        assert_eq!(count(&hit, "Common"), 6);

        // The List left Play Boosters with Bloomburrow.
        let later = expansion("2024-08-02");
        assert!((0..500).all(|_| count(&generate(&later, play, &listed), "The List") == 0));
    }
}
