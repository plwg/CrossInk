//! Turns Anki decks into deck folders for the e-reader's SD card.
//!
//! Formats match `lib/Flashcards/FlashcardFormat.h` in the firmware and are
//! documented in its `docs/flashcards.md`. All integers are little-endian.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fsrs::{FSRS, FSRSItem};
use rusqlite::Connection;

use crate::anki_model::{self, CardRow, NoteType};
use crate::collection::Collection;
use crate::deck_file::{self, DeckFile};
use crate::revlog::{CardHistory, card_histories};
use crate::template::{RenderContext, RenderedCard, html_to_text, render_card};

const ANKI_SM2_RETENTION: f32 = 0.9;

/// Scheduling state of one card, as in state.bin. All zero means new.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CardState {
    pub stability: f32,
    pub difficulty: f32,
    pub stability_fast: f32,
    /// Unix seconds; 0 for a new card.
    pub last_review: u32,
    pub due: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportCard {
    pub anki_id: i64,
    pub front: String,
    pub back: String,
    pub state: CardState,
}

#[derive(Debug, Default)]
pub struct DeckExport {
    /// Anki deck name, e.g. `Languages::German`.
    pub name: String,
    pub cards: Vec<ExportCard>,
    pub suspended: usize,
    /// Learned in Anki but with no review history to carry over; exported as new.
    pub progress_lost: usize,
}

impl DeckExport {
    pub fn studied(&self) -> usize {
        self.cards
            .iter()
            .filter(|c| c.state.last_review != 0)
            .count()
    }
}

/// Seconds for an FSRS interval, exactly as the reader rounds it.
pub use inki_core::fsrs7::interval_seconds;

/// File name (without extension) for a deck: `::` becomes ` - `, and characters FAT
/// file systems reject become `_`.
pub fn file_stem(deck_name: &str) -> String {
    let joined = deck_name.replace("::", " - ");
    let cleaned: String = joined
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "Deck".to_string()
    } else {
        trimmed.to_string()
    }
}

pub struct ExportOptions<'a> {
    /// Export only this deck and its subdecks, as one folder. Otherwise each
    /// top-level deck (with its subdecks) becomes a folder.
    pub deck: Option<&'a str>,
    /// Carry over progress from the review history.
    pub with_progress: bool,
    pub fsrs: &'a FSRS,
    pub desired_retention: f32,
}

/// A card of a deck group with its rendered text (`None` if its note type is missing).
pub struct GroupedCard {
    /// The exported deck: the requested deck, or the card's top-level deck.
    pub group: String,
    pub row: CardRow,
    pub text: Option<RenderedCard>,
}

/// Cards of `deck` and its subdecks, or of every deck grouped by top-level
/// deck when `deck` is `None`. Suspended cards are included.
pub fn collect_cards(conn: &Connection, deck: Option<&str>) -> Result<Vec<GroupedCard>> {
    let note_types = anki_model::note_types(conn)?;
    let deck_names = anki_model::deck_names(conn)?;
    let mut out = Vec::new();
    for row in anki_model::cards(conn)? {
        let full_name = deck_names
            .get(&row.deck_id)
            .cloned()
            .unwrap_or_else(|| "Default".to_string());
        let group = match deck {
            Some(wanted)
                if full_name == wanted || full_name.starts_with(&format!("{wanted}::")) =>
            {
                wanted.to_string()
            }
            Some(_) => continue,
            None => full_name
                .split("::")
                .next()
                .unwrap_or(&full_name)
                .to_string(),
        };
        let text = note_types.get(&row.note_type_id).map(|nt| render(&row, nt));
        out.push(GroupedCard { group, row, text });
    }
    Ok(out)
}

/// Studied cards first, then new cards in Anki's new-card order, which the
/// device follows when it introduces them.
pub fn device_order(row: &CardRow) -> (bool, i64, i64) {
    let is_new = row.card_type == 0;
    (is_new, if is_new { row.due } else { 0 }, row.id)
}

