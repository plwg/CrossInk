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
  // A learning card due within this time is shown early rather than ending
  // the session, like Anki's "learn ahead limit".
  uint32_t learnAheadSeconds = 20 * 60;
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
  // Starts a session; follow with consider() for every card, then
  // finishLoading(). Returns false if the queues cannot be allocated.
  bool begin(const fsrs7::Config& config, const Limits& limits, const StudyDay& day, uint16_t newDoneToday,
             uint16_t reviewsDoneToday);
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

  fsrs7::Config config_{};
  Limits limits_{};
  StudyDay day_{};

  QueueBuffer reviews_;   // sorted by due once loaded
  QueueBuffer newCards_;  // in deck order
  QueueBuffer learning_;  // sorted by due
  size_t reviewPos_ = 0;
  size_t newPos_ = 0;
  size_t latestReviewSlot_ = 0;  // while loading: slot with the latest due

  const QueueItem* current_ = nullptr;
  Source currentSource_ = Source::None;
};

}  // namespace flashcards
