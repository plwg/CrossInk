//! Turns Anki review-log rows into FSRS training items.

use fsrs::{FSRSItem, FSRSReview};

const MS_PER_DAY: f64 = 86_400_000.0;

/// Values of the `type` column in Anki's `revlog` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    Learning,
    Review,
    Relearning,
    Filtered,
    Manual,
    Rescheduled,
}

impl ReviewKind {
    pub fn from_anki(value: i64) -> Option<Self> {
        Some(match value {
            0 => Self::Learning,
            1 => Self::Review,
            2 => Self::Relearning,
            3 => Self::Filtered,
            4 => Self::Manual,
            5 => Self::Rescheduled,
            _ => return None,
        })
    }
}

/// One row of Anki's `revlog` table (only the columns FSRS needs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevlogEntry {
    /// Review time, in milliseconds since the Unix epoch.
    pub id: i64,
    pub card_id: i64,
    /// Button pressed: 1 Again .. 4 Easy, 0 for manual changes.
    pub ease: u8,
    pub kind: ReviewKind,
    /// Ease factor in permille; 0 means the card was reset or not rescheduled.
    pub factor: i64,
}

impl RevlogEntry {
    /// "Forget"/reset entries clear a card's history.
    fn is_reset(&self) -> bool {
        self.kind == ReviewKind::Manual && self.ease == 0 && self.factor == 0
    }

    /// Entries that reflect a real answer that changed the card's schedule.
    fn is_graded_review(&self) -> bool {
        (1..=4).contains(&self.ease)
            && match self.kind {
                ReviewKind::Learning | ReviewKind::Review | ReviewKind::Relearning => true,
                // Filtered-deck reviews with factor 0 did not reschedule the card.
                ReviewKind::Filtered => self.factor != 0,
                ReviewKind::Manual | ReviewKind::Rescheduled => false,
            }
    }
}

/// Training data for the FSRS optimizer.
#[derive(Debug, Default)]
pub struct TrainingSet {
    pub items: Vec<FSRSItem>,
    /// Card id for each item, aligned by index.
    pub card_ids: Vec<i64>,
    /// Cards that contributed at least one item.
    pub cards: usize,
    /// Graded reviews in the usable histories.
    pub reviews: usize,
}

/// A card's usable review history.
#[derive(Debug, Clone, PartialEq)]
pub struct CardHistory {
    pub card_id: i64,
    /// Graded answers since the last reset, oldest first. Starts with the
    /// card's first learning answer (delta_t 0) when `complete`.
    pub reviews: Vec<FSRSReview>,
    /// Whether the history starts at the first learning answer. Imported or
    /// truncated histories do not, so FSRS cannot replay them from scratch.
    pub complete: bool,
    /// Time of each answer in `reviews`, in Unix milliseconds.
    pub times_ms: Vec<i64>,
    /// Time of the latest graded answer, in Unix milliseconds.
    pub last_review_ms: i64,
}

/// Splits review-log entries of any number of cards into per-card histories.
/// The history of a card starts after its last reset; manual changes,
/// reschedules and cram reviews are left out. Elapsed time is in fractional
/// days, which FSRS-7 uses for same-day reviews.
pub fn card_histories(mut entries: Vec<RevlogEntry>) -> Vec<CardHistory> {
    entries.sort_by_key(|e| (e.card_id, e.id));
    let mut histories = Vec::new();
    for card in entries.chunk_by(|a, b| a.card_id == b.card_id) {
        let start = card
            .iter()
            .rposition(RevlogEntry::is_reset)
            .map_or(0, |i| i + 1);
        let graded: Vec<&RevlogEntry> = card[start..]
            .iter()
            .filter(|e| e.is_graded_review())
            .collect();
        let (Some(first), Some(last)) = (graded.first(), graded.last()) else {
            continue;
        };
        let reviews = graded
            .iter()
            .enumerate()
            .map(|(i, e)| FSRSReview {
                rating: u32::from(e.ease),
                delta_t: if i == 0 {
                    0.0
                } else {
                    ((e.id - graded[i - 1].id) as f64 / MS_PER_DAY) as f32
                },
            })
            .collect();
        histories.push(CardHistory {
            card_id: first.card_id,
            reviews,
            complete: first.kind == ReviewKind::Learning,
            times_ms: graded.iter().map(|e| e.id).collect(),
            last_review_ms: last.id,
        });
    }
    histories
}

