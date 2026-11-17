mod app;
mod booster;
mod cache;
mod cardlist;
mod definition;
mod images;
mod logging;
mod scryfall;
mod sorter;

use booster::{Finish, PackCard, SetData};
use definition::Source;
use scryfall::{Card, SetInfo};
use sorter::Owned;
use std::collections::BTreeMap;

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Options that apply to every way of running the app.
    let mut take_flag = |names: &[&str]| {
        let before = args.len();
        args.retain(|a| !names.contains(&a.as_str()));
        args.len() != before
    };
    let verbose = take_flag(&["--verbose", "-v"]);
    let refresh = take_flag(&["--refresh"]);
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }

    let log_file = logging::init(verbose);
    log::info!("mtgboostersim {} starting", env!("CARGO_PKG_VERSION"));
    match (&log_file, cache::dir()) {
        (Some(file), Some(dir)) => {
            log::debug!("Log file: {}", file.display());
            log::debug!("Cache folder: {}", dir.display());
        }
        _ => log::warn!("No cache folder available; nothing will be cached or logged to a file"),
    }
    if refresh {
        log::info!("--refresh: ignoring cached Scryfall data for this run");
        cache::set_refresh(true);
    }

    match args.first().map(String::as_str) {
        Some("--cli") => return cli(&args[1..]).map_err(Into::into),
        Some("--sort") => return sort_cli(&args[1..]).map_err(Into::into),
        Some(other) => return Err(format!("Unknown option '{other}'.\n\n{USAGE}").into()),
        None => {}
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "MTG Booster Simulator",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )?;
    Ok(())
}

const USAGE: &str = "\
mtgboostersim
    Open the window.

mtgboostersim --cli <set code> [booster id]
    Print one random booster, e.g. `--cli mkm collector`.

mtgboostersim --sort <file> [--set <code>] [--booster <id>] [--packs <n>]
    Sort a list of cards you own into booster packs, e.g. the contents of a
    bundle. The file is a text list (`4 Shock (MKM) 144 *F*` or
    `4 Shock [MKM] F`, where the set, collector number and foil marker are
    optional) or a CSV export with a header row. Without --set the most common set in the list is used,
    without --booster the set's first booster type, and without --packs the
    number of packs the cards would fill.

Options for all of the above:
    --verbose, -v   Show debug detail in the terminal: every Scryfall request,
                    cache hit and slot decision. The log file always has it.
    --refresh       Ignore cached Scryfall data and download it again.

Scryfall data and card images are cached on disk: the list of sets for a day,
card data for a week, images for good. The latest run's log is written to
mtgboostersim.log in the cache folder (shown with --verbose). Set
MTGBOOSTERSIM_CACHE to move the cache folder and MTGBOOSTERSIM_LOG to
error, warn, info or debug to change what the terminal shows.";

fn log_request_counts() {
    let (cached, downloaded) = scryfall::request_counts();
    log::info!("Scryfall requests this run: {downloaded} downloaded, {cached} answered from cache");
}

fn finish_tag(finish: Finish) -> &'static str {
    match finish {
        Finish::Nonfoil => "",
        Finish::Foil => " [foil]",
        Finish::Etched => " [etched]",
    }
}

fn print_card(c: &PackCard) {
    let price = c.price().map(|p| format!("  ${p:.2}")).unwrap_or_default();
    println!("  {:<24} {}{}{price}", c.slot, c.name(), finish_tag(c.finish));
}

/// A set code from what a list calls the set: its code or its full name.
fn resolve_set(text: &str, sets: &[SetInfo]) -> Option<String> {
    sets.iter()
        .find(|s| s.code == text)
        .or_else(|| sets.iter().find(|s| s.name.to_lowercase() == text))
        .map(|s| s.code.clone())
}

