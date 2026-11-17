//! Sorting a list of owned cards into booster packs.
//!
//! Each pack is laid out as a row of units (one card position each) taken
//! from a booster definition. Cards are then assigned to units with a
//! bipartite matching, so as many cards as possible land in a slot that
//! could really have held them.

use crate::booster::{Finish, Item, PackCard};
use crate::definition::{Definition, Replace, Slot, Source, Treatment};
use crate::scryfall::Card;

/// One physical card from the list.
#[derive(Clone, Debug)]
pub struct Owned {
    pub card: Card,
    pub finish: Finish,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FoilRule {
    Foil,
    Nonfoil,
    Either,
}

/// What one unit will accept, derived from a slot or a replacement.
#[derive(Clone, Debug)]
struct Spec {
    label: String,
    source: Source,
    treatment: Treatment,
    /// The slot may upgrade a card to a special printing.
    upgradable: bool,
    foil: FoilRule,
}

fn tier_of(card: &Card) -> usize {
    match card.rarity.as_str() {
        "common" => 0,
        "uncommon" => 1,
        "mythic" => 3,
        _ => 2,
    }
}

impl Spec {
    fn of_slot(slot: &Slot) -> Self {
        let foil = if slot.foil {
            FoilRule::Foil
        } else if slot.foil_odds > 0.0 {
            FoilRule::Either
        } else {
            FoilRule::Nonfoil
        };
        Spec {
            label: slot.label.clone(),
            source: slot.source,
            treatment: slot.treatment,
            upgradable: slot.treatment_odds > 0.0,
            foil,
        }
    }

    fn of_replace(rep: &Replace, slot: &Slot) -> Self {
        Spec {
            label: rep.label.clone().unwrap_or_else(|| slot.label.clone()),
            source: rep.source,
            treatment: rep.treatment,
            upgradable: false,
            foil: if rep.foil { FoilRule::Foil } else { FoilRule::Nonfoil },
        }
    }

    /// Only tokens, art cards or placeholders: things card lists often omit.
    fn is_extra(&self) -> bool {
        matches!(self.source, Source::Token | Source::ArtCard | Source::Placeholder)
    }

    /// How well a card suits this spec: `None` if it cannot go here, else a
    /// score where lower is better.
    fn fit(&self, owned: &Owned, main_set: &str, ignore_foil: bool) -> Option<u8> {
        let card = &owned.card;
        let is_foil = owned.finish != Finish::Nonfoil;
        let foil_ok = ignore_foil
            || match self.foil {
                FoilRule::Foil => is_foil,
                FoilRule::Nonfoil => !is_foil,
                FoilRule::Either => true,
            };
        if !foil_ok {
            return None;
        }
        let in_main_set = card.set == main_set && !card.is_token() && !card.is_art_card();
        let special_land = card.full_art || card.is_special_treatment();
        let tier_ok = |tiers: std::ops::RangeInclusive<usize>| {
            (in_main_set && !card.is_basic_land() && tiers.contains(&tier_of(card)))
                .then(|| self.treatment_score(card))
        };
        match self.source {
            Source::Common => tier_ok(0..=0),
            Source::Uncommon => tier_ok(1..=1),
            Source::Rare => tier_ok(2..=2),
            Source::Mythic => tier_ok(3..=3),
            Source::RareMythic => tier_ok(2..=3),
            Source::CommonUncommon => tier_ok(0..=1),
            Source::Wildcard => tier_ok(0..=3),
            Source::BasicLand => {
                (in_main_set && card.is_basic_land()).then_some(special_land as u8)
            }
            Source::SpecialLand => {
                (in_main_set && card.is_basic_land()).then_some(!special_land as u8)
            }
            Source::Token => card.is_token().then_some(0),
            Source::ArtCard => card.is_art_card().then_some(0),
            Source::SpecialGuest => (card.set == "spg").then_some(0),
            Source::List => (card.set == "plst").then_some(0),
            Source::Placeholder => None,
        }
    }

