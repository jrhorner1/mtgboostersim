//! Reading a list of owned cards from a text or CSV file.
//!
//! Text lines look like `4 Lightning Bolt (MKM) 123 *F*` or
//! `4 Lightning Bolt [MKM] F`; the set, collector number and foil marker are
//! optional. Foil is marked by `*F*` anywhere, or by `F` or `foil` after the
//! set (`*E*`, `E` or `etched` for foil-etched). CSV files need a header row with a
//! name column and, optionally, quantity, set, collector number and foil
//! columns (the exports of the common collection tools work as they are).

use crate::booster::Finish;

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub quantity: u32,
    pub name: String,
    /// Set code or set name, lower-cased, as written in the file.
    pub set: Option<String>,
    pub number: Option<String>,
    pub finish: Finish,
}

const QUANTITY_HEADERS: [&str; 4] = ["quantity", "count", "qty", "amount"];
const NAME_HEADERS: [&str; 3] = ["name", "card name", "card"];
const SET_HEADERS: [&str; 6] = ["set code", "set", "edition", "edition code", "set_code", "set name"];
const NUMBER_HEADERS: [&str; 4] = ["collector number", "collector_number", "card number", "number"];
const FOIL_HEADERS: [&str; 3] = ["foil", "finish", "printing"];
/// Section headings some deck exports include.
const SECTION_WORDS: [&str; 5] = ["deck", "sideboard", "commander", "maybeboard", "companion"];

pub fn parse(text: &str) -> Result<Vec<Row>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let first = text.lines().find(|l| !l.trim().is_empty()).ok_or("The card list is empty.")?;
    let rows = match csv_layout(first) {
        Some(layout) => parse_csv(text, &layout)?,
        None => text.lines().filter_map(parse_text_line).collect(),
    };
    if rows.is_empty() {
        return Err("No cards found in the list.".into());
    }
    Ok(rows)
}

struct CsvLayout {
    delimiter: char,
    quantity: Option<usize>,
    name: usize,
    set: Option<usize>,
    number: Option<usize>,
    foil: Option<usize>,
}

/// Recognise a CSV header row: a delimited line with a name column.
fn csv_layout(header: &str) -> Option<CsvLayout> {
    let delimiter = [',', ';', '\t'].into_iter().max_by_key(|d| header.matches(*d).count())?;
    if !header.contains(delimiter) {
        return None;
    }
    let columns: Vec<String> =
        split_csv(header, delimiter).iter().map(|c| c.trim().to_lowercase()).collect();
    let find = |names: &[&str]| {
        names.iter().find_map(|n| columns.iter().position(|c| c == n))
    };
    Some(CsvLayout {
        delimiter,
        quantity: find(&QUANTITY_HEADERS),
        name: find(&NAME_HEADERS)?,
        set: find(&SET_HEADERS),
        number: find(&NUMBER_HEADERS),
        foil: find(&FOIL_HEADERS),
    })
}

fn split_csv(line: &str, delimiter: char) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let field = fields.last_mut().unwrap();
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => fields.push(String::new()),
            c => field.push(c),
        }
    }
    fields
}

fn parse_csv(text: &str, layout: &CsvLayout) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate().skip_while(|(_, l)| l.trim().is_empty()).skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_csv(line, layout.delimiter);
        let get = |col: Option<usize>| {
            col.and_then(|c| fields.get(c)).map(|f| f.trim()).filter(|f| !f.is_empty())
        };
        let Some(name) = get(Some(layout.name)) else { continue };
        let quantity = match get(layout.quantity) {
            Some(q) => q.parse().map_err(|_| format!("Line {}: bad quantity \"{q}\"", i + 1))?,
            None => 1,
        };
        let finish = match get(layout.foil).map(str::to_lowercase).as_deref() {
            Some(f) if f.contains("etched") => Finish::Etched,
            Some("foil" | "true" | "yes" | "1" | "f") => Finish::Foil,
            _ => Finish::Nonfoil,
        };
        rows.push(Row {
            quantity,
            name: name.to_string(),
            set: get(layout.set).map(str::to_lowercase),
            number: get(layout.number).map(str::to_lowercase),
            finish,
        });
    }
    Ok(rows)
}

/// `4x Lightning Bolt (MKM) 123 *F*`, with everything but the name optional.
fn parse_text_line(line: &str) -> Option<Row> {
    let rest = line.trim();
    if rest.is_empty() || rest.starts_with('#') || rest.starts_with("//") {
        return None;
    }
    if SECTION_WORDS.contains(&rest.trim_end_matches(':').to_lowercase().as_str()) {
        return None;
    }

    let finish = if rest.contains("*E*") {
        Finish::Etched
    } else if rest.contains("*F*") {
        Finish::Foil
    } else {
        Finish::Nonfoil
    };
    let cleaned = rest.replace("*F*", " ").replace("*E*", " ");
    parse_text_body(cleaned.trim(), finish)
}

