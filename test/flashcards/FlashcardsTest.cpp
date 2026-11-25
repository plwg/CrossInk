#include <gtest/gtest.h>

#include <cstring>

#include "FlashcardFormat.h"
#include "ReviewSession.h"

using flashcards::CardIndexEntry;
using flashcards::DeckHeader;
using flashcards::Limits;
using flashcards::QueueItem;
using flashcards::ReviewLogEntry;
using flashcards::ReviewSession;
using flashcards::StateRecord;
using flashcards::StudyDay;

namespace {

constexpr uint32_t kDay = 86400;
constexpr uint32_t kHour = 3600;
// 2026-11-25 12:00:00 UTC
constexpr uint32_t kNoon = 1795608000;

StateRecord newCard() { return StateRecord{}; }

StateRecord reviewCard(uint32_t lastReview, uint32_t due) {
  return StateRecord{fsrs7::MemoryState{10.0f, 5.0f, 8.0f}, lastReview, due};
}

}  // namespace

namespace {
// A valid layout: 3 cards, 5-byte deck name, 100 bytes of card text.
DeckHeader sampleHeader() {
  DeckHeader h{};
  h.cardCount = 3;
  h.nameLength = 5;
  h.indexOffset = 32 + 5;
  h.stateOffset = h.indexOffset + 3 * 16 + 100;
  h.logOffset = h.stateOffset + 3 * 20;
  return h;
}
}  // namespace

TEST(FlashcardFormat, DeckHeaderRoundTripAndValidation) {
  uint8_t buf[flashcards::kDeckHeaderSize];
  const DeckHeader h = sampleHeader();
  flashcards::encodeDeckHeader(h, buf);
  EXPECT_EQ(0, std::memcmp(buf, "INKD", 4));
  DeckHeader out{};
  ASSERT_TRUE(flashcards::decodeDeckHeader(buf, h.logOffset, out));
  EXPECT_EQ(3u, out.cardCount);
  EXPECT_EQ(h.stateOffset, out.stateOffset);
  EXPECT_EQ(5u, out.nameLength);

  // The state region must be complete.
  EXPECT_FALSE(flashcards::decodeDeckHeader(buf, h.logOffset - 1, out));

  DeckHeader damaged = h;
  damaged.logOffset += 4;  // state region the wrong size
  flashcards::encodeDeckHeader(damaged, buf);
  EXPECT_FALSE(flashcards::decodeDeckHeader(buf, 100000, out));

  damaged = h;
  damaged.cardCount = 0xFFFFFFFF;  // must not wrap around
  flashcards::encodeDeckHeader(damaged, buf);
  EXPECT_FALSE(flashcards::decodeDeckHeader(buf, 0xFFFFFFFF, out));

  flashcards::encodeDeckHeader(h, buf);
  buf[4] = 99;  // unsupported version
  EXPECT_FALSE(flashcards::decodeDeckHeader(buf, h.logOffset, out));
}

TEST(FlashcardFormat, AnswerLogAppendsAfterCompleteEntries) {
  const DeckHeader h = sampleHeader();
  EXPECT_EQ(0u, flashcards::reviewLogEntries(h, h.logOffset));
  EXPECT_EQ(h.logOffset, flashcards::reviewLogAppendOffset(h, h.logOffset));
  EXPECT_EQ(2u, flashcards::reviewLogEntries(h, h.logOffset + 64));
  // A half-written third entry is not counted and gets overwritten.
  EXPECT_EQ(2u, flashcards::reviewLogEntries(h, h.logOffset + 64 + 10));
  EXPECT_EQ(h.logOffset + 64, flashcards::reviewLogAppendOffset(h, h.logOffset + 64 + 10));
}

TEST(FlashcardFormat, RecordsRoundTripLittleEndian) {
  uint8_t buf[flashcards::kReviewLogEntrySize];

  const CardIndexEntry entry{0x0102030405060708ull, 0x11223344u, 77};
  flashcards::encodeCardIndexEntry(entry, buf);
  EXPECT_EQ(0x08, buf[0]);
  EXPECT_EQ(0x44, buf[8]);
  CardIndexEntry entryOut{};
  flashcards::decodeCardIndexEntry(buf, entryOut);
  EXPECT_EQ(entry.ankiCardId, entryOut.ankiCardId);
  EXPECT_EQ(entry.textOffset, entryOut.textOffset);
  EXPECT_EQ(entry.textLength, entryOut.textLength);

  const StateRecord state = reviewCard(kNoon, kNoon + 3 * kDay);
  flashcards::encodeStateRecord(state, buf);
  StateRecord stateOut{};
  flashcards::decodeStateRecord(buf, stateOut);
  EXPECT_EQ(state.memory.stability, stateOut.memory.stability);
  EXPECT_EQ(state.memory.difficulty, stateOut.memory.difficulty);
  EXPECT_EQ(state.memory.stabilityFast, stateOut.memory.stabilityFast);
  EXPECT_EQ(state.lastReview, stateOut.lastReview);
  EXPECT_EQ(state.due, stateOut.due);

  const ReviewLogEntry log{42, 7, kNoon, 3, flashcards::kReviewFlagWasNew, 12, 1.5f, 3.25f, 4.75f};
  flashcards::encodeReviewLogEntry(log, buf);
  ReviewLogEntry logOut{};
  flashcards::decodeReviewLogEntry(buf, logOut);
  EXPECT_EQ(log.ankiCardId, logOut.ankiCardId);
  EXPECT_EQ(log.cardIndex, logOut.cardIndex);
  EXPECT_EQ(log.reviewTime, logOut.reviewTime);
  EXPECT_EQ(log.rating, logOut.rating);
  EXPECT_EQ(log.flags, logOut.flags);
  EXPECT_EQ(log.answerSeconds, logOut.answerSeconds);
  EXPECT_EQ(log.elapsedDays, logOut.elapsedDays);
  EXPECT_EQ(log.stability, logOut.stability);
  EXPECT_EQ(log.difficulty, logOut.difficulty);
}

