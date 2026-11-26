//! The `.inki` deck file: one file per deck in the SD card's `flashcards`
//! folder. All integers are little-endian.
//!
//! ```text
//! header        32 bytes (DeckHeader)
//! settings      144 bytes (DeckSettings): daily limits and FSRS-7 parameters
//! deck name     the Anki deck name, UTF-8, name_length bytes
//! card index    card_count × 16 bytes at index_offset (CardIndexEntry)
//! card text     per card: question, NUL, answer
//! state         card_count × 20 bytes at state_offset (StateRecord), updated in place
//! answer log    32-byte entries (ReviewLogEntry) from log_offset to the end, appended
//! ```
//!
//! The reader only overwrites state records and appends to the log, so it
//! never rewrites the file. This module holds the fixed-size pieces; the PC
//! tool assembles whole files from them.
//!
//! Version 1 files have no settings block (the name follows the header) and
//! are read with the default settings.

use crate::fsrs7::{
    DEFAULT_DESIRED_RETENTION, DEFAULT_PARAMS, MemoryState, PARAM_COUNT, Params, clip_params,
    valid_retention,
};

pub const EXTENSION: &str = "inki";
pub const MAGIC: [u8; 4] = *b"INKD";
/// The version written. Version 1 is still read.
pub const VERSION: u16 = 2;
pub const HEADER_SIZE: usize = 32;
pub const SETTINGS_SIZE: usize = 8 + 4 * PARAM_COUNT;
pub const DEFAULT_NEW_PER_DAY: u16 = 20;
pub const DEFAULT_REVIEWS_PER_DAY: u16 = 200;
pub const INDEX_ENTRY_SIZE: usize = 16;
pub const STATE_RECORD_SIZE: usize = 20;
pub const REVIEW_LOG_ENTRY_SIZE: usize = 32;
/// The reader reads at most this many bytes of a card's text.
pub const MAX_CARD_TEXT: usize = 4096;
/// Review-log flag: the card was new when answered.
pub const REVIEW_FLAG_WAS_NEW: u8 = 0x01;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckHeader {
    pub card_count: u32,
    pub index_offset: u32,
    pub state_offset: u32,
    /// Always `state_offset + card_count * STATE_RECORD_SIZE`.
    pub log_offset: u32,
    pub name_length: u16,
    /// [`VERSION`] when writing; 1 or 2 when read.
    pub version: u16,
}

/// How a deck is studied, chosen on the PC.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckSettings {
    /// New cards introduced per study day.
    pub new_per_day: u16,
    /// Reviews of studied cards per study day (relearning is not limited).
    pub reviews_per_day: u16,
    pub desired_retention: f32,
    /// FSRS-7 parameters: the defaults, or trained on the user's reviews.
    pub params: Params,
}

impl Default for DeckSettings {
    fn default() -> Self {
        Self {
            new_per_day: DEFAULT_NEW_PER_DAY,
            reviews_per_day: DEFAULT_REVIEWS_PER_DAY,
            desired_retention: DEFAULT_DESIRED_RETENTION,
            params: DEFAULT_PARAMS,
        }
    }
}

/// Where a card's text is: `text_offset` from the start of the file.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CardIndexEntry {
    pub anki_card_id: u64,
    pub text_offset: u32,
    pub text_length: u32,
}

/// Scheduling state of one card. All zero means a new card.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StateRecord {
    pub memory: MemoryState,
    /// Unix seconds (UTC); 0 for a new card.
    pub last_review: u32,
    /// Unix seconds (UTC).
    pub due: u32,
}

impl StateRecord {
    pub fn is_new(&self) -> bool {
        self.last_review == 0
    }
}

/// One answer given on the reader.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReviewLogEntry {
    pub anki_card_id: u64,
    pub card_index: u32,
    /// Unix seconds (UTC).
    pub review_time: u32,
    /// 1 Again .. 4 Easy.
    pub rating: u8,
    /// [`REVIEW_FLAG_WAS_NEW`].
    pub flags: u8,
    pub answer_seconds: u16,
    /// Since the previous answer; 0 for a new card.
    pub elapsed_days: f32,
    /// After the answer.
    pub stability: f32,
    /// After the answer.
    pub difficulty: f32,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | (u64::from(u32_at(b, at + 4)) << 32)
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