fn parse_text_body(rest: &str, mut finish: Finish) -> Option<Row> {
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let (quantity, rest) = if digits > 0 && rest[digits..].starts_with([' ', 'x', 'X', '\t']) {
        let after = rest[digits..].trim_start_matches(['x', 'X']).trim_start();
        (rest[..digits].parse().ok()?, after)
    } else {
        (1, rest)
    };

    // The last short bracketed group is the set code: "(MKM)" or "[MKM]".
    let mut name = rest;
    let mut set = None;
    let mut number = None;
    if let Some(open) = rest.rfind(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') { ')' } else { ']' };
        if let Some(len) = rest[open..].find(close_char) {
            let code = &rest[open + 1..open + len];
            let is_code = (2..=6).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric());
            if is_code && open > 0 {
                name = &rest[..open];
                set = Some(code.to_lowercase());
                // After the set: a collector number, a foil indicator, or both.
                for word in rest[open + len + 1..].split_whitespace() {
                    match word.to_lowercase().as_str() {
                        "f" | "foil" => finish = Finish::Foil,
                        "e" | "etched" => finish = Finish::Etched,
                        other if number.is_none() => number = Some(other.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }
    let name = name.trim();
    (!name.is_empty() && quantity > 0).then(|| Row { quantity, name: name.to_string(), set, number, finish })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(quantity: u32, name: &str, set: Option<&str>, number: Option<&str>, finish: Finish) -> Row {
        Row {
            quantity,
            name: name.into(),
            set: set.map(Into::into),
            number: number.map(Into::into),
            finish,
        }
    }

    #[test]
    fn text_lines() {
        let rows = parse(
            "# my bundle\nDeck\n4 Lightning Bolt\n2x Shock (MKM) 144\n1 Forest [mkm] 280 *F*\n\
             Borborygmos, Enraged (MKM)\n1 Erase (Not the Urza's Legacy One) (UNH) 10\n",
        )
        .unwrap();
        assert_eq!(
            rows,
            [
                row(4, "Lightning Bolt", None, None, Finish::Nonfoil),
                row(2, "Shock", Some("mkm"), Some("144"), Finish::Nonfoil),
                row(1, "Forest", Some("mkm"), Some("280"), Finish::Foil),
                row(1, "Borborygmos, Enraged", Some("mkm"), None, Finish::Nonfoil),
                row(1, "Erase (Not the Urza's Legacy One)", Some("unh"), Some("10"), Finish::Nonfoil),
            ]
        );
    }

    #[test]
    fn bare_foil_indicator_after_the_set() {
        let rows = parse(
            "1 Shock [MKM]\n1 Shock [MKM] F\n2 Forest [mkm] f\n1 Island (MKM) 273 foil\n\
             1 Swamp [MKM] E\n1 Gruul Signet F [MKM]\n",
        )
        .unwrap();
        assert_eq!(
            rows,
            [
                row(1, "Shock", Some("mkm"), None, Finish::Nonfoil),
                row(1, "Shock", Some("mkm"), None, Finish::Foil),
                row(2, "Forest", Some("mkm"), None, Finish::Foil),
                row(1, "Island", Some("mkm"), Some("273"), Finish::Foil),
                row(1, "Swamp", Some("mkm"), None, Finish::Etched),
                // An F before the set is part of the name, not an indicator.
                row(1, "Gruul Signet F", Some("mkm"), None, Finish::Nonfoil),
            ]
        );
    }

    #[test]
    fn csv_with_quoted_names() {
        let rows = parse(
            "Count,Name,Edition,Collector Number,Foil\n\
             2,\"Borborygmos, Enraged\",MKM,191,\n1,Shock,mkm,144,foil\n3,Island,MKM,,etched\n",
        )
        .unwrap();
        assert_eq!(
            rows,
            [
                row(2, "Borborygmos, Enraged", Some("mkm"), Some("191"), Finish::Nonfoil),
                row(1, "Shock", Some("mkm"), Some("144"), Finish::Foil),
                row(3, "Island", Some("mkm"), None, Finish::Etched),
            ]
        );
    }

    #[test]
    fn a_name_with_a_comma_is_not_csv() {
        let rows = parse("1 Borborygmos, Enraged\n").unwrap();
        assert_eq!(rows, [row(1, "Borborygmos, Enraged", None, None, Finish::Nonfoil)]);
    }

    #[test]
    fn empty_lists_are_errors() {
        assert!(parse("\n\n").is_err());
        assert!(parse("# nothing here\n").is_err());
    }
}