    /// Slots take the printing they ask for first, but accept the others,
    /// just as pack generation falls back when a set lacks a treatment.
    fn treatment_score(&self, card: &Card) -> u8 {
        let treated = card.is_special_treatment();
        let extended = card.frame_effects.iter().any(|f| f == "extendedart");
        match self.treatment {
            Treatment::Regular if !treated => 0,
            Treatment::Regular if self.upgradable => 1,
            Treatment::Regular => 2,
            Treatment::Any if treated => 0,
            Treatment::Extended if extended => 0,
            Treatment::Showcase if treated && !extended => 0,
            _ if treated => 1,
            _ => 2,
        }
    }
}

/// One card position in one pack.
#[derive(Clone, Debug)]
struct Unit {
    pack: usize,
    /// Position of the unit's slot in the definition, for display order.
    slot: usize,
    /// Acceptable contents, most preferred first.
    options: Vec<Spec>,
    /// Extra capacity that a pack may or may not have.
    optional: bool,
    random_foil_eligible: bool,
}

fn count_range(slot: &Slot) -> (usize, usize) {
    match &slot.count_weights {
        Some(weights) => {
            let counts = || weights.iter().filter(|(_, w)| **w > 0).filter_map(|(c, _)| c.parse().ok());
            (counts().min().unwrap_or(slot.count), counts().max().unwrap_or(slot.count))
        }
        None => (slot.count, slot.count),
    }
}

/// The units of one pack, in slot order.
fn units_for_pack(def: &Definition, released: &str, pack: usize) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut previous_max = 0;
    for (i, slot) in def.slots.iter().enumerate() {
        let spec = Spec::of_slot(slot);
        let unit = |options: Vec<Spec>, optional: bool| Unit {
            pack,
            slot: i,
            options,
            optional,
            random_foil_eligible: slot.random_foil_eligible,
        };
        let (min, max) = match slot.complement_of_previous {
            // The cards shared with the previous slot were added with it.
            Some(total) => {
                let fixed = total.saturating_sub(previous_max);
                (fixed, fixed)
            }
            None => count_range(slot),
        };
        previous_max = max;

        let first = units.len();
        units.extend((0..min).map(|_| unit(vec![spec.clone()], false)));
        // A replacement can take the place of the slot's last card.
        if min > 0 {
            let replacements = slot
                .replace
                .iter()
                .filter(|r| r.set_before.as_deref().is_none_or(|d| released < d))
                .map(|r| Spec::of_replace(r, slot));
            let last = &mut units[first + min - 1].options;
            *last = replacements.chain(last.iter().cloned()).collect();
        }
        // A variable count: the spare positions belong to this slot or, when
        // the next slot makes up the difference, to either of the two.
        let next = def.slots.get(i + 1).filter(|n| n.complement_of_previous.is_some());
        for _ in min..max {
            match next {
                Some(next) => units.push(unit(vec![spec.clone(), Spec::of_slot(next)], false)),
                None => units.push(unit(vec![spec.clone()], true)),
            }
        }
    }
    units
}

/// How many packs the list most likely came from: enough to hold every card
/// that is not a basic land, token or art card.
pub fn pack_count(def: &Definition, released: &str, inventory: &[Owned]) -> usize {
    let per_pack = units_for_pack(def, released, 0)
        .iter()
        .filter(|u| !u.optional)
        .filter(|u| {
            u.options.iter().any(|s| {
                !s.is_extra() && !matches!(s.source, Source::BasicLand | Source::SpecialLand)
            })
        })
        .count()
        .max(1);
    let cards = inventory
        .iter()
        .filter(|o| !o.card.is_basic_land() && !o.card.is_token() && !o.card.is_art_card())
        .count();
    cards.div_ceil(per_pack).max(1)
}

pub struct SortedPack {
    pub cards: Vec<PackCard>,
    /// Labels of the positions no card in the list could fill.
    pub missing: Vec<String>,
}

pub struct Sorted {
    pub packs: Vec<SortedPack>,
    /// Cards that did not fit any pack.
    pub leftover: Vec<Owned>,
}

fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, rand::random_range(0..=i));
    }
}

