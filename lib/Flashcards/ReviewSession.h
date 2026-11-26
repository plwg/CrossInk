#pragma once

// Chooses which flashcard to show next and applies answers with FSRS-7.
// Pure logic with no storage or UI access, so it is unit-tested on the host.

#include <Fsrs7.h>
#include <Memory.h>

#include <cstddef>
#include <cstdint>
#include <memory>

#include "FlashcardFormat.h"

namespace flashcards {

struct Limits {
  uint16_t newPerDay = 20;
  uint16_t reviewsPerDay = 200;
  // At most this many reviews and new cards are loaded per session, to
  // bound memory; opening the deck again loads more.
  uint16_t maxSessionReviews = 300;
  uint16_t maxSessionNew = 100;
  // A learning card due within this time is shown early rather than ending
  // the session, like Anki's "learn ahead limit".
  uint32_t learnAheadSeconds = 20 * 60;
};

// The order of a session. Reviews always come lowest retrievability first.
struct Ordering {
  bool randomNewOrder = false;     // a fixed shuffle instead of deck order
  bool mixNewWithReviews = false;  // spread new cards among the reviews
};

// A study day runs from 4 am to 4 am local time, as in Anki.
struct StudyDay {
  uint32_t start;
  uint32_t end;
};

StudyDay studyDayFor(uint32_t now, int32_t utcOffsetSeconds);

// Seconds for an FSRS interval in days: at least one minute, saturating.
uint32_t intervalSeconds(float days);

struct QueueItem {
  uint32_t cardIndex;
  StateRecord state;
  float retrievability;  // at the start of the session; reviews only
};

// Fixed-capacity array of queue items. Allocated once per session with a
// nothrow allocation, so running out of memory fails cleanly instead of
// aborting (exceptions are disabled on the device).
class QueueBuffer {
 public:
  bool allocate(size_t capacity);
  void release();
  size_t size() const { return size_; }
  size_t capacity() const { return capacity_; }
  bool full() const { return size_ >= capacity_; }
  bool empty() const { return size_ == 0; }
  QueueItem& operator[](size_t i) { return items_[i]; }
  const QueueItem& operator[](size_t i) const { return items_[i]; }
  QueueItem* begin() { return items_.get(); }
  QueueItem* end() { return begin() + size_; }
  void pushBack(const QueueItem& item);              // no-op when full
  void insertAt(size_t pos, const QueueItem& item);  // no-op when full
  void eraseFront();

 private:
  std::unique_ptr<QueueItem[]> items_;
  size_t size_ = 0;
  size_t capacity_ = 0;
};

class ReviewSession {
 public:
  // Starts a session at `now`; follow with consider() for every card, then
  // finishLoading(). Returns false if the queues cannot be allocated.
  bool begin(const fsrs7::Config& config, const Limits& limits, const Ordering& ordering, const StudyDay& day,
             uint32_t now, uint16_t newDoneToday, uint16_t reviewsDoneToday);
  void consider(uint32_t cardIndex, const StateRecord& state);
  void finishLoading();
  void clear();

  // Chooses the card to show at `now`. Returns nullptr when nothing is left
  // to study right now (see nextLearningDue()).
  const QueueItem* pick(uint32_t now);
  const QueueItem* current() const { return current_; }

  // Outcome of each rating for the current card.
  fsrs7::NextStates preview(uint32_t now) const;

  // Answers the current card. `log.ankiCardId` is left for the caller.
  void answer(uint32_t now, fsrs7::Rating rating, uint16_t answerSeconds, StateRecord& newState, ReviewLogEntry& log);

  // Earliest due time of a learning card still queued today, or 0.
  uint32_t nextLearningDue() const;

  // Cards left today: reviews (including learning cards due today) and new.
  size_t dueCount() const;
  size_t newCount() const { return newCards_.size() - newPos_; }

 private:
  enum class Source : uint8_t { None, Learning, Review, New };

  float elapsedDays(const QueueItem& item, uint32_t now) const;
  void insertLearning(const QueueItem& item);
  bool newBefore(const QueueItem& a, const QueueItem& b) const;
  bool showNewNext() const;

  fsrs7::Config config_{};
  Limits limits_{};
  Ordering ordering_{};
  StudyDay day_{};
  uint32_t now_ = 0;

  QueueBuffer reviews_;   // lowest retrievability first once loaded
  QueueBuffer newCards_;  // in deck order or shuffled
  QueueBuffer learning_;  // sorted by due
  size_t reviewPos_ = 0;
  size_t newPos_ = 0;
  // While loading, when a queue is full: the slot a better card replaces.
  size_t worstReviewSlot_ = 0;
  size_t worstNewSlot_ = 0;

  const QueueItem* current_ = nullptr;
  Source currentSource_ = Source::None;
};

}  // namespace flashcards
