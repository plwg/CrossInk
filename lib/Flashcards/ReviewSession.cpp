#include "ReviewSession.h"

#include <algorithm>
#include <cmath>

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

uint32_t intervalSeconds(float days) {
  if (!(days > 0.0f)) return 60;
  const double seconds = std::round(static_cast<double>(days) * kSecondsPerDay);
  if (seconds < 60.0) return 60;
  if (seconds > static_cast<double>(UINT32_MAX)) return UINT32_MAX;
  return static_cast<uint32_t>(seconds);
}

bool ReviewSession::begin(const fsrs7::Config& config, const Limits& limits, const StudyDay& day, uint16_t newDoneToday,
                          uint16_t reviewsDoneToday) {
  clear();
  config_ = config;
  limits_ = limits;
  day_ = day;
  const size_t reviewCapacity = limits.reviewsPerDay > reviewsDoneToday ? limits.reviewsPerDay - reviewsDoneToday : 0;
  const size_t newCapacity = limits.newPerDay > newDoneToday ? limits.newPerDay - newDoneToday : 0;
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
  latestReviewSlot_ = 0;
  current_ = nullptr;
  currentSource_ = Source::None;
}

void ReviewSession::consider(uint32_t cardIndex, const StateRecord& state) {
  const QueueItem item{cardIndex, state};
  if (state.isNew()) {
    newCards_.pushBack(item);
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
  if (!reviews_.full()) {
    reviews_.pushBack(item);
    if (reviews_.size() == 1 || dueBefore(reviews_[latestReviewSlot_], item)) {
      latestReviewSlot_ = reviews_.size() - 1;
    }
    return;
  }
  // Full: keep the most overdue cards.
  if (!dueBefore(item, reviews_[latestReviewSlot_])) return;
  reviews_[latestReviewSlot_] = item;
  latestReviewSlot_ = 0;
  for (size_t i = 1; i < reviews_.size(); ++i) {
    if (dueBefore(reviews_[latestReviewSlot_], reviews_[i])) latestReviewSlot_ = i;
  }
}

void ReviewSession::finishLoading() {
  std::sort(reviews_.begin(), reviews_.end(), dueBefore);
  std::sort(learning_.begin(), learning_.end(), dueBefore);
}

const QueueItem* ReviewSession::pick(uint32_t now) {
  current_ = nullptr;
  currentSource_ = Source::None;
  if (!learning_.empty() && learning_[0].state.due <= now) {
    currentSource_ = Source::Learning;
    current_ = &learning_[0];
  } else if (reviewPos_ < reviews_.size()) {
    currentSource_ = Source::Review;
    current_ = &reviews_[reviewPos_];
  } else if (newPos_ < newCards_.size()) {
    currentSource_ = Source::New;
    current_ = &newCards_[newPos_];
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

  if (newState.due <= day_.end) insertLearning(QueueItem{item.cardIndex, newState});
}

void ReviewSession::insertLearning(const QueueItem& item) {
  if (learning_.full()) return;  // shown next session instead
  const QueueItem* pos = std::upper_bound(learning_.begin(), learning_.end(), item, dueBefore);
  learning_.insertAt(static_cast<size_t>(pos - learning_.begin()), item);
}

uint32_t ReviewSession::nextLearningDue() const { return learning_.empty() ? 0 : learning_[0].state.due; }

size_t ReviewSession::dueCount() const { return reviews_.size() - reviewPos_ + learning_.size(); }

}  // namespace flashcards