/// Builds training items from review-log entries of any number of cards.
///
/// Only complete histories are used, so the optimizer sees each card from its
/// first answer. Each item is a prefix of the history: earlier reviews plus
/// the review whose outcome FSRS learns to predict.
pub fn training_set(entries: Vec<RevlogEntry>) -> TrainingSet {
    let mut set = TrainingSet::default();
    for history in card_histories(entries) {
        if !history.complete || history.reviews.len() < 2 {
            continue;
        }
        for end in 2..=history.reviews.len() {
            set.items.push(FSRSItem {
                reviews: history.reviews[..end].to_vec(),
            });
            set.card_ids.push(history.card_id);
        }
        set.cards += 1;
        set.reviews += history.reviews.len();
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    fn entry(card_id: i64, id: i64, ease: u8, kind: ReviewKind) -> RevlogEntry {
        RevlogEntry {
            id,
            card_id,
            ease,
            kind,
            factor: 2500,
        }
    }

    fn deltas(item: &FSRSItem) -> Vec<(u32, f32)> {
        item.reviews.iter().map(|r| (r.rating, r.delta_t)).collect()
    }

    #[test]
    fn builds_prefixes_with_fractional_days() {
        let set = training_set(vec![
            entry(1, 0, 3, ReviewKind::Learning),
            entry(1, DAY / 2, 3, ReviewKind::Learning),
            entry(1, DAY / 2 + 3 * DAY, 1, ReviewKind::Review),
        ]);
        assert_eq!(set.items.len(), 2);
        assert_eq!(set.card_ids, vec![1, 1]);
        assert_eq!(set.cards, 1);
        assert_eq!(set.reviews, 3);
        assert_eq!(deltas(&set.items[0]), vec![(3, 0.0), (3, 0.5)]);
        assert_eq!(deltas(&set.items[1]), vec![(3, 0.0), (3, 0.5), (1, 3.0)]);
    }

    #[test]
    fn sorts_unordered_rows_and_separates_cards() {
        let set = training_set(vec![
            entry(2, DAY, 4, ReviewKind::Review),
            entry(1, DAY, 3, ReviewKind::Review),
            entry(2, 0, 3, ReviewKind::Learning),
            entry(1, 0, 2, ReviewKind::Learning),
        ]);
        assert_eq!(set.card_ids, vec![1, 2]);
        assert_eq!(deltas(&set.items[0]), vec![(2, 0.0), (3, 1.0)]);
        assert_eq!(deltas(&set.items[1]), vec![(3, 0.0), (4, 1.0)]);
    }

    #[test]
    fn history_restarts_after_reset() {
        let mut reset = entry(1, 2 * DAY, 0, ReviewKind::Manual);
        reset.factor = 0;
        let set = training_set(vec![
            entry(1, 0, 3, ReviewKind::Learning),
            entry(1, DAY, 3, ReviewKind::Review),
            reset,
            entry(1, 3 * DAY, 1, ReviewKind::Learning),
            entry(1, 4 * DAY, 3, ReviewKind::Learning),
        ]);
        assert_eq!(set.items.len(), 1);
        assert_eq!(deltas(&set.items[0]), vec![(1, 0.0), (3, 1.0)]);
    }

    #[test]
    fn skips_cards_without_a_learning_start() {
        // Imported or truncated history: the first answer is unknown.
        let set = training_set(vec![
            entry(1, 0, 3, ReviewKind::Review),
            entry(1, DAY, 3, ReviewKind::Review),
        ]);
        assert!(set.items.is_empty());
        assert_eq!(set.cards, 0);
    }

    #[test]
    fn ignores_manual_rescheduled_and_unscheduled_filtered_entries() {
        let mut cram = entry(1, DAY / 2, 1, ReviewKind::Filtered);
        cram.factor = 0;
        let set = training_set(vec![
            entry(1, 0, 3, ReviewKind::Learning),
            cram,
            entry(1, DAY / 3, 0, ReviewKind::Rescheduled),
            entry(1, DAY, 3, ReviewKind::Manual),
            entry(1, 2 * DAY, 3, ReviewKind::Review),
        ]);
        assert_eq!(set.items.len(), 1);
        assert_eq!(deltas(&set.items[0]), vec![(3, 0.0), (3, 2.0)]);
    }

    #[test]
    fn histories_mark_incomplete_starts_and_last_review() {
        let histories = card_histories(vec![
            entry(1, 0, 3, ReviewKind::Learning),
            entry(1, DAY, 4, ReviewKind::Review),
            entry(2, 5 * DAY, 3, ReviewKind::Review),
        ]);
        assert_eq!(histories.len(), 2);
        assert!(histories[0].complete);
        assert_eq!(histories[0].last_review_ms, DAY);
        assert_eq!(histories[0].reviews.len(), 2);
        assert!(!histories[1].complete);
        assert_eq!(histories[1].last_review_ms, 5 * DAY);
    }

    #[test]
    fn single_review_cards_give_no_items() {
        let set = training_set(vec![entry(1, 0, 3, ReviewKind::Learning)]);
        assert!(set.items.is_empty());
    }
}