TEST(FlashcardFormat, AllZeroStateIsNewCard) {
  uint8_t zeros[flashcards::kStateRecordSize] = {};
  StateRecord state{};
  flashcards::decodeStateRecord(zeros, state);
  EXPECT_TRUE(state.isNew());
}

TEST(FlashcardFormat, SplitsCardText) {
  const char text[] = "front\0back side";
  size_t frontLength = 0, backLength = 0;
  const char* back = nullptr;
  flashcards::splitCardText(text, sizeof(text) - 1, frontLength, back, backLength);
  EXPECT_EQ(5u, frontLength);
  EXPECT_EQ(std::string("back side"), std::string(back, backLength));

  flashcards::splitCardText("only front", 10, frontLength, back, backLength);
  EXPECT_EQ(10u, frontLength);
  EXPECT_EQ(0u, backLength);
}

TEST(StudyDay, RollsOverAtFourAmLocalTime) {
  // UTC: 12:00 is after 04:00, so the day started at 04:00 today.
  StudyDay day = flashcards::studyDayFor(kNoon, 0);
  EXPECT_EQ(kNoon - 8 * kHour, day.start);
  EXPECT_EQ(day.start + kDay, day.end);

  // UTC+10: 12:00 UTC is 22:00 local, so the day started at 04:00 local = 18:00 UTC yesterday.
  day = flashcards::studyDayFor(kNoon, 10 * 3600);
  EXPECT_EQ(kNoon - 18 * kHour, day.start);

  // 02:00 UTC belongs to the previous study day.
  day = flashcards::studyDayFor(kNoon - 10 * kHour, 0);
  EXPECT_EQ(kNoon - 8 * kHour - kDay, day.start);
}

TEST(IntervalSeconds, ClampsToAtLeastOneMinute) {
  EXPECT_EQ(60u, flashcards::intervalSeconds(0.0f));
  EXPECT_EQ(60u, flashcards::intervalSeconds(-1.0f));
  EXPECT_EQ(86400u, flashcards::intervalSeconds(1.0f));
  EXPECT_EQ(UINT32_MAX, flashcards::intervalSeconds(1e9f));
}

class SessionTest : public ::testing::Test {
 protected:
  void begin(Limits limits = Limits{}, uint16_t newDone = 0, uint16_t reviewsDone = 0) {
    ASSERT_TRUE(session.begin(fsrs7::defaultConfig(), limits, day, newDone, reviewsDone));
  }

  StudyDay day = flashcards::studyDayFor(kNoon, 0);
  ReviewSession session;
};

TEST_F(SessionTest, ShowsReviewsByDueThenNewCards) {
  begin();
  session.consider(0, newCard());
  session.consider(1, reviewCard(kNoon - 10 * kDay, kNoon - kDay));      // overdue
  session.consider(2, reviewCard(kNoon - 10 * kDay, kNoon - 3 * kDay));  // more overdue
  session.consider(3, reviewCard(kNoon - 2 * kDay, kNoon + 30 * kDay));  // not due
  session.consider(4, reviewCard(kNoon - 5 * kDay, kNoon + 3 * kHour));  // due later today
  session.finishLoading();
  EXPECT_EQ(3u, session.dueCount());
  EXPECT_EQ(1u, session.newCount());

  const uint32_t expected[] = {2, 1, 4, 0};
  for (uint32_t index : expected) {
    const QueueItem* item = session.pick(kNoon);
    ASSERT_NE(nullptr, item);
    EXPECT_EQ(index, item->cardIndex);
    StateRecord state{};
    ReviewLogEntry log{};
    session.answer(kNoon, fsrs7::Rating::Easy, 5, state, log);
    EXPECT_EQ(index, log.cardIndex);
  }
  EXPECT_EQ(nullptr, session.pick(kNoon));
}