pub fn build_decks(collection: &Collection, options: &ExportOptions) -> Result<Vec<DeckExport>> {
    let conn = collection.conn();
    let histories: HashMap<i64, CardHistory> = if options.with_progress {
        card_histories(collection.revlog()?)
            .into_iter()
            .map(|h| (h.card_id, h))
            .collect()
    } else {
        HashMap::new()
    };

    let mut decks: HashMap<String, (DeckExport, Vec<(CardRow, ExportCard)>)> = HashMap::new();
    for GroupedCard {
        group,
        row: card,
        text,
    } in collect_cards(conn, options.deck)?
    {
        let (deck, cards) = decks.entry(group.clone()).or_insert_with(|| {
            (
                DeckExport {
                    name: group,
                    ..Default::default()
                },
                Vec::new(),
            )
        });
        if card.queue == -1 {
            deck.suspended += 1;
            continue;
        }
        let Some(text) = text else {
            continue;
        };
        let state = card_state(&card, histories.get(&card.id), options);
        if state.last_review == 0 && card.card_type != 0 {
            deck.progress_lost += 1;
        }
        let export = ExportCard {
            anki_id: card.id,
            front: text.front,
            back: text.back,
            state,
        };
        cards.push((card, export));
    }

    if let Some(wanted) = options.deck
        && decks.is_empty()
    {
        bail!("no deck named {wanted:?} in this collection");
    }

    let mut result: Vec<DeckExport> = decks
        .into_values()
        .filter(|(_, cards)| !cards.is_empty())
        .map(|(mut deck, mut cards)| {
            // Studied cards first; new cards in Anki's new-card order, which
            // the device follows when it introduces them.
            cards.sort_by_key(|(row, _)| device_order(row));
            deck.cards = cards.into_iter().map(|(_, card)| card).collect();
            deck
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

fn render(card: &CardRow, note_type: &NoteType) -> RenderedCard {
    let fields: HashMap<String, String> = note_type
        .field_names
        .iter()
        .cloned()
        .zip(card.fields.iter().cloned())
        .collect();
    let template_index = if note_type.is_cloze {
        0
    } else {
        card.ord as usize
    };
    match note_type.templates.get(template_index) {
        Some((qfmt, afmt)) => render_card(
            qfmt,
            afmt,
            &RenderContext {
                fields: &fields,
                ord: card.ord,
                is_cloze: note_type.is_cloze,
            },
        ),
        // No template: first field on the front, the rest on the back.
        None => RenderedCard {
            front: html_to_text(card.fields.first().map_or("", String::as_str)),
            back: card
                .fields
                .iter()
                .skip(1)
                .map(|f| html_to_text(f))
                .filter(|f| !f.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
        },
    }
}

/// A card's state from its Anki history (new if it has none).
pub fn card_state(
    card: &CardRow,
    history: Option<&CardHistory>,
    options: &ExportOptions,
) -> CardState {
    let Some(history) = history.filter(|_| card.card_type != 0) else {
        return CardState::default();
    };
    let memory = if history.complete {
        options.fsrs.memory_state(
            FSRSItem {
                reviews: history.reviews.clone(),
            },
            None,
        )
    } else if card.interval_days > 0 {
        // Imported or truncated history: start from Anki's SM-2 schedule.
        options.fsrs.memory_state_from_sm2(
            card.factor_permille as f32 / 1000.0,
            card.interval_days as f32,
            ANKI_SM2_RETENTION,
        )
    } else {
        return CardState::default();
    };
    let Ok(memory) = memory else {
        return CardState::default();
    };
    let Ok(last_review) = u32::try_from(history.last_review_ms / 1000) else {
        return CardState::default();
    };
    let interval = options
        .fsrs
        .next_interval_for_state(memory, options.desired_retention);
    CardState {
        stability: memory.stability,
        difficulty: memory.difficulty,
        stability_fast: memory.stability_fast,
        last_review,
        due: last_review.saturating_add(interval_seconds(interval)),
    }
}

/// Writes `<deck>.inki` into `dir`. Refuses to replace an existing file,
/// which may hold progress made on the reader, unless `force` is set.
pub fn write_deck(dir: &Path, deck: &DeckExport, force: bool) -> Result<PathBuf> {
    let path = dir.join(format!(
        "{}.{}",
        file_stem(&deck.name),
        deck_file::EXTENSION
    ));
    if path.exists() && !force {
        bail!(
            "{} already exists and may hold progress made on the e-reader; \
             use --force to replace it",
            path.display()
        );
    }
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let file = DeckFile {
        anki_deck: deck.name.clone(),
        cards: deck.cards.clone(),
        answer_log: Vec::new(),
    };
    fs::write(&path, file.encode()?).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn card(id: i64, front: &str, back: &str) -> ExportCard {
        ExportCard {
            anki_id: id,
            front: front.into(),
            back: back.into(),
            state: CardState::default(),
        }
    }

    pub(crate) const DAY_MS: i64 = 86_400_000;
    pub(crate) const T0: i64 = 1_790_000_000_000;

    /// Cards and notes shared by both schema versions:
    /// note 1 "Basic (and reversed)" in Languages::German -> cards 11 (ord 0) and 12 (ord 1)
    /// note 2 cloze in Languages -> cards 21 (c1) and 22 (c2, suspended)
    /// note 3 basic in Science -> card 31, new
    fn add_cards_and_revlog(conn: &rusqlite::Connection) {
        conn.execute_batch(&format!(
            "CREATE TABLE notes (id INTEGER PRIMARY KEY, guid TEXT, mid INTEGER, mod INTEGER, usn INTEGER,
                 tags TEXT, flds TEXT, sfld TEXT, csum INTEGER, flags INTEGER, data TEXT);
             CREATE TABLE cards (id INTEGER PRIMARY KEY, nid INTEGER, did INTEGER, ord INTEGER, mod INTEGER,
                 usn INTEGER, type INTEGER, queue INTEGER, due INTEGER, ivl INTEGER, factor INTEGER,
                 reps INTEGER, lapses INTEGER, left INTEGER, odue INTEGER, odid INTEGER, flags INTEGER, data TEXT);
             CREATE TABLE revlog (id INTEGER PRIMARY KEY, cid INTEGER, usn INTEGER, ease INTEGER, ivl INTEGER,
                 lastIvl INTEGER, factor INTEGER, time INTEGER, type INTEGER);
             INSERT INTO notes VALUES (1, 'a', 100, 0, 0, '', 'Hund{sep}<b>dog</b>', '', 0, 0, '');
             INSERT INTO notes VALUES (2, 'b', 200, 0, 0, '', '{{{{c1::Paris}}}} is in {{{{c2::France}}}}{sep}extra', '', 0, 0, '');
             INSERT INTO notes VALUES (3, 'c', 100, 0, 0, '', 'H2O{sep}water', '', 0, 0, '');
             INSERT INTO cards VALUES (11, 1, 2, 0, 0, 0, 2, 2, 0, 3, 2500, 2, 0, 0, 0, 0, 0, '');
             INSERT INTO cards VALUES (12, 1, 2, 1, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, '');
             INSERT INTO cards VALUES (21, 2, 1, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, '');
             INSERT INTO cards VALUES (22, 2, 1, 1, 0, 0, 0, -1, 3, 0, 0, 0, 0, 0, 0, 0, 0, '');
             INSERT INTO cards VALUES (31, 3, 3, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, '');
             INSERT INTO revlog VALUES ({t0}, 11, 0, 3, -600, 0, 0, 5000, 0);
             INSERT INTO revlog VALUES ({t1}, 11, 0, 3, 3, -600, 2500, 5000, 1);",
            sep = '\x1f',
            t0 = T0,
            t1 = T0 + DAY_MS,
        ))
        .unwrap();
    }

    const BASIC_Q: &str = "{{Front}}";
    const BASIC_A: &str = "{{FrontSide}}<hr id=answer>{{Back}}";
    const REV_Q: &str = "{{Back}}";
    const REV_A: &str = "{{FrontSide}}<hr id=answer>{{Front}}";
    const CLOZE_Q: &str = "{{cloze:Text}}";
    const CLOZE_A: &str = "{{cloze:Text}}<br>{{Back Extra}}";

    pub(crate) fn current_schema(path: &Path) {
        use crate::anki_model::tests::{proto_string, proto_varint};
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE notetypes (id INTEGER PRIMARY KEY, name TEXT, mtime_secs INTEGER, usn INTEGER, config BLOB);
             CREATE TABLE fields (ntid INTEGER, ord INTEGER, name TEXT, config BLOB);
             CREATE TABLE templates (ntid INTEGER, ord INTEGER, name TEXT, mtime_secs INTEGER, usn INTEGER, config BLOB);
             CREATE TABLE decks (id INTEGER PRIMARY KEY, name TEXT, mtime_secs INTEGER, usn INTEGER, common BLOB, kind BLOB);
             CREATE TABLE col (id INTEGER PRIMARY KEY, crt INTEGER, mod INTEGER);
             INSERT INTO col VALUES (1, 1700000000, 0);
             INSERT INTO decks (id, name) VALUES (1, 'Languages');
             INSERT INTO decks (id, name) VALUES (2, 'Languages' || char(31) || 'German');
             INSERT INTO decks (id, name) VALUES (3, 'Science');",
        )
        .unwrap();
        let mut cloze_config = Vec::new();
        proto_varint(1, 1, &mut cloze_config);
        conn.execute(
            "INSERT INTO notetypes VALUES (100, 'Basic+rev', 0, 0, x'')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO notetypes VALUES (200, 'Cloze', 0, 0, ?1)",
            [cloze_config],
        )
        .unwrap();
        for (ntid, ord, name) in [
            (100, 0, "Front"),
            (100, 1, "Back"),
            (200, 0, "Text"),
            (200, 1, "Back Extra"),
        ] {
            conn.execute(
                "INSERT INTO fields VALUES (?1, ?2, ?3, x'')",
                rusqlite::params![ntid, ord, name],
            )
            .unwrap();
        }
        for (ntid, ord, q, a) in [
            (100, 0, BASIC_Q, BASIC_A),
            (100, 1, REV_Q, REV_A),
            (200, 0, CLOZE_Q, CLOZE_A),
        ] {
            let mut config = Vec::new();
            proto_string(1, q, &mut config);
            proto_string(2, a, &mut config);
            conn.execute(
                "INSERT INTO templates VALUES (?1, ?2, 'Card', 0, 0, ?3)",
                rusqlite::params![ntid, ord, config],
            )
            .unwrap();
        }
        add_cards_and_revlog(&conn);
    }

    fn legacy_schema(path: &Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        let models = serde_json::json!({
            "100": {"type": 0, "flds": [{"name": "Front", "ord": 0}, {"name": "Back", "ord": 1}],
                    "tmpls": [{"ord": 0, "qfmt": BASIC_Q, "afmt": BASIC_A}, {"ord": 1, "qfmt": REV_Q, "afmt": REV_A}]},
            "200": {"type": 1, "flds": [{"name": "Text", "ord": 0}, {"name": "Back Extra", "ord": 1}],
                    "tmpls": [{"ord": 0, "qfmt": CLOZE_Q, "afmt": CLOZE_A}]}
        });
        let decks = serde_json::json!({
            "1": {"name": "Languages"}, "2": {"name": "Languages::German"}, "3": {"name": "Science"}
        });
        conn.execute_batch(
            "CREATE TABLE col (models TEXT, decks TEXT, crt INTEGER DEFAULT 1700000000,
                 mod INTEGER DEFAULT 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO col (models, decks) VALUES (?1, ?2)",
            [models.to_string(), decks.to_string()],
        )
        .unwrap();
        add_cards_and_revlog(&conn);
    }

    fn export(path: &Path, deck: Option<&str>) -> Vec<DeckExport> {
        let fsrs = FSRS::default();
        build_decks(
            &Collection::open(path).unwrap(),
            &ExportOptions {
                deck,
                with_progress: true,
                fsrs: &fsrs,
                desired_retention: 0.9,
            },
        )
        .unwrap()
    }

    fn check_collection(path: &Path) {
        let decks = export(path, None);
        let names: Vec<&str> = decks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Languages", "Science"]);

        let languages = &decks[0];
        assert_eq!(languages.suspended, 1);
        let summary: Vec<(i64, &str, &str)> = languages
            .cards
            .iter()
            .map(|c| (c.anki_id, c.front.as_str(), c.back.as_str()))
            .collect();
        // Studied card first, then new cards in Anki's order (due 2, then 5).
        assert_eq!(
            summary,
            [
                (11, "Hund", "dog"),
                (21, "[...] is in France", "Paris is in France\nextra"),
                (12, "dog", "Hund"),
            ]
        );
        assert_eq!(languages.studied(), 1);

        let studied = languages.cards[0].state;
        let fsrs = FSRS::default();
        let expected = fsrs
            .memory_state(
                FSRSItem {
                    reviews: vec![
                        fsrs::FSRSReview {
                            rating: 3,
                            delta_t: 0.0,
                        },
                        fsrs::FSRSReview {
                            rating: 3,
                            delta_t: 1.0,
                        },
                    ],
                },
                None,
            )
            .unwrap();
        assert_eq!(studied.stability, expected.stability);
        assert_eq!(studied.last_review as i64, (T0 + DAY_MS) / 1000);
        assert_eq!(
            studied.due - studied.last_review,
            interval_seconds(fsrs.next_interval_for_state(expected, 0.9))
        );
        assert_eq!(languages.cards[1].state, CardState::default());

        let german = export(path, Some("Languages::German"));
        assert_eq!(german.len(), 1);
        assert_eq!(german[0].name, "Languages::German");
        assert_eq!(german[0].cards.len(), 2);
    }

    #[test]
    fn exports_current_schema_collection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.anki2");
        current_schema(&path);
        check_collection(&path);
    }

    #[test]
    fn exports_legacy_schema_collection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.anki2");
        legacy_schema(&path);
        check_collection(&path);
    }

    #[test]
    fn unknown_deck_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.anki2");
        legacy_schema(&path);
        let fsrs = FSRS::default();
        assert!(
            build_decks(
                &Collection::open(&path).unwrap(),
                &ExportOptions {
                    deck: Some("Nope"),
                    with_progress: true,
                    fsrs: &fsrs,
                    desired_retention: 0.9,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn interval_seconds_matches_device_rules() {
        assert_eq!(interval_seconds(0.0), 60);
        assert_eq!(interval_seconds(f32::NAN), 60);
        assert_eq!(interval_seconds(0.0001), 60);
        assert_eq!(interval_seconds(1.0), 86_400);
        assert_eq!(interval_seconds(1e9), u32::MAX);
    }

    #[test]
    fn file_stems_are_fat_safe() {
        assert_eq!(file_stem("Languages::German"), "Languages - German");
        assert_eq!(file_stem("a/b?c*"), "a_b_c_");
        assert_eq!(file_stem("  ...  "), "Deck");
        assert_eq!(file_stem("Notes."), "Notes");
    }

    #[test]
    fn write_deck_protects_device_progress() {
        let dir = tempfile::tempdir().unwrap();
        let deck = DeckExport {
            name: "Languages::German".into(),
            cards: vec![card(1, "a", "b")],
            ..Default::default()
        };
        let path = write_deck(dir.path(), &deck, false).unwrap();
        assert_eq!(path.file_name().unwrap(), "Languages - German.inki");
        let file = DeckFile::decode(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(file.anki_deck, "Languages::German");
        assert_eq!(file.cards, deck.cards);
        assert!(write_deck(dir.path(), &deck, false).is_err());
        write_deck(dir.path(), &deck, true).unwrap();
    }
}
