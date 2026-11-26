//! Whole `.inki` deck files, built from and read with the fixed-size pieces
//! in `inki_core::format`, which the reader's firmware uses too. The layout
//! is documented there and in the firmware fork's `docs/flashcards.md`.

use anyhow::{Context, Result, ensure};
use inki_core::format::{
    CardIndexEntry, DeckHeader, DeckSettings, HEADER_SIZE, INDEX_ENTRY_SIZE, MAX_CARD_TEXT,
    REVIEW_LOG_ENTRY_SIZE as LOG_ENTRY_SIZE, SETTINGS_SIZE, STATE_RECORD_SIZE, StateRecord,
    VERSION, split_card_text,
};
use inki_core::fsrs7::MemoryState;

use crate::deck_export::{CardState, ExportCard};

pub use inki_core::format::EXTENSION;

const MAX_FRONT_TEXT: usize = 2047;
const MAX_NAME: usize = 1024;

impl From<CardState> for StateRecord {
    fn from(s: CardState) -> Self {
        Self {
            memory: MemoryState {
                stability: s.stability,
                difficulty: s.difficulty,
                stability_fast: s.stability_fast,
            },
            last_review: s.last_review,
            due: s.due,
        }
    }
}

impl From<StateRecord> for CardState {
    fn from(r: StateRecord) -> Self {
        Self {
            stability: r.memory.stability,
            difficulty: r.memory.difficulty,
            stability_fast: r.memory.stability_fast,
            last_review: r.last_review,
            due: r.due,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeckFile {
    /// The Anki deck the file was made from, e.g. `Languages::German`.
    pub anki_deck: String,
    pub settings: DeckSettings,
    pub cards: Vec<ExportCard>,
    /// Answers given on the reader (raw 32-byte entries). Kept when a deck is
    /// updated so the reader's daily limits still count them.
    pub answer_log: Vec<u8>,
}

impl DeckFile {
    pub fn studied(&self) -> usize {
        self.cards
            .iter()
            .filter(|c| c.state.last_review != 0)
            .count()
    }

    pub fn answers(&self) -> usize {
        self.answer_log.len() / LOG_ENTRY_SIZE
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let name = truncate(&self.anki_deck, MAX_NAME);
        let texts: Vec<Vec<u8>> = self.cards.iter().map(card_text).collect();
        let count = self.cards.len();
        let index_offset = HEADER_SIZE + SETTINGS_SIZE + name.len();
        let text_start = index_offset + count * INDEX_ENTRY_SIZE;
        let text_len: usize = texts.iter().map(Vec::len).sum();
        // Records start on a 4-byte boundary.
        let state_offset = (text_start + text_len).next_multiple_of(4);
        let log_offset = state_offset + count * STATE_RECORD_SIZE;
        let u32_of = |n: usize| u32::try_from(n).context("deck too large");

        let header = DeckHeader {
            card_count: u32_of(count)?,
            index_offset: u32_of(index_offset)?,
            state_offset: u32_of(state_offset)?,
            log_offset: u32_of(log_offset)?,
            name_length: name.len() as u16,
            version: VERSION,
        };
        let mut out = Vec::with_capacity(log_offset + self.answer_log.len());
        out.extend_from_slice(&header.encode());
        out.extend_from_slice(&self.settings.encode());
        out.extend_from_slice(name.as_bytes());
        let mut offset = text_start;
        for (card, text) in self.cards.iter().zip(&texts) {
            let entry = CardIndexEntry {
                anki_card_id: card.anki_id as u64,
                text_offset: u32_of(offset)?,
                text_length: u32_of(text.len())?,
            };
            out.extend_from_slice(&entry.encode());
            offset += text.len();
        }
        for text in &texts {
            out.extend_from_slice(text);
        }
        out.resize(state_offset, 0);
        for card in &self.cards {
            out.extend_from_slice(&StateRecord::from(card.state).encode());
        }
        let whole_entries = self.answer_log.len() / LOG_ENTRY_SIZE * LOG_ENTRY_SIZE;
        out.extend_from_slice(&self.answer_log[..whole_entries]);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let header_bytes: &[u8; HEADER_SIZE] = bytes
            .get(..HEADER_SIZE)
            .and_then(|b| b.try_into().ok())
            .context("not an .inki deck file")?;
        ensure!(
            header_bytes[..4] == inki_core::format::MAGIC,
            "not an .inki deck file"
        );
        let h = DeckHeader::decode(header_bytes, bytes.len() as u64)
            .context("the deck file is damaged or from a newer version of inki")?;
        // Version 1 decks have no settings and get the defaults.
        let settings = h
            .settings_offset()
            .map_or_else(DeckSettings::default, |at| {
                let at = at as usize;
                DeckSettings::decode(bytes[at..at + SETTINGS_SIZE].try_into().expect("size"))
            });
        let name_start = h.name_offset() as usize;
        let name_end = name_start + usize::from(h.name_length);
        let anki_deck = String::from_utf8_lossy(&bytes[name_start..name_end]).into_owned();

        let cards = (0..h.card_count as usize)
            .map(|i| {
                let at = h.index_offset as usize + i * INDEX_ENTRY_SIZE;
                let entry = CardIndexEntry::decode(bytes[at..at + INDEX_ENTRY_SIZE].try_into()?);
                let start = entry.text_offset as usize;
                let text = bytes
                    .get(start..start + entry.text_length as usize)
                    .context("the deck file is damaged")?;
                let (front, back) = split_card_text(text);
                let at = h.state_offset as usize + i * STATE_RECORD_SIZE;
                let state = StateRecord::decode(bytes[at..at + STATE_RECORD_SIZE].try_into()?);
                Ok(ExportCard {
                    anki_id: entry.anki_card_id as i64,
                    front: String::from_utf8_lossy(front).into_owned(),
                    back: String::from_utf8_lossy(back).into_owned(),
                    state: state.into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // A partly written last entry (power lost mid-answer) is dropped.
        let log_end = h.review_log_append_offset(bytes.len() as u64) as usize;
        Ok(Self {
            anki_deck,
            settings,
            cards,
            answer_log: bytes[h.log_offset as usize..log_end].to_vec(),
        })
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary, marking the cut.
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Question, NUL, answer; cut to fit the reader's card buffer.
fn card_text(card: &ExportCard) -> Vec<u8> {
    let front = truncate(&card.front.replace('\0', ""), MAX_FRONT_TEXT);
    let back = truncate(
        &card.back.replace('\0', ""),
        MAX_CARD_TEXT - front.len() - 1,
    );
    let mut text = front.into_bytes();
    text.push(0);
    text.extend_from_slice(back.as_bytes());
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn card(id: i64, front: &str, back: &str, last_review: u32) -> ExportCard {
        ExportCard {
            anki_id: id,
            front: front.into(),
            back: back.into(),
            state: CardState {
                stability: if last_review == 0 { 0.0 } else { 2.5 },
                difficulty: if last_review == 0 { 0.0 } else { 5.0 },
                stability_fast: if last_review == 0 { 0.0 } else { 2.0 },
                last_review,
                due: if last_review == 0 {
                    0
                } else {
                    last_review + 86_400
                },
            },
        }
    }

    fn deck() -> DeckFile {
        let mut settings = DeckSettings {
            new_per_day: 7,
            ..Default::default()
        };
        settings.params[0] = 0.5;
        DeckFile {
            anki_deck: "Languages::German".into(),
            settings,
            cards: vec![card(7, "Hund", "dog", 1000), card(9, "é", "", 0)],
            answer_log: vec![1; 64],
        }
    }

    #[test]
    fn round_trips() {
        let d = deck();
        let decoded = DeckFile::decode(&d.encode().unwrap()).unwrap();
        assert_eq!(decoded, d);
        assert_eq!(decoded.studied(), 1);
        assert_eq!(decoded.answers(), 2);
    }

    #[test]
    fn layout_matches_the_firmware() {
        let bytes = deck().encode().unwrap();
        assert_eq!(&bytes[..4], b"INKD");
        assert_eq!(u16_at(&bytes, 4), 2);
        assert_eq!(u32_at(&bytes, 8), 2);
        assert_eq!(u16_at(&bytes, 32), 7, "new cards per day");
        assert_eq!(u16_at(&bytes, 34), 200, "reviews per day");
        assert_eq!(&bytes[36..40], &0.9f32.to_le_bytes());
        assert_eq!(&bytes[40..44], &0.5f32.to_le_bytes(), "first parameter");
        let name_len = usize::from(u16_at(&bytes, 24));
        assert_eq!(&bytes[176..176 + name_len], b"Languages::German");
        let index = u32_at(&bytes, 12) as usize;
        assert_eq!(index, 176 + name_len);
        let text_offset = u32_at(&bytes, index + 8) as usize;
        let text_len = u32_at(&bytes, index + 12) as usize;
        assert_eq!(&bytes[text_offset..text_offset + text_len], b"Hund\0dog");
        let state = u32_at(&bytes, 16) as usize;
        assert_eq!(state % 4, 0);
        assert_eq!(u32_at(&bytes, state + 12), 1000);
        assert!(
            bytes[state + 20..state + 40].iter().all(|&b| b == 0),
            "new card"
        );
        let log = u32_at(&bytes, 20) as usize;
        assert_eq!(log, state + 40);
        assert_eq!(bytes.len(), log + 64);
    }

    #[test]
    fn reads_version_1_with_default_settings() {
        let v2 = deck().encode().unwrap();
        // The same deck as version 1: no settings block, offsets 144 lower.
        let mut v1 = v2[..32].to_vec();
        v1.extend_from_slice(&v2[32 + SETTINGS_SIZE..]);
        v1[4] = 1;
        let shift = |at: usize, v1: &mut Vec<u8>| {
            let value = u32_at(v1, at) - SETTINGS_SIZE as u32;
            v1[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        for at in [12, 16, 20] {
            shift(at, &mut v1);
        }
        for card in 0..2 {
            shift(u32_at(&v1, 12) as usize + card * 16 + 8, &mut v1);
        }
        let decoded = DeckFile::decode(&v1).unwrap();
        assert_eq!(decoded.settings, DeckSettings::default());
        assert_eq!(
            DeckFile {
                settings: deck().settings,
                ..decoded
            },
            deck()
        );
    }

    #[test]
    fn drops_partial_log_entry() {
        let mut bytes = deck().encode().unwrap();
        bytes.extend_from_slice(&[9; 5]);
        assert_eq!(DeckFile::decode(&bytes).unwrap().answers(), 2);
    }

    #[test]
    fn rejects_damage() {
        let bytes = deck().encode().unwrap();
        assert!(DeckFile::decode(&bytes[..40]).is_err());
        assert!(DeckFile::decode(b"INKC0000000000000000000000000000").is_err());
        let mut wrong_version = bytes.clone();
        wrong_version[4] = 9;
        assert!(DeckFile::decode(&wrong_version).is_err());
    }

    #[test]
    fn long_text_fits_the_reader_buffer() {
        let long = "ü".repeat(5000);
        let d = DeckFile {
            anki_deck: "x".into(),
            settings: DeckSettings::default(),
            cards: vec![card(1, &long, &long, 0)],
            answer_log: Vec::new(),
        };
        let decoded = DeckFile::decode(&d.encode().unwrap()).unwrap();
        let c = &decoded.cards[0];
        assert!(c.front.len() <= MAX_FRONT_TEXT && c.front.ends_with('…'));
        assert!(c.front.len() + 1 + c.back.len() <= MAX_CARD_TEXT);
        assert!(c.back.ends_with('…'));
    }
}
