//! Updates a deck file from the reader with a newer Anki export.
//!
//! The reader's progress is the truth for cards it already has: their order,
//! scheduling state and the answer log are kept, and only their text is
//! refreshed. Cards added in Anki are appended (the reader relies on
//! existing cards keeping their position), starting from their Anki progress.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::anki_model;
use crate::collection::Collection;
use crate::deck_export::{
    ExportCard, ExportOptions, GroupedCard, card_state, collect_cards, device_order, file_stem,
};
use crate::deck_file::DeckFile;
use crate::revlog::card_histories;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct UpdateSummary {
    pub anki_deck: String,
    pub cards: usize,
    /// Cards the reader has studied; their progress is kept.
    pub studied_on_reader: usize,
    /// Cards added in Anki since the deck was made.
    pub new_cards: usize,
    /// Cards whose text was edited in Anki.
    pub text_changes: usize,
    /// Cards deleted in Anki; they stay on the reader.
    pub missing_in_anki: usize,
}

/// The Anki deck a deck file was made from: recorded in the file, or else
/// the deck whose file name matches `stem`.
pub fn anki_deck_for(collection: &Collection, stem: &str, deck: &DeckFile) -> Result<String> {
    if !deck.anki_deck.is_empty() {
        return Ok(deck.anki_deck.clone());
    }
    anki_model::deck_names(collection.conn())?
        .into_values()
        .find(|name| file_stem(name) == stem)
        .with_context(|| format!("cannot tell which Anki deck {stem:?} was made from"))
}

pub fn update_deck(
    collection: &Collection,
    stem: &str,
    deck: &DeckFile,
    options: &ExportOptions,
) -> Result<(DeckFile, UpdateSummary)> {
    let anki_deck = anki_deck_for(collection, stem, deck)?;
    let grouped = collect_cards(collection.conn(), Some(&anki_deck))?;
    let by_id: HashMap<i64, &GroupedCard> = grouped.iter().map(|g| (g.row.id, g)).collect();
    let mut summary = UpdateSummary {
        anki_deck: anki_deck.clone(),
        studied_on_reader: deck.studied(),
        ..Default::default()
    };

    let mut cards = Vec::with_capacity(deck.cards.len());
    for card in &deck.cards {
        let mut card = card.clone();
        match by_id.get(&card.anki_id).map(|g| g.text.as_ref()) {
            None => summary.missing_in_anki += 1,
            Some(Some(t)) if t.front != card.front || t.back != card.back => {
                summary.text_changes += 1;
                card.front = t.front.clone();
                card.back = t.back.clone();
            }
            Some(_) => {}
        }
        cards.push(card);
    }

    let on_reader: HashSet<i64> = deck.cards.iter().map(|c| c.anki_id).collect();
    let mut added: Vec<&GroupedCard> = grouped
        .iter()
        .filter(|g| !on_reader.contains(&g.row.id) && g.row.queue != -1 && g.text.is_some())
        .collect();
    if !added.is_empty() {
        added.sort_by_key(|g| device_order(&g.row));
        let histories: HashMap<i64, _> = if options.with_progress {
            card_histories(collection.revlog()?)
                .into_iter()
                .map(|h| (h.card_id, h))
                .collect()
        } else {
            HashMap::new()
        };
        for g in added {
            let text = g.text.as_ref().expect("filtered on text");
            cards.push(ExportCard {
                anki_id: g.row.id,
                front: text.front.clone(),
                back: text.back.clone(),
                state: card_state(&g.row, histories.get(&g.row.id), options),
            });
            summary.new_cards += 1;
        }
    }
    summary.cards = cards.len();
    Ok((
        DeckFile {
            anki_deck,
            cards,
            answer_log: deck.answer_log.clone(),
        },
        summary,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck_export::tests::current_schema;
    use crate::deck_export::{CardState, build_decks};
    use fsrs::FSRS;

    fn options(fsrs: &FSRS) -> ExportOptions<'_> {
        ExportOptions {
            deck: None,
            with_progress: true,
            fsrs,
            desired_retention: 0.9,
        }
    }

    /// Exports "Languages" (cards 11, 21, 12) and returns it as the reader
    /// would hand it back: card 12 studied, two answers in the log.
    fn reader_copy(collection: &Collection) -> DeckFile {
        let fsrs = FSRS::default();
        let mut decks = build_decks(collection, &options(&fsrs)).unwrap();
        let deck = decks.remove(0);
        assert_eq!(deck.name, "Languages");
        let mut file = DeckFile {
            anki_deck: deck.name,
            cards: deck.cards,
            answer_log: vec![7; 64],
        };
        file.cards[2].state = CardState {
            stability: 4.0,
            difficulty: 5.0,
            stability_fast: 3.0,
            last_review: 1_800_000_000,
            due: 1_800_300_000,
        };
        DeckFile::decode(&file.encode().unwrap()).unwrap()
    }

    #[test]
    fn keeps_reader_progress_and_appends_new_cards() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.anki2");
        current_schema(&path);
        let on_reader = reader_copy(&Collection::open(&path).unwrap());

        // In Anki: edit note 1 and add a card to Languages::German.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "UPDATE notes SET flds = 'Hund' || char(31) || 'dog (animal)' WHERE id = 1;
             INSERT INTO notes VALUES (4, 'd', 100, 0, 0, '', 'Maus' || char(31) || 'mouse', '', 0, 0, '');
             INSERT INTO cards VALUES (41, 4, 2, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, '');",
        )
        .unwrap();
        drop(conn);

        let collection = Collection::open(&path).unwrap();
        let fsrs = FSRS::default();
        let (updated, summary) =
            update_deck(&collection, "Languages", &on_reader, &options(&fsrs)).unwrap();
        assert_eq!(
            summary,
            UpdateSummary {
                anki_deck: "Languages".into(),
                cards: 4,
                studied_on_reader: 2,
                new_cards: 1,
                text_changes: 2,
                missing_in_anki: 0,
            }
        );
        let ids: Vec<i64> = updated.cards.iter().map(|c| c.anki_id).collect();
        assert_eq!(ids, [11, 21, 12, 41], "existing order kept, new card last");
        assert_eq!(updated.cards[0].back, "dog (animal)");
        assert_eq!(
            updated.cards[2].state, on_reader.cards[2].state,
            "progress kept"
        );
        assert_eq!(updated.cards[3].state, CardState::default());
        assert_eq!(updated.answer_log, on_reader.answer_log, "answer log kept");
    }

    #[test]
    fn finds_deck_by_file_name_when_unnamed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("collection.anki2");
        current_schema(&path);
        let collection = Collection::open(&path).unwrap();
        let mut deck = reader_copy(&collection);
        deck.anki_deck.clear();
        assert_eq!(
            anki_deck_for(&collection, "Languages - German", &deck).unwrap(),
            "Languages::German"
        );
        assert!(anki_deck_for(&collection, "Unknown", &deck).is_err());
    }
}