TEST_F(SessionTest, AnsweringUpdatesStateAndLog) {
  begin();
  session.consider(0, newCard());
  session.finishLoading();
  ASSERT_NE(nullptr, session.pick(kNoon));

  StateRecord state{};
  ReviewLogEntry log{};
  session.answer(kNoon, fsrs7::Rating::Good, 9, state, log);

  const fsrs7::Params& params = fsrs7::kDefaultParams;
  const fsrs7::MemoryState expected = fsrs7::initialState(params, fsrs7::Rating::Good);
  EXPECT_FLOAT_EQ(expected.stability, state.memory.stability);
  EXPECT_EQ(kNoon, state.lastReview);
  EXPECT_EQ(kNoon + flashcards::intervalSeconds(fsrs7::nextIntervalDays(params, expected, 0.9f)), state.due);
  EXPECT_EQ(3, log.rating);
  EXPECT_EQ(flashcards::kReviewFlagWasNew, log.flags);
  EXPECT_EQ(9, log.answerSeconds);
  EXPECT_EQ(0.0f, log.elapsedDays);
}

TEST_F(SessionTest, AgainRequeuesCardLaterToday) {
  begin();
  session.consider(0, newCard());
  session.finishLoading();
  ASSERT_NE(nullptr, session.pick(kNoon));
  StateRecord state{};
  ReviewLogEntry log{};
  session.answer(kNoon, fsrs7::Rating::Again, 3, state, log);

  ASSERT_LT(state.due, day.end);
  EXPECT_EQ(state.due, session.nextLearningDue());
  EXPECT_EQ(1u, session.dueCount());

  // Not shown before it is due unless within the learn-ahead window.
  const uint32_t learnAhead = Limits{}.learnAheadSeconds;
  if (state.due > kNoon + learnAhead) {
    EXPECT_EQ(nullptr, session.pick(kNoon));
  }
  const QueueItem* again = session.pick(state.due);
  ASSERT_NE(nullptr, again);
  EXPECT_EQ(0u, again->cardIndex);
  session.answer(state.due, fsrs7::Rating::Good, 3, state, log);
  EXPECT_EQ(0, log.flags);
  EXPECT_GT(log.elapsedDays, 0.0f);
}

TEST_F(SessionTest, LearningCardsDueNowComeFirst) {
  begin();
  session.consider(0, reviewCard(kNoon - 5 * kDay, kNoon - kDay));
  session.consider(1, reviewCard(kNoon - 30 * 60, kNoon - 60));  // short-term step, due
  session.finishLoading();
  const QueueItem* item = session.pick(kNoon);
  ASSERT_NE(nullptr, item);
  EXPECT_EQ(1u, item->cardIndex);
}

TEST_F(SessionTest, RespectsDailyLimitsAndDoneCounts) {
  Limits limits;
  limits.newPerDay = 3;
  limits.reviewsPerDay = 2;
  begin(limits, 1, 0);
  for (uint32_t i = 0; i < 10; ++i) session.consider(i, newCard());
  // Five due reviews; the two most overdue are kept.
  for (uint32_t i = 0; i < 5; ++i) {
    session.consider(100 + i, reviewCard(kNoon - 20 * kDay, kNoon - (i + 1) * kDay));
  }
  session.finishLoading();
  EXPECT_EQ(2u, session.newCount());
  EXPECT_EQ(2u, session.dueCount());
  const QueueItem* item = session.pick(kNoon);
  ASSERT_NE(nullptr, item);
  EXPECT_EQ(104u, item->cardIndex);  // due 5 days ago
}

TEST_F(SessionTest, LimitsReachedLeaveNothingToStudy) {
  Limits limits;
  limits.newPerDay = 5;
  limits.reviewsPerDay = 5;
  begin(limits, 5, 5);
  session.consider(0, newCard());
  session.consider(1, reviewCard(kNoon - 5 * kDay, kNoon - kDay));
  session.finishLoading();
  EXPECT_EQ(nullptr, session.pick(kNoon));
  EXPECT_EQ(0u, session.dueCount());
  EXPECT_EQ(0u, session.newCount());
}

TEST_F(SessionTest, PreviewMatchesFsrs) {
  begin();
  const StateRecord state = reviewCard(kNoon - 4 * kDay, kNoon);
  session.consider(0, state);
  session.finishLoading();
  ASSERT_NE(nullptr, session.pick(kNoon));
  const fsrs7::NextStates preview = session.preview(kNoon);
  const fsrs7::NextStates expected = fsrs7::nextStates(fsrs7::kDefaultParams, &state.memory, 0.9f, 4.0f);
  EXPECT_FLOAT_EQ(expected.good.intervalDays, preview.good.intervalDays);
  EXPECT_FLOAT_EQ(expected.again.memory.stability, preview.again.memory.stability);
}