/// Kuhn's augmenting-path step: find a card for `unit`, moving other units
/// to different cards where that frees one up.
fn assign(unit: usize, edges: &[Vec<usize>], holder: &mut [Option<usize>], seen: &mut [bool]) -> bool {
    for &card in &edges[unit] {
        if seen[card] {
            continue;
        }
        seen[card] = true;
        if holder[card].is_none_or(|other| assign(other, edges, holder, seen)) {
            holder[card] = Some(unit);
            return true;
        }
    }
    false
}

/// A real pack rarely holds two copies of a card. Where the matching put
/// two in one pack, trade one with a different card from another pack,
/// provided both cards still suit their new positions.
fn spread_duplicates(
    units: &[Unit],
    held: &mut [Option<usize>],
    inventory: &[Owned],
    main_set: &str,
    locked: &[bool],
) {
    let key = |c: usize| (inventory[c].card.name.as_str(), inventory[c].finish);
    let fits = |u: usize, c: usize| {
        !locked[u] && units[u].options.iter().any(|s| s.fit(&inventory[c], main_set, false).is_some())
    };
    // Whether a pack holds `card`'s twin in a position other than `except`.
    let pack_has = |held: &[Option<usize>], pack: usize, card: usize, except: usize| {
        units
            .iter()
            .enumerate()
            .any(|(u, unit)| unit.pack == pack && u != except && held[u].is_some_and(|c| key(c) == key(card)))
    };
    for u in 0..units.len() {
        let Some(a) = held[u] else { continue };
        if locked[u] || !pack_has(held, units[u].pack, a, u) {
            continue;
        }
        let partner = (0..units.len()).find(|&v| {
            units[v].pack != units[u].pack
                && held[v].is_some_and(|b| {
                    key(b) != key(a)
                        && fits(u, b)
                        && fits(v, a)
                        && !pack_has(held, units[u].pack, b, u)
                        && !pack_has(held, units[v].pack, a, v)
                })
        });
        if let Some(v) = partner {
            log::debug!(
                "Duplicate {} in pack {}: traded with pack {}",
                inventory[a].card.name,
                units[u].pack + 1,
                units[v].pack + 1
            );
            held.swap(u, v);
        }
    }
}