impl DeckHeader {
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        put(&mut out, 0, &MAGIC);
        put(&mut out, 4, &self.version.to_le_bytes());
        put(&mut out, 8, &self.card_count.to_le_bytes());
        put(&mut out, 12, &self.index_offset.to_le_bytes());
        put(&mut out, 16, &self.state_offset.to_le_bytes());
        put(&mut out, 20, &self.log_offset.to_le_bytes());
        put(&mut out, 24, &self.name_length.to_le_bytes());
        out
    }

    /// Decodes and checks a header: magic, version, and that every region
    /// fits in a file of `file_size` bytes.
    pub fn decode(bytes: &[u8; HEADER_SIZE], file_size: u64) -> Option<Self> {
        let version = u16_at(bytes, 4);
        if bytes[..4] != MAGIC || !(1..=VERSION).contains(&version) {
            return None;
        }
        let h = Self {
            version,
            card_count: u32_at(bytes, 8),
            index_offset: u32_at(bytes, 12),
            state_offset: u32_at(bytes, 16),
            log_offset: u32_at(bytes, 20),
            name_length: u16_at(bytes, 24),
        };
        // u64 arithmetic: a damaged count must not wrap around.
        let count = u64::from(h.card_count);
        let fits = u64::from(h.index_offset)
            >= u64::from(h.name_offset()) + u64::from(h.name_length)
            && u64::from(h.state_offset)
                >= u64::from(h.index_offset) + count * INDEX_ENTRY_SIZE as u64
            && u64::from(h.log_offset)
                == u64::from(h.state_offset) + count * STATE_RECORD_SIZE as u64
            && u64::from(h.log_offset) <= file_size;
        fits.then_some(h)
    }

    /// Where the settings block is; `None` for version 1, which has none.
    pub fn settings_offset(&self) -> Option<u32> {
        (self.version >= 2).then_some(HEADER_SIZE as u32)
    }

    /// Where the deck name is.
    pub fn name_offset(&self) -> u32 {
        match self.settings_offset() {
            Some(at) => at + SETTINGS_SIZE as u32,
            None => HEADER_SIZE as u32,
        }
    }

    /// Complete answer-log entries in a file of `file_size` bytes. A partly
    /// written last entry (power lost mid-write) is not counted.
    pub fn review_log_entries(&self, file_size: u64) -> u64 {
        file_size.saturating_sub(u64::from(self.log_offset)) / REVIEW_LOG_ENTRY_SIZE as u64
    }

    /// Where the next answer is appended: after the last complete entry, so
    /// a partly written one is overwritten.
    pub fn review_log_append_offset(&self, file_size: u64) -> u64 {
        u64::from(self.log_offset)
            + self.review_log_entries(file_size) * REVIEW_LOG_ENTRY_SIZE as u64
    }
}

impl DeckSettings {
    pub fn encode(&self) -> [u8; SETTINGS_SIZE] {
        let mut out = [0u8; SETTINGS_SIZE];
        put(&mut out, 0, &self.new_per_day.to_le_bytes());
        put(&mut out, 2, &self.reviews_per_day.to_le_bytes());
        put(&mut out, 4, &self.desired_retention.to_le_bytes());
        for (i, p) in self.params.iter().enumerate() {
            put(&mut out, 8 + 4 * i, &p.to_le_bytes());
        }
        out
    }

    /// Decodes a settings block. A retention FSRS-7 cannot use, or any
    /// parameter that is not a number, falls back to the default; parameters
    /// are clipped to the ranges FSRS-7 accepts.
    pub fn decode(bytes: &[u8; SETTINGS_SIZE]) -> Self {
        let mut params = [0.0f32; PARAM_COUNT];
        for (i, p) in params.iter_mut().enumerate() {
            *p = f32_at(bytes, 8 + 4 * i);
        }
        if params.iter().all(|p| p.is_finite()) {
            clip_params(&mut params);
        } else {
            params = DEFAULT_PARAMS;
        }
        let retention = f32_at(bytes, 4);
        Self {
            new_per_day: u16_at(bytes, 0),
            reviews_per_day: u16_at(bytes, 2),
            desired_retention: if valid_retention(retention) {
                retention
            } else {
                DEFAULT_DESIRED_RETENTION
            },
            params,
        }
    }

