//! The `.inki` deck file: one file per deck on the reader's SD card, in
//! `/flashcards/<deck>.inki`. Read by `lib/Flashcards` in the firmware and
//! documented in its `docs/flashcards.md`. All integers are little-endian.
//!
//! ```text
//! header        32 bytes (below)
//! deck name     UTF-8 Anki deck name, name_length bytes
//! card index    card_count × 16 bytes: u64 Anki card id, u32 text offset, u32 text length
//! card text     per card: question, NUL, answer
//! state         card_count × 20 bytes at state_offset, updated in place by the reader
//! answer log    32-byte entries from log_offset to the end, appended by the reader
//! ```
//!
//! Header: `INKD`, u16 version, u16 flags, u32 card_count, u32 index_offset,
//! u32 state_offset, u32 log_offset, u16 name_length, u16 reserved,
//! u32 reserved.

use anyhow::{Context, Result, ensure};

use crate::deck_export::{CardState, ExportCard};

pub const EXTENSION: &str = "inki";
const MAGIC: &[u8; 4] = b"INKD";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 32;
const INDEX_ENTRY_SIZE: usize = 16;
const STATE_RECORD_SIZE: usize = 20;
const LOG_ENTRY_SIZE: usize = 32;
/// The reader reads at most this many bytes of a card's text.
const MAX_CARD_TEXT: usize = 4096;
const MAX_FRONT_TEXT: usize = 2047;
const MAX_NAME: usize = 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct DeckFile {
    /// The Anki deck the file was made from, e.g. `Languages::German`.
    pub anki_deck: String,
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
        let index_offset = HEADER_SIZE + name.len();
        let text_start = index_offset + count * INDEX_ENTRY_SIZE;
        let text_len: usize = texts.iter().map(Vec::len).sum();
        // Records start on a 4-byte boundary.
        let state_offset = (text_start + text_len).next_multiple_of(4);
        let log_offset = state_offset + count * STATE_RECORD_SIZE;
        let u32_of = |n: usize| u32::try_from(n).context("deck too large");

        let mut out = Vec::with_capacity(log_offset + self.answer_log.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for n in [count, index_offset, state_offset, log_offset] {
            out.extend_from_slice(&u32_of(n)?.to_le_bytes());
        }
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(name.as_bytes());

        let mut offset = text_start;
        for (card, text) in self.cards.iter().zip(&texts) {
            out.extend_from_slice(&(card.anki_id as u64).to_le_bytes());
            out.extend_from_slice(&u32_of(offset)?.to_le_bytes());
            out.extend_from_slice(&u32_of(text.len())?.to_le_bytes());
            offset += text.len();
        }
        for text in &texts {
            out.extend_from_slice(text);
        }
        out.resize(state_offset, 0);
        for card in &self.cards {
            let s = card.state;
            out.extend_from_slice(&s.stability.to_le_bytes());
            out.extend_from_slice(&s.difficulty.to_le_bytes());
            out.extend_from_slice(&s.stability_fast.to_le_bytes());
            out.extend_from_slice(&s.last_review.to_le_bytes());
            out.extend_from_slice(&s.due.to_le_bytes());
        }
        let whole_entries = self.answer_log.len() / LOG_ENTRY_SIZE * LOG_ENTRY_SIZE;
        out.extend_from_slice(&self.answer_log[..whole_entries]);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= HEADER_SIZE && &bytes[..4] == MAGIC,
            "not an .inki deck file"
        );
        let version = u16_at(bytes, 4);
        ensure!(
            version == VERSION,
            "deck file version {version} is not supported"
        );
        let count = u32_at(bytes, 8) as usize;
        let index_offset = u32_at(bytes, 12) as usize;
        let state_offset = u32_at(bytes, 16) as usize;
        let log_offset = u32_at(bytes, 20) as usize;
        let name_len = usize::from(u16_at(bytes, 24));
        ensure!(
            index_offset >= HEADER_SIZE + name_len
                && state_offset >= index_offset + count * INDEX_ENTRY_SIZE
                && log_offset == state_offset + count * STATE_RECORD_SIZE
                && bytes.len() >= log_offset,
            "the deck file is damaged"
        );
        let anki_deck =
            String::from_utf8_lossy(&bytes[HEADER_SIZE..HEADER_SIZE + name_len]).into_owned();

        let cards = (0..count)
            .map(|i| {
                let e = index_offset + i * INDEX_ENTRY_SIZE;
                let text_offset = u32_at(bytes, e + 8) as usize;
                let text_len = u32_at(bytes, e + 12) as usize;
                let text = bytes
                    .get(text_offset..text_offset + text_len)
                    .context("the deck file is damaged")?;
                let text = String::from_utf8_lossy(text);
                let (front, back) = text.split_once('\0').unwrap_or((&text, ""));
                let r = state_offset + i * STATE_RECORD_SIZE;
                Ok(ExportCard {
                    anki_id: u64::from_le_bytes(bytes[e..e + 8].try_into()?) as i64,
                    front: front.to_string(),
                    back: back.to_string(),
                    state: CardState {
                        stability: f32_at(bytes, r),
                        difficulty: f32_at(bytes, r + 4),
                        stability_fast: f32_at(bytes, r + 8),
                        last_review: u32_at(bytes, r + 12),
                        due: u32_at(bytes, r + 16),
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // A partly written last entry (power lost mid-answer) is dropped.
        let log = &bytes[log_offset..];
        let whole = log.len() / LOG_ENTRY_SIZE * LOG_ENTRY_SIZE;
        Ok(Self {
            anki_deck,
            cards,
            answer_log: log[..whole].to_vec(),
        })
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
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
        DeckFile {
            anki_deck: "Languages::German".into(),
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
        assert_eq!(u16_at(&bytes, 4), 1);
        assert_eq!(u32_at(&bytes, 8), 2);
        let name_len = usize::from(u16_at(&bytes, 24));
        assert_eq!(&bytes[32..32 + name_len], b"Languages::German");
        let index = u32_at(&bytes, 12) as usize;
        assert_eq!(index, 32 + name_len);
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
