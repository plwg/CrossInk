#include "ReviewSession.h"

#include <inki_core.h>

#include <algorithm>

namespace flashcards {

namespace {

constexpr uint32_t kSecondsPerDay = 86400;
constexpr uint32_t kRolloverSeconds = 4 * 3600;
// Short-term cards beyond the daily limits; they only exist after an answer.
constexpr size_t kExtraLearningSlots = 16;

bool dueBefore(const QueueItem& a, const QueueItem& b) {
  if (a.state.due != b.state.due) return a.state.due < b.state.due;
  return a.cardIndex < b.cardIndex;
}

// The card most likely forgotten first.
bool lessRetrievable(const QueueItem& a, const QueueItem& b) {
  if (a.retrievability != b.retrievability) return a.retrievability < b.retrievability;
  return a.cardIndex < b.cardIndex;
}

// A fixed shuffle of card positions (MurmurHash3's finaliser, a bijection),
// so the random order is the same every session.
uint32_t shuffleKey(uint32_t cardIndex) {
  uint32_t x = cardIndex + 0x9e3779b9u;
  x ^= x >> 16;
  x *= 0x85ebca6bu;
  x ^= x >> 13;
  x *= 0xc2b2ae35u;
  x ^= x >> 16;
  return x;
}

// Adds `item` to a bounded queue that keeps the cards first in `before`
// order. `worst` tracks the slot a better card replaces once full.
template <typename Before>
void offer(QueueBuffer& queue, size_t& worst, const QueueItem& item, Before before) {
  if (queue.capacity() == 0) return;
  if (!queue.full()) {
    queue.pushBack(item);
    if (queue.size() == 1 || before(queue[worst], item)) worst = queue.size() - 1;
    return;
  }
  if (!before(item, queue[worst])) return;
  queue[worst] = item;
  worst = 0;
  for (size_t i = 1; i < queue.size(); ++i) {
    if (before(queue[worst], queue[i])) worst = i;
  }
}

}  // namespace

bool QueueBuffer::allocate(size_t capacity) {
  release();
  if (capacity == 0) return true;
  items_ = makeUniqueNoThrow<QueueItem[]>(capacity);
  if (!items_) return false;
  capacity_ = capacity;
  return true;
}

void QueueBuffer::release() {
  items_.reset();
  size_ = 0;
  capacity_ = 0;
}

void QueueBuffer::pushBack(const QueueItem& item) {
  if (full()) return;
  items_[size_++] = item;
}

void QueueBuffer::insertAt(size_t pos, const QueueItem& item) {
  if (full() || pos > size_) return;
  for (size_t i = size_; i > pos; --i) items_[i] = items_[i - 1];
  items_[pos] = item;
  ++size_;
}

void QueueBuffer::eraseFront() {
  if (empty()) return;
  for (size_t i = 1; i < size_; ++i) items_[i - 1] = items_[i];
  --size_;
}

StudyDay studyDayFor(uint32_t now, int32_t utcOffsetSeconds) {
  const int64_t local = static_cast<int64_t>(now) + utcOffsetSeconds - kRolloverSeconds;
  const int64_t dayIndex = local >= 0 ? local / kSecondsPerDay : (local - kSecondsPerDay + 1) / kSecondsPerDay;
  const int64_t start = dayIndex * kSecondsPerDay - utcOffsetSeconds + kRolloverSeconds;
  const int64_t clampedStart = std::max<int64_t>(start, 0);
  return StudyDay{static_cast<uint32_t>(clampedStart),
                  static_cast<uint32_t>(std::min<int64_t>(start + kSecondsPerDay, UINT32_MAX))};
}

// Shared with the inki PC tool, so both schedule a card for the same second.
uint32_t intervalSeconds(float days) { return inki_interval_seconds(days); }

bool ReviewSession::begin(const fsrs7::Config& config, const Limits& limits, const Ordering& ordering,
                          const StudyDay& day, uint32_t now, uint16_t newDoneToday, uint16_t reviewsDoneToday) {
  clear();
  config_ = config;
  limits_ = limits;
  ordering_ = ordering;
  day_ = day;
  now_ = now;
  const size_t reviewsLeft = limits.reviewsPerDay > reviewsDoneToday ? limits.reviewsPerDay - reviewsDoneToday : 0;
  const size_t newLeft = limits.newPerDay > newDoneToday ? limits.newPerDay - newDoneToday : 0;
  const size_t reviewCapacity = std::min<size_t>(reviewsLeft, limits.maxSessionReviews);
  const size_t newCapacity = std::min<size_t>(newLeft, limits.maxSessionNew);
  if (!reviews_.allocate(reviewCapacity) || !newCards_.allocate(newCapacity) ||
      !learning_.allocate(reviewCapacity + newCapacity + kExtraLearningSlots)) {
    clear();
    return false;
  }
  return true;
}

void ReviewSession::clear() {
  reviews_.release();
  newCards_.release();
  learning_.release();
  reviewPos_ = 0;
  newPos_ = 0;
  worstReviewSlot_ = 0;
  worstNewSlot_ = 0;
  current_ = nullptr;
  currentSource_ = Source::None;
}

bool ReviewSession::newBefore(const QueueItem& a, const QueueItem& b) const {
  if (ordering_.randomNewOrder) return shuffleKey(a.cardIndex) < shuffleKey(b.cardIndex);
  return a.cardIndex < b.cardIndex;
}

void ReviewSession::consider(uint32_t cardIndex, const StateRecord& state) {
  QueueItem item{cardIndex, state, 0.0f};
  if (state.isNew()) {
    offer(newCards_, worstNewSlot_, item, [this](const QueueItem& a, const QueueItem& b) { return newBefore(a, b); });
    return;
  }
  if (state.due > day_.end) return;

  // Cards on short-term (same-day) intervals are learning cards; they do not
  // count against the review limit, matching Anki.
  const bool shortTerm = state.due > state.lastReview && state.due - state.lastReview < kSecondsPerDay;
  if (shortTerm) {
    learning_.pushBack(item);
    return;
  }
  if (reviews_.capacity() == 0) return;
  // Past the limit, keep the cards most likely forgotten.
  item.retrievability = fsrs7::retrievability(config_.params, state.memory, elapsedDays(item, now_));
  offer(reviews_, worstReviewSlot_, item, lessRetrievable);
}

void ReviewSession::finishLoading() {
  std::sort(reviews_.begin(), reviews_.end(), lessRetrievable);
  std::sort(newCards_.begin(), newCards_.end(),
            [this](const QueueItem& a, const QueueItem& b) { return newBefore(a, b); });
  std::sort(learning_.begin(), learning_.end(), dueBefore);
}

// With mixing, new cards are spread evenly: one is due once its share of
// the new cards falls behind the share of reviews done.
bool ReviewSession::showNewNext() const {
  const bool reviewsLeft = reviewPos_ < reviews_.size();
  const bool newLeft = newPos_ < newCards_.size();
  if (!newLeft) return false;
  if (!reviewsLeft) return true;
  if (!ordering_.mixNewWithReviews) return false;
  return static_cast<uint64_t>(newPos_) * reviews_.size() < static_cast<uint64_t>(reviewPos_) * newCards_.size();
}

const QueueItem* ReviewSession::pick(uint32_t now) {
  current_ = nullptr;
  currentSource_ = Source::None;
  if (!learning_.empty() && learning_[0].state.due <= now) {
    currentSource_ = Source::Learning;
    current_ = &learning_[0];
  } else if (showNewNext()) {
    currentSource_ = Source::New;
    current_ = &newCards_[newPos_];
  } else if (reviewPos_ < reviews_.size()) {
    currentSource_ = Source::Review;
    current_ = &reviews_[reviewPos_];
  } else if (!learning_.empty() && learning_[0].state.due <= now + limits_.learnAheadSeconds) {
    currentSource_ = Source::Learning;
    current_ = &learning_[0];
  }
  return current_;
}

float ReviewSession::elapsedDays(const QueueItem& item, uint32_t now) const {
  if (item.state.isNew() || now <= item.state.lastReview) return 0.0f;
  return static_cast<float>(now - item.state.lastReview) / static_cast<float>(kSecondsPerDay);
}

fsrs7::NextStates ReviewSession::preview(uint32_t now) const {
  const QueueItem& item = *current_;
  return fsrs7::nextStates(config_.params, item.state.isNew() ? nullptr : &item.state.memory, config_.desiredRetention,
                           elapsedDays(item, now));
}

void ReviewSession::answer(uint32_t now, fsrs7::Rating rating, uint16_t answerSeconds, StateRecord& newState,
                           ReviewLogEntry& log) {
  const QueueItem item = *current_;
  const fsrs7::ScheduledState chosen = preview(now).forRating(rating);
  const uint32_t interval = intervalSeconds(chosen.intervalDays);
  newState.memory = chosen.memory;
  newState.lastReview = now;
  newState.due = now > UINT32_MAX - interval ? UINT32_MAX : now + interval;

  log = ReviewLogEntry{};
  log.cardIndex = item.cardIndex;
  log.reviewTime = now;
  log.rating = static_cast<uint8_t>(rating);
  log.flags = item.state.isNew() ? kReviewFlagWasNew : 0;
  log.answerSeconds = answerSeconds;
  log.elapsedDays = elapsedDays(item, now);
  log.stability = chosen.memory.stability;
  log.difficulty = chosen.memory.difficulty;

  switch (currentSource_) {
    case Source::Learning:
      learning_.eraseFront();
      break;
    case Source::Review:
      ++reviewPos_;
      break;
    case Source::New:
      ++newPos_;
      break;
    case Source::None:
      break;
  }
  current_ = nullptr;
  currentSource_ = Source::None;

  if (newState.due <= day_.end) insertLearning(QueueItem{item.cardIndex, newState, 0.0f});
}

void ReviewSession::insertLearning(const QueueItem& item) {
  if (learning_.full()) return;  // shown next session instead
  const QueueItem* pos = std::upper_bound(learning_.begin(), learning_.end(), item, dueBefore);
  learning_.insertAt(static_cast<size_t>(pos - learning_.begin()), item);
}

uint32_t ReviewSession::nextLearningDue() const { return learning_.empty() ? 0 : learning_[0].state.due; }

size_t ReviewSession::dueCount() const { return reviews_.size() - reviewPos_ + learning_.size(); }

}  // namespace flashcards