    /// Whether the parameters are FSRS-7's defaults rather than trained ones.
    pub fn uses_default_params(&self) -> bool {
        self.params == DEFAULT_PARAMS
    }
}

impl CardIndexEntry {
    pub fn encode(&self) -> [u8; INDEX_ENTRY_SIZE] {
        let mut out = [0u8; INDEX_ENTRY_SIZE];
        put(&mut out, 0, &self.anki_card_id.to_le_bytes());
        put(&mut out, 8, &self.text_offset.to_le_bytes());
        put(&mut out, 12, &self.text_length.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; INDEX_ENTRY_SIZE]) -> Self {
        Self {
            anki_card_id: u64_at(bytes, 0),
            text_offset: u32_at(bytes, 8),
            text_length: u32_at(bytes, 12),
        }
    }
}

impl StateRecord {
    pub fn encode(&self) -> [u8; STATE_RECORD_SIZE] {
        let mut out = [0u8; STATE_RECORD_SIZE];
        put(&mut out, 0, &self.memory.stability.to_le_bytes());
        put(&mut out, 4, &self.memory.difficulty.to_le_bytes());
        put(&mut out, 8, &self.memory.stability_fast.to_le_bytes());
        put(&mut out, 12, &self.last_review.to_le_bytes());
        put(&mut out, 16, &self.due.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; STATE_RECORD_SIZE]) -> Self {
        Self {
            memory: MemoryState {
                stability: f32_at(bytes, 0),
                difficulty: f32_at(bytes, 4),
                stability_fast: f32_at(bytes, 8),
            },
            last_review: u32_at(bytes, 12),
            due: u32_at(bytes, 16),
        }
    }
}

impl ReviewLogEntry {
    pub fn encode(&self) -> [u8; REVIEW_LOG_ENTRY_SIZE] {
        let mut out = [0u8; REVIEW_LOG_ENTRY_SIZE];
        put(&mut out, 0, &self.anki_card_id.to_le_bytes());
        put(&mut out, 8, &self.card_index.to_le_bytes());
        put(&mut out, 12, &self.review_time.to_le_bytes());
        out[16] = self.rating;
        out[17] = self.flags;
        put(&mut out, 18, &self.answer_seconds.to_le_bytes());
        put(&mut out, 20, &self.elapsed_days.to_le_bytes());
        put(&mut out, 24, &self.stability.to_le_bytes());
        put(&mut out, 28, &self.difficulty.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; REVIEW_LOG_ENTRY_SIZE]) -> Self {
        Self {
            anki_card_id: u64_at(bytes, 0),
            card_index: u32_at(bytes, 8),
            review_time: u32_at(bytes, 12),
            rating: bytes[16],
            flags: bytes[17],
            answer_seconds: u16_at(bytes, 18),
            elapsed_days: f32_at(bytes, 20),
            stability: f32_at(bytes, 24),
            difficulty: f32_at(bytes, 28),
        }
    }
}