/// Distribute `inventory` over `packs` packs of the given definition.
/// `main_set` is the code of the set the packs are from.
pub fn sort(
    def: &Definition,
    released: &str,
    main_set: &str,
    mut inventory: Vec<Owned>,
    packs: usize,
) -> Sorted {
    // Which card ends up in which pack is random, as it was when opened.
    shuffle(&mut inventory);
    let units: Vec<Unit> = (0..packs).flat_map(|p| units_for_pack(def, released, p)).collect();
    log::info!(
        "Sorting {} cards into {} packs of {} ({} positions)",
        inventory.len(),
        packs,
        def.name,
        units.len()
    );
    // For each unit, the card it holds and whether it got there as the
    // definition's "random foil".
    let mut held: Vec<Option<usize>> = vec![None; units.len()];
    let mut holder: Vec<Option<usize>> = vec![None; inventory.len()];

    // Random foils first: a foil card takes a position that would accept it
    // were it not foil, at most `random_foils` times per pack.
    for pack in 0..packs {
        for _ in 0..def.random_foils {
            let best = units
                .iter()
                .enumerate()
                .filter(|(u, unit)| unit.pack == pack && unit.random_foil_eligible && held[*u].is_none())
                .flat_map(|(u, unit)| {
                    let (inventory, holder) = (&inventory, &holder);
                    inventory
                        .iter()
                        .enumerate()
                        .filter(move |(c, o)| holder[*c].is_none() && o.finish != Finish::Nonfoil)
                        .filter_map(move |(c, o)| {
                            let score = unit.options.iter().filter_map(|s| s.fit(o, main_set, true)).min()?;
                            Some((score, unit.optional, u, c))
                        })
                })
                .min();
            if let Some((_, _, u, c)) = best {
                log::debug!(
                    "Pack {}: {} takes the random foil, in a \"{}\" position",
                    pack + 1,
                    inventory[c].card.name,
                    units[u].options[0].label
                );
                held[u] = Some(c);
                holder[c] = Some(u);
            }
        }
    }
    let foil_placed: Vec<bool> = held.iter().map(Option::is_some).collect();

    // Then a maximum matching for everything else. Units are served slot by
    // slot across the packs, so every pack gets its rare before any pack's
    // spare positions are filled; optional positions come last.
    let edges: Vec<Vec<usize>> = units
        .iter()
        .enumerate()
        .map(|(u, unit)| {
            if foil_placed[u] {
                return Vec::new();
            }
            let mut fits: Vec<(usize, u8, usize)> = inventory
                .iter()
                .enumerate()
                .filter(|(c, _)| holder[*c].is_none())
                .filter_map(|(c, o)| {
                    unit.options
                        .iter()
                        .enumerate()
                        .find_map(|(k, s)| Some((k, s.fit(o, main_set, false)?, c)))
                })
                .collect();
            fits.sort();
            fits.into_iter().map(|(_, _, c)| c).collect()
        })
        .collect();
    let mut order: Vec<usize> = (0..units.len()).filter(|u| !foil_placed[*u]).collect();
    order.sort_by_key(|&u| (units[u].optional, units[u].slot, units[u].pack));
    for unit in order {
        assign(unit, &edges, &mut holder, &mut vec![false; inventory.len()]);
    }
    for (card, unit) in holder.iter().enumerate() {
        if let Some(unit) = unit {
            held[*unit] = Some(card);
        }
    }

    spread_duplicates(&units, &mut held, &inventory, main_set, &foil_placed);

    let no_extras = !inventory.iter().any(|o| o.card.is_token() || o.card.is_art_card());
    let mut sorted: Vec<SortedPack> =
        (0..packs).map(|_| SortedPack { cards: Vec::new(), missing: Vec::new() }).collect();
    for (u, unit) in units.iter().enumerate() {
        let pack = &mut sorted[unit.pack];
        match held[u] {
            Some(card) => {
                let owned = &inventory[card];
                let label = unit
                    .options
                    .iter()
                    .find(|s| s.fit(owned, main_set, foil_placed[u]).is_some())
                    .map_or_else(|| unit.options[0].label.clone(), |s| s.label.clone());
                pack.cards.push(PackCard {
                    slot: label,
                    item: Item::Card(owned.card.clone()),
                    finish: owned.finish,
                });
            }
            None if unit.optional => {}
            // Don't report tokens and art cards as missing from a list that
            // leaves them out altogether.
            None if no_extras && unit.options.iter().all(Spec::is_extra) => {}
            None => {
                let wanted = unit.options.iter().rfind(|s| s.source != Source::Placeholder);
                pack.missing.push(wanted.unwrap_or(&unit.options[0]).label.clone());
            }
        }
    }
    let leftover = inventory
        .iter()
        .zip(&holder)
        .filter(|(_, unit)| unit.is_none())
        .map(|(owned, _)| owned.clone())
        .collect();
    let leftover: Vec<Owned> = leftover;
    log::info!(
        "Placed {} cards; {} left over; {} positions unfilled",
        inventory.len() - leftover.len(),
        leftover.len(),
        sorted.iter().map(|p| p.missing.len()).sum::<usize>()
    );
    for owned in &leftover {
        log::debug!(
            "Left over: {} [{} {}] {:?} {}",
            owned.card.name,
            owned.card.set,
            owned.card.collector_number,
            owned.finish,
            owned.card.rarity
        );
    }
    Sorted { packs: sorted, leftover }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{self, options_for};
    use crate::scryfall::SetInfo;

    fn info(set_type: &str, released: &str) -> SetInfo {
        SetInfo {
            code: "tst".into(),
            name: "Test".into(),
            set_type: set_type.into(),
            released_at: Some(released.into()),
            parent_set_code: None,
            digital: false,
            card_count: 300,
        }
    }

    fn definition_for(released: &str, file: &str) -> Definition {
        let defs = definition::repo_definitions();
        options_for(&defs, &info("expansion", released))
            .into_iter()
            .find(|d| d.file == file)
            .unwrap()
            .clone()
    }

    fn owned(name: &str, rarity: &str, type_line: &str, finish: Finish) -> Owned {
        Owned {
            card: Card {
                id: name.into(),
                name: name.into(),
                rarity: rarity.into(),
                type_line: Some(type_line.into()),
                set: "tst".into(),
                layout: "normal".into(),
                booster: true,
                finishes: vec!["nonfoil".into(), "foil".into()],
                ..Default::default()
            },
            finish,
        }
    }

    fn many(n: usize, prefix: &str, rarity: &str, finish: Finish) -> Vec<Owned> {
        (0..n).map(|i| owned(&format!("{prefix} {i}"), rarity, "Creature", finish)).collect()
    }

    fn lands(n: usize, finish: Finish) -> Vec<Owned> {
        (0..n).map(|_| owned("Forest", "common", "Basic Land — Forest", finish)).collect()
    }

    fn count(pack: &SortedPack, slot: &str) -> usize {
        pack.cards.iter().filter(|c| c.slot == slot).count()
    }

    /// Ten play boosters' worth of cards plus the bundle's 80 lands.
    fn bundle() -> Vec<Owned> {
        let mut cards = many(70, "common", "common", Finish::Nonfoil);
        cards.extend(many(30, "uncommon", "uncommon", Finish::Nonfoil));
        cards.extend(many(8, "rare", "rare", Finish::Nonfoil));
        cards.extend(many(2, "mythic", "mythic", Finish::Nonfoil));
        // Wildcards: ten non-foil and ten foil, of mixed rarity.
        cards.extend(many(6, "wild common", "common", Finish::Nonfoil));
        cards.extend(many(3, "wild uncommon", "uncommon", Finish::Nonfoil));
        cards.extend(many(1, "wild rare", "rare", Finish::Nonfoil));
        cards.extend(many(7, "foil common", "common", Finish::Foil));
        cards.extend(many(2, "foil uncommon", "uncommon", Finish::Foil));
        cards.extend(many(1, "foil rare", "rare", Finish::Foil));
        cards.extend(lands(10 + 40, Finish::Nonfoil));
        cards.extend(lands(40, Finish::Foil));
        cards
    }

    #[test]
    fn a_bundle_sorts_into_ten_complete_play_boosters() {
        let def = definition_for("2025-01-01", "play.toml");
        for _ in 0..20 {
            let cards = bundle();
            assert_eq!(pack_count(&def, "2025-01-01", &cards), 10);
            let sorted = sort(&def, "2025-01-01", "tst", cards, 10);
            for pack in &sorted.packs {
                assert_eq!(pack.cards.len(), 14, "missing {:?}", pack.missing);
                assert!(pack.missing.is_empty());
                assert_eq!(count(pack, "Common"), 7);
                assert_eq!(count(pack, "Uncommon"), 3);
                assert_eq!(count(pack, "Rare / Mythic"), 1);
                assert_eq!(count(pack, "Wildcard"), 1);
                assert_eq!(count(pack, "Foil Wildcard"), 1);
                assert_eq!(count(pack, "Basic Land"), 1);
                let foils = pack.cards.iter().filter(|c| c.finish == Finish::Foil).count();
                assert_eq!(foils, 1);
            }
            // The 80 bundle lands are what is left over.
            assert_eq!(sorted.leftover.len(), 80);
            assert!(sorted.leftover.iter().all(|o| o.card.is_basic_land()));
        }
    }

    #[test]
    fn copies_of_a_card_are_spread_across_packs() {
        let def = definition_for("2025-01-01", "play.toml");
        for _ in 0..50 {
            // Seven commons, two copies of each, for two packs.
            let mut cards = many(7, "common", "common", Finish::Nonfoil);
            cards.extend(many(7, "common", "common", Finish::Nonfoil));
            let sorted = sort(&def, "2025-01-01", "tst", cards, 2);
            assert!(sorted.leftover.is_empty());
            for pack in &sorted.packs {
                let mut names: Vec<_> = pack.cards.iter().map(|c| c.name()).collect();
                names.sort();
                names.dedup();
                assert_eq!(names.len(), 7);
            }
        }
    }

    #[test]
    fn rares_reach_every_pack_before_wildcards_take_them() {
        let def = definition_for("2025-01-01", "play.toml");
        let mut cards = many(21, "common", "common", Finish::Nonfoil);
        cards.extend(many(9, "uncommon", "uncommon", Finish::Nonfoil));
        // Exactly one rare per pack and nothing else for the wildcards.
        cards.extend(many(3, "rare", "rare", Finish::Nonfoil));
        let sorted = sort(&def, "2025-01-01", "tst", cards, 3);
        for pack in &sorted.packs {
            assert_eq!(count(pack, "Rare / Mythic"), 1);
            assert!(pack.missing.contains(&"Wildcard".to_string()));
        }
        assert!(sorted.leftover.is_empty());
    }

    #[test]
    fn special_cards_use_their_replacement_positions() {
        let def = definition_for("2024-03-01", "play.toml");
        let mut cards = many(7, "common", "common", Finish::Nonfoil);
        let mut listed = owned("Old Card", "rare", "Creature", Finish::Nonfoil);
        listed.card.set = "plst".into();
        cards.push(listed);
        let mut token = owned("Clue", "common", "Token Artifact — Clue", Finish::Nonfoil);
        token.card.layout = "token".into();
        token.card.set = "ttst".into();
        cards.push(token);
        // A card from an unrelated set never goes in a pack.
        let mut stray = owned("Stray", "common", "Creature", Finish::Nonfoil);
        stray.card.set = "xyz".into();
        cards.push(stray);

        let sorted = sort(&def, "2024-03-01", "tst", cards, 1);
        let pack = &sorted.packs[0];
        assert_eq!(count(pack, "The List"), 1);
        assert_eq!(count(pack, "Common"), 6);
        assert_eq!(count(pack, "Token / Ad"), 1);
        let left: Vec<_> = sorted.leftover.iter().map(|o| o.card.name.as_str()).collect();
        // The seventh common moves on to the wildcard position.
        assert_eq!(count(pack, "Wildcard"), 1);
        assert_eq!(left, ["Stray"]);
    }

    #[test]
    fn set_boosters_share_commons_and_uncommons_and_take_one_foil() {
        let def = definition_for("2021-01-01", "set.toml");
        let mut cards = lands(2, Finish::Nonfoil);
        // Pack contents: 6 commons/uncommons, showcase, 2 wildcards, rare.
        // Across two packs: 7 uncommons and 11 commons, one of them foil.
        cards.extend(many(7, "uncommon", "uncommon", Finish::Nonfoil));
        cards.extend(many(10, "common", "common", Finish::Nonfoil));
        cards.extend(many(1, "foil common", "common", Finish::Foil));
        cards.extend(many(2, "rare", "rare", Finish::Nonfoil));
        // A second foil has nowhere to go in the pack that already has one.
        let sorted = sort(&def, "2021-01-01", "tst", cards, 2);
        assert!(sorted.leftover.is_empty(), "{:?}", sorted.leftover.len());
        let foils: usize = sorted
            .packs
            .iter()
            .map(|p| p.cards.iter().filter(|c| c.finish == Finish::Foil).count())
            .sum();
        assert_eq!(foils, 1);
        for pack in &sorted.packs {
            assert_eq!(pack.cards.len(), 11);
            assert_eq!(count(pack, "Rare / Mythic"), 1);
        }
    }

    #[test]
    fn collector_boosters_take_foils_and_treatments() {
        let def = definition_for("2025-01-01", "collector.toml");
        let mut cards = many(4, "common", "common", Finish::Foil);
        cards.extend(many(4, "uncommon", "uncommon", Finish::Foil));
        cards.extend(many(1, "foil rare", "rare", Finish::Foil));
        let mut fancy = many(5, "borderless rare", "rare", Finish::Nonfoil);
        for o in &mut fancy {
            o.card.border_color = Some("borderless".into());
        }
        cards.extend(fancy);
        cards.extend(lands(1, Finish::Foil));
        let sorted = sort(&def, "2025-01-01", "tst", cards, 1);
        assert!(sorted.leftover.is_empty());
        let pack = &sorted.packs[0];
        assert_eq!(pack.cards.len(), 15);
        assert_eq!(count(pack, "Foil Uncommon"), 4);
        assert_eq!(count(pack, "Rare / Mythic") + count(pack, "Foil Rare / Mythic"), 6);
    }
}