/// The printing a list row refers to among `cards`: by collector number when
/// given, else by name, preferring the regular booster printing.
fn find_printing<'a>(cards: impl Iterator<Item = &'a Card> + Clone, row: &cardlist::Row) -> Option<&'a Card> {
    if let Some(number) = &row.number {
        match cards.clone().find(|c| c.collector_number.eq_ignore_ascii_case(number)) {
            Some(card) if names_match(&card.name, &row.name) => return Some(card),
            Some(card) => log::warn!(
                "{} #{number} is {}, not {}; going by the name instead",
                card.set.to_uppercase(),
                card.name,
                row.name
            ),
            None => {}
        }
    }
    let wants_foil = row.finish == Finish::Foil;
    cards
        .filter(|c| names_match(&c.name, &row.name))
        .min_by_key(|c| (wants_foil && !c.has_finish("foil"), c.is_special_treatment(), !c.booster))
}

/// Whether a list's name refers to a card: its full name, or the front face
/// of a two-faced card.
fn names_match(card_name: &str, listed: &str) -> bool {
    let (card_name, listed) = (card_name.to_lowercase(), listed.to_lowercase());
    card_name == listed || card_name.starts_with(&format!("{listed} //"))
}

/// Look a list row up, first in the set data already fetched, then on Scryfall.
fn identify(row: &cardlist::Row, set: Option<&str>, data: &SetData) -> Result<Option<Card>, String> {
    let main = data.info.code.as_str();
    let set = set.unwrap_or(main);
    if set == main {
        let extras = data.tokens.iter().chain(&data.art);
        let found = find_printing(data.cards.iter(), row)
            // A token or art card listed without its own set code.
            .or_else(|| find_printing(extras.filter(|_| row.set.is_none()), row));
        if found.is_some() {
            return Ok(found.cloned());
        }
    }
    let related = data.tokens.iter().chain(&data.art).chain(&data.guests).filter(|c| c.set == set);
    if let Some(card) = find_printing(related, row) {
        return Ok(Some(card.clone()));
    }
    if let Some(number) = &row.number {
        match scryfall::card_by_number(set, number)? {
            Some(card) if names_match(&card.name, &row.name) => return Ok(Some(card)),
            Some(card) => log::warn!(
                "{} #{number} is {}, not {}; going by the name instead",
                set.to_uppercase(),
                card.name,
                row.name
            ),
            None => {}
        }
    }
    scryfall::card_named(&row.name, Some(set))
}