/// Splits card text into question and answer at the first NUL. Text without
/// a NUL is all question.
pub fn split_card_text(text: &[u8]) -> (&[u8], &[u8]) {
    match text.iter().position(|&b| b == 0) {
        Some(nul) => (&text[..nul], &text[nul + 1..]),
        None => (text, &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> DeckHeader {
        // 3 cards, 5-byte name, 100 bytes of text.
        let index_offset = 32 + 144 + 5;
        let state_offset = index_offset + 3 * 16 + 100;
        DeckHeader {
            card_count: 3,
            index_offset,
            state_offset,
            log_offset: state_offset + 3 * 20,
            name_length: 5,
            version: VERSION,
        }
    }

    #[test]
    fn header_round_trips_and_validates() {
        let h = header();
        let bytes = h.encode();
        assert_eq!(&bytes[..4], b"INKD");
        assert_eq!(DeckHeader::decode(&bytes, u64::from(h.log_offset)), Some(h));
        assert_eq!(
            DeckHeader::decode(&bytes, u64::from(h.log_offset) - 1),
            None
        );

        let mut bad = h;
        bad.log_offset += 4;
        assert_eq!(DeckHeader::decode(&bad.encode(), 1 << 20), None);
        bad = h;
        bad.card_count = u32::MAX;
        assert_eq!(DeckHeader::decode(&bad.encode(), u64::MAX), None);
        let mut version = bytes;
        version[4] = 9;
        assert_eq!(DeckHeader::decode(&version, 1 << 20), None);
    }

    #[test]
    fn reads_version_1_without_settings() {
        let mut h = header();
        assert_eq!(h.settings_offset(), Some(32));
        assert_eq!(h.name_offset(), 176);
        h.version = 1;
        assert_eq!(h.settings_offset(), None);
        assert_eq!(h.name_offset(), 32);
        assert_eq!(DeckHeader::decode(&h.encode(), 1 << 20), Some(h));
        // Version 2 needs room for the settings block before the name.
        let mut v2 = h;
        v2.version = 2;
        v2.index_offset = 32 + 5;
        assert_eq!(DeckHeader::decode(&v2.encode(), 1 << 20), None);
    }

    #[test]
    fn settings_round_trip_and_fall_back() {
        let mut s = DeckSettings {
            new_per_day: 5,
            reviews_per_day: 999,
            desired_retention: 0.85,
            params: DEFAULT_PARAMS,
        };
        s.params[0] = 0.5;
        let bytes = s.encode();
        assert_eq!(u16_at(&bytes, 2), 999);
        assert_eq!(DeckSettings::decode(&bytes), s);
        assert!(!s.uses_default_params());
        assert!(DeckSettings::default().uses_default_params());

        let mut damaged = bytes;
        put(&mut damaged, 4, &1.5f32.to_le_bytes());
        put(&mut damaged, 8, &f32::NAN.to_le_bytes());
        let d = DeckSettings::decode(&damaged);
        assert_eq!(d.desired_retention, DEFAULT_DESIRED_RETENTION);
        assert_eq!(d.params, DEFAULT_PARAMS);
        assert_eq!(d.new_per_day, 5);

        let mut out_of_range = s;
        out_of_range.params[24] = 3.0;
        assert_eq!(
            DeckSettings::decode(&out_of_range.encode()).params[24],
            0.95
        );
    }

    #[test]
    fn answer_log_appends_after_complete_entries() {
        let h = header();
        let log = u64::from(h.log_offset);
        assert_eq!(h.review_log_entries(log), 0);
        assert_eq!(h.review_log_entries(log + 64 + 10), 2);
        assert_eq!(h.review_log_append_offset(log + 64 + 10), log + 64);
        assert_eq!(h.review_log_entries(log - 1), 0);
    }

    #[test]
    fn records_round_trip_little_endian() {
        let entry = CardIndexEntry {
            anki_card_id: 0x0102_0304_0506_0708,
            text_offset: 0x1122_3344,
            text_length: 77,
        };
        let bytes = entry.encode();
        assert_eq!((bytes[0], bytes[8]), (0x08, 0x44));
        assert_eq!(CardIndexEntry::decode(&bytes), entry);

        let state = StateRecord {
            memory: MemoryState {
                stability: 10.0,
                difficulty: 5.0,
                stability_fast: 8.0,
            },
            last_review: 1_795_608_000,
            due: 1_795_608_000 + 3 * 86_400,
        };
        assert_eq!(StateRecord::decode(&state.encode()), state);
        assert!(StateRecord::decode(&[0; STATE_RECORD_SIZE]).is_new());

        let log = ReviewLogEntry {
            anki_card_id: 42,
            card_index: 7,
            review_time: 1_795_608_000,
            rating: 3,
            flags: REVIEW_FLAG_WAS_NEW,
            answer_seconds: 12,
            elapsed_days: 1.5,
            stability: 3.25,
            difficulty: 4.75,
        };
        assert_eq!(ReviewLogEntry::decode(&log.encode()), log);
    }

    #[test]
    fn splits_card_text() {
        assert_eq!(
            split_card_text(b"front\0back side"),
            (&b"front"[..], &b"back side"[..])
        );
        assert_eq!(
            split_card_text(b"only front"),
            (&b"only front"[..], &b""[..])
        );
    }
}
