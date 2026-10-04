//! No-Intro style filename parser (offline, deterministic).
//!
//! `Legend of Zelda, The - A Link to the Past (Europe).sfc` → title
//! `The Legend of Zelda - A Link to the Past`, region `Europe`.
//! Only the stem is interpreted; nothing touches the filesystem here.

/// Parsed display title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTitle {
    /// Clean title (`The Legend of Zelda - A Link to the Past`).
    pub title: String,
    /// Region tag (`Europe`, `USA`, …) when present.
    pub region: Option<String>,
    /// Extra note (`Rev 1`, `En,Fr,De`, …) when present.
    pub note: Option<String>,
}

/// Known region tags (checked in order).
const REGIONS: &[&str] = &[
    "Europe", "USA", "Japan", "World", "Canada", "Australia", "Korea", "China", "Brazil", "France", "Germany",
    "Spain", "Italy", "Netherlands", "Sweden",
];

/// Parse a ROM file stem (without extension).
pub fn parse_title(stem: &str) -> ParsedTitle {
    // Split trailing `(...)` / `[...]` tags.
    let mut base = stem.trim().to_owned();
    let mut tags: Vec<String> = Vec::new();
    loop {
        let t = base.trim_end().to_owned();
        let (open, close) = if t.ends_with(')') { ('(', ')') } else if t.ends_with(']') { ('[', ']') } else { break };
        let Some(i) = t.rfind(open) else { break };
        tags.push(t[i + 1..t.len() - close.len_utf8()].trim().to_owned());
        base = t[..i].trim_end().to_owned();
    }
    tags.reverse();
    let region = tags
        .iter()
        .find(|t| REGIONS.iter().any(|r| t.split([',', ' ']).any(|w| w == *r)))
        .cloned();
    let rest: Vec<&String> = tags.iter().filter(|t| Some(*t) != region.as_ref()).collect();
    let note = if rest.is_empty() { None } else { Some(rest.into_iter().cloned().collect::<Vec<_>>().join(", ")) };
    // "Legend of Zelda, The - ..." → "The Legend of Zelda - ...".
    // Only the leading article word moves; the rest (subtitle) stays put.
    let title = match base.split_once(", ") {
        Some((head, tail)) => {
            let mut words = tail.splitn(2, ' ');
            match (words.next(), words.next()) {
                (Some(a @ ("The" | "A" | "An")), Some(rest)) => format!("{a} {head} {rest}"),
                (Some(a @ ("The" | "A" | "An")), None) => format!("{a} {head}"),
                _ => base,
            }
        }
        _ => base,
    };
    ParsedTitle { title: title.trim().to_owned(), region, note }
}

/// Move a leading article back to where many ROM files put it:
/// `The Legend of Zelda - A Link to the Past` →
/// `Legend of Zelda, The - A Link to the Past`.
///
/// The article goes after the *first* segment, not at the end, because that is
/// where the `X, The - Subtitle` style puts it. This is the exact inverse of
/// [`parse_title`], so a cover saved with the ROM's own spelling resolves.
///
/// Returns `None` when there is no leading article or the title already uses
/// the `X, The` form, so callers can add it as an extra candidate safely.
pub fn trailed_article(title: &str) -> Option<String> {
    const ARTICLES: &[&str] = &[
        "The", "A", "An", "Le", "La", "Les", "El", "Los", "Las", "Il", "Der", "Die", "Das",
    ];
    let (article, rest) = title.split_once(' ')?;
    if !ARTICLES.contains(&article) {
        return None;
    }
    // Already in `X, The` form: nothing to move.
    if rest.contains(", ") {
        return None;
    }
    // Keep the subtitle on the right of the separator, as the ROM style does.
    let (head, tail) = match rest.split_once(" - ") {
        Some((head, tail)) => (head, format!(" - {tail}")),
        None => (rest, String::new()),
    };
    Some(format!("{head}, {article}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailed_article_round_trips_the_rom_naming() {
        assert_eq!(
            trailed_article("The Legend of Zelda - A Link to the Past").as_deref(),
            Some("Legend of Zelda, The - A Link to the Past")
        );
        assert_eq!(
            trailed_article("The Legend of Zelda - A Link Between Worlds").as_deref(),
            Some("Legend of Zelda, The - A Link Between Worlds")
        );
        // The exact reverse of `parse_title`: feed it back and get the title.
        assert_eq!(
            parse_title("Legend of Zelda, The - A Link to the Past").title,
            "The Legend of Zelda - A Link to the Past"
        );
    }

    #[test]
    fn trailed_article_declines_titles_without_one() {
        assert_eq!(trailed_article("Luigi's Mansion"), None);
        assert_eq!(trailed_article("Super Mario All-Stars"), None);
        assert_eq!(trailed_article("Legend of Zelda, The"), None, "already moved, and no comma to build on");
        assert_eq!(trailed_article(""), None);
        // No subtitle: the article simply goes last.
        assert_eq!(trailed_article("The Last Story").as_deref(), Some("Last Story, The"));
        // A title that starts with a word that is not an article is untouched.
        assert_eq!(trailed_article("Super Mario All-Stars"), None);
    }

    #[test]
    fn parses_real_collection_names() {
        let t = parse_title("Legend of Zelda, The - A Link to the Past (Europe)");
        assert_eq!(t.title, "The Legend of Zelda - A Link to the Past");
        assert_eq!(t.region.as_deref(), Some("Europe"));
        let t = parse_title("New Super Mario Bros. Wii (Europe) (En,Fr,De,Es,It) (Rev 2)");
        assert_eq!(t.title, "New Super Mario Bros. Wii");
        assert_eq!(t.region.as_deref(), Some("Europe"));
        assert!(t.note.unwrap().contains("Rev 2"));
        let t = parse_title("Super Mario Sunshine (USA, Canada)");
        assert_eq!(t.title, "Super Mario Sunshine");
        assert_eq!(t.region.as_deref(), Some("USA, Canada"));
        let t = parse_title("Mario Party 2 (Europe) (En,Fr,De,Es,It)");
        assert_eq!(t.title, "Mario Party 2");
        let t = parse_title("plain game");
        assert_eq!((t.title.as_str(), t.region), ("plain game", None));
    }
}