fn sort_cli(args: &[String]) -> Result<(), String> {
    let mut file = None;
    let mut flags: BTreeMap<&str, &str> = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            flag @ ("--set" | "--booster" | "--packs") => {
                let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
                flags.insert(flag, value);
            }
            other if other.starts_with("--") => return Err(format!("Unknown option '{other}'.\n\n{USAGE}")),
            path if file.is_none() => file = Some(path),
            extra => return Err(format!("Unexpected argument '{extra}'.\n\n{USAGE}")),
        }
    }
    let file = file.ok_or(USAGE)?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    let rows = cardlist::parse(&text)?;
    log::info!(
        "Read {} lines ({} cards) from {file}",
        rows.len(),
        rows.iter().map(|r| r.quantity).sum::<u32>()
    );

    let sets = scryfall::fetch_sets()?;
    let row_sets: Vec<Option<String>> = rows
        .iter()
        .map(|r| r.set.as_deref().map(|s| resolve_set(s, &sets).unwrap_or_else(|| s.to_string())))
        .collect();
    let main_code = match flags.get("--set") {
        Some(code) => code.to_lowercase(),
        None => {
            let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
            for (row, set) in rows.iter().zip(&row_sets) {
                if let Some(set) = set {
                    *counts.entry(set).or_default() += row.quantity;
                }
            }
            let (code, _) = counts
                .into_iter()
                .max_by_key(|(_, n)| *n)
                .ok_or("The list names no sets; say which set the packs are from with --set <code>.")?;
            code.to_string()
        }
    };
    let info = sets
        .iter()
        .find(|s| s.code == main_code)
        .ok_or_else(|| format!("No set with code '{main_code}'"))?;

    let defs = definition::load()?;
    let options = definition::options_for(&defs, info);
    let ids = || options.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", ");
    let def = match flags.get("--booster") {
        None => options.first().copied(),
        Some(id) => options.iter().copied().find(|d| d.id == id.to_lowercase()),
    }
    .ok_or_else(|| format!("{} offers: {}", info.name, ids()))?;

    log::info!("Packs are {} from {} ({})", def.name, info.name, info.code);
    let data = booster::load_set(info, &sets, def.uses(Source::SpecialGuest))?;
    let mut inventory = Vec::new();
    let mut unknown = Vec::new();
    for (row, set) in rows.iter().zip(&row_sets) {
        match identify(row, set.as_deref(), &data)? {
            Some(card) => {
                let copy = Owned { card, finish: row.finish };
                inventory.extend(std::iter::repeat_n(copy, row.quantity as usize));
            }
            None => {
                log::warn!("Not found on Scryfall: {} (set {:?}, number {:?})", row.name, set, row.number);
                unknown.push(row);
            }
        }
    }
    log_request_counts();

    let released = info.released_at.as_deref().unwrap_or("");
    let packs = match flags.get("--packs") {
        Some(n) => n.parse().ok().filter(|n| *n > 0).ok_or("--packs needs a whole number above zero")?,
        None => sorter::pack_count(def, released, &inventory),
    };
    let total_cards = inventory.len();
    let sorted = sorter::sort(def, released, &info.code, inventory, packs);

    println!("{} — {} × {} from {} cards", info.name, packs, def.name, total_cards);
    if !flags.contains_key("--packs") {
        println!("(pack count worked out from the list; set it with --packs)");
    }
    let mut total = 0.0;
    for (i, pack) in sorted.packs.iter().enumerate() {
        let value: f64 = pack.cards.iter().filter_map(PackCard::price).sum();
        total += value;
        println!("\nPack {} — {} cards, ${value:.2}", i + 1, pack.cards.len());
        pack.cards.iter().for_each(print_card);
        if !pack.missing.is_empty() {
            let mut missing: BTreeMap<&str, usize> = BTreeMap::new();
            for label in &pack.missing {
                *missing.entry(label).or_default() += 1;
            }
            let text: Vec<String> = missing.iter().map(|(label, n)| format!("{n} × {label}")).collect();
            println!("  Nothing in the list for: {}", text.join(", "));
        }
    }
    println!("\nValue of the packs: ${total:.2}");

    if !sorted.leftover.is_empty() {
        let mut groups: BTreeMap<(String, String, String, &str), usize> = BTreeMap::new();
        for o in &sorted.leftover {
            let key = (
                o.card.name.clone(),
                o.card.set.to_uppercase(),
                o.card.collector_number.clone(),
                finish_tag(o.finish),
            );
            *groups.entry(key).or_default() += 1;
        }
        println!("\nNot in any pack ({} cards):", sorted.leftover.len());
        for ((name, set, number, finish), n) in groups {
            println!("  {n:>3} × {name} ({set}) {number}{finish}");
        }
    }
    if !unknown.is_empty() {
        println!("\nNot found on Scryfall:");
        for row in unknown {
            let set = row.set.as_deref().map(|s| format!(" ({})", s.to_uppercase())).unwrap_or_default();
            println!("  {} × {}{set}", row.quantity, row.name);
        }
    }
    Ok(())
}

fn cli(args: &[String]) -> Result<(), String> {
    let code = args.first().ok_or("usage: --cli <set code> [booster type]")?.to_lowercase();
    let sets = scryfall::fetch_sets()?;
    let info = sets
        .iter()
        .find(|s| s.code == code)
        .ok_or_else(|| format!("No set with code '{code}'"))?;
    let defs = definition::load()?;
    let options = definition::options_for(&defs, info);
    let ids = || options.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", ");
    let def = match args.get(1) {
        None => options.first().copied(),
        Some(id) => options.iter().copied().find(|d| d.id == id.to_lowercase()),
    }
    .ok_or_else(|| format!("{} offers: {}", info.name, ids()))?;
    let with_guests = options.iter().any(|d| d.uses(Source::SpecialGuest));
    let data = booster::load_set(info, &sets, with_guests)?;
    let pack = booster::generate(&data, def, &|| scryfall::random("e:plst").ok().flatten());
    println!("{} — {} ({} cards)", pack.set_name, pack.booster_name, pack.cards.len());
    pack.cards.iter().for_each(print_card);
    println!("Estimated value: ${:.2}", pack.total_price());
    log_request_counts();
    Ok(())
}
