#include <gtest/gtest.h>

#include <cstring>
#include <vector>

#include "FlashcardFormat.h"
#include "LineBreak.h"
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
  h.version = flashcards::kFormatVersion;
  h.indexOffset = 32 + 160 + 5;
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

TEST(FlashcardFormat, SettingsBlockAndVersion1) {
  const DeckHeader h = sampleHeader();
  EXPECT_EQ(32u, flashcards::deckSettingsOffset(h));

  // Version 1: no settings block, the name follows the header.
  DeckHeader v1 = h;
  v1.version = 1;
  v1.indexOffset = 32 + 5;
  uint8_t buf[flashcards::kDeckHeaderSize];
  flashcards::encodeDeckHeader(v1, buf);
  DeckHeader out{};
  ASSERT_TRUE(flashcards::decodeDeckHeader(buf, 100000, out));
  EXPECT_EQ(1u, out.version);
  EXPECT_EQ(0u, flashcards::deckSettingsOffset(out));
  // The same layout claiming version 2 has no room for the settings.
  v1.version = 2;
  flashcards::encodeDeckHeader(v1, buf);
  EXPECT_FALSE(flashcards::decodeDeckHeader(buf, 100000, out));

  flashcards::DeckSettings settings = flashcards::defaultDeckSettings();
  EXPECT_EQ(20u, settings.newPerDay);
  EXPECT_EQ(200u, settings.reviewsPerDay);
  EXPECT_FLOAT_EQ(0.9f, settings.fsrs.desiredRetention);
  settings.newPerDay = 7;
  settings.fsrs.params.w[0] = 0.5f;
  settings.fsrs.desiredRetention = 0.85f;
  settings.randomNewOrder = true;
  settings.mixNewWithReviews = true;
  uint8_t block[flashcards::kDeckSettingsSize];
  flashcards::encodeDeckSettings(settings, block);
  EXPECT_EQ(7, block[0]);
  EXPECT_EQ(1, block[144]);
  EXPECT_EQ(1, block[145]);
  flashcards::DeckSettings decoded{};
  flashcards::decodeDeckSettings(block, sizeof(block), decoded);
  EXPECT_EQ(7u, decoded.newPerDay);
  EXPECT_FLOAT_EQ(0.5f, decoded.fsrs.params.w[0]);
  EXPECT_FLOAT_EQ(0.85f, decoded.fsrs.desiredRetention);
  EXPECT_TRUE(decoded.randomNewOrder);
  EXPECT_TRUE(decoded.mixNewWithReviews);

  // A version 2 block has no order options: they get the defaults.
  DeckHeader v2 = h;
  v2.version = 2;
  EXPECT_EQ(144u, flashcards::deckSettingsSize(v2));
  EXPECT_EQ(160u, flashcards::deckSettingsSize(h));
  flashcards::decodeDeckSettings(block, 144, decoded);
  EXPECT_EQ(7u, decoded.newPerDay);
  EXPECT_FALSE(decoded.randomNewOrder);
  EXPECT_FALSE(decoded.mixNewWithReviews);

  // A damaged retention falls back to the default.
  const float bad = 3.0f;
  std::memcpy(block + 4, &bad, sizeof(bad));
  flashcards::decodeDeckSettings(block, sizeof(block), decoded);
  EXPECT_FLOAT_EQ(0.9f, decoded.fsrs.desiredRetention);
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
  void begin(Limits limits = Limits{}, uint16_t newDone = 0, uint16_t reviewsDone = 0,
             flashcards::Ordering ordering = flashcards::Ordering{}) {
    ASSERT_TRUE(session.begin(fsrs7::defaultConfig(), limits, ordering, day, kNoon, newDone, reviewsDone));
  }

  // Card indexes in the order the session shows them, answering Easy.
  std::vector<uint32_t> studyAll() {
    std::vector<uint32_t> order;
    while (const QueueItem* item = session.pick(kNoon)) {
      order.push_back(item->cardIndex);
      StateRecord state{};
      ReviewLogEntry log{};
      session.answer(kNoon, fsrs7::Rating::Easy, 5, state, log);
    }
    return order;
  }

  StudyDay day = flashcards::studyDayFor(kNoon, 0);
  ReviewSession session;
};

TEST_F(SessionTest, ShowsReviewsLeastRetrievableFirstThenNewCards) {
  begin();
  session.consider(0, newCard());
  // Same memory (stability 10 days): the longer since the last review, the
  // lower the retrievability, whatever the due time says.
  session.consider(1, reviewCard(kNoon - 10 * kDay, kNoon - 3 * kDay));
  session.consider(2, reviewCard(kNoon - 12 * kDay, kNoon - kDay));
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
  // Five due reviews; the two least retrievable are kept.
  for (uint32_t i = 0; i < 5; ++i) {
    session.consider(100 + i, reviewCard(kNoon - (20 + i) * kDay, kNoon - kDay));
  }
  session.finishLoading();
  EXPECT_EQ(2u, session.newCount());
  EXPECT_EQ(2u, session.dueCount());
  EXPECT_EQ((std::vector<uint32_t>{104, 103, 0, 1}), studyAll());
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

TEST_F(SessionTest, RandomNewOrderIsAFixedShuffle) {
  Limits limits;
  limits.newPerDay = 10;
  flashcards::Ordering ordering;
  ordering.randomNewOrder = true;
  auto load = [&] {
    begin(limits, 0, 0, ordering);
    for (uint32_t i = 0; i < 100; ++i) session.consider(i, newCard());
    session.finishLoading();
    return studyAll();
  };
  const std::vector<uint32_t> first = load();
  ASSERT_EQ(10u, first.size());
  EXPECT_NE((std::vector<uint32_t>{0, 1, 2, 3, 4, 5, 6, 7, 8, 9}), first);
  EXPECT_EQ(first, load()) << "the same cards in the same order every session";
}

TEST_F(SessionTest, MixesNewCardsEvenlyWithReviews) {
  Limits limits;
  limits.newPerDay = 2;
  flashcards::Ordering ordering;
  ordering.mixNewWithReviews = true;
  begin(limits, 0, 0, ordering);
  session.consider(0, newCard());
  session.consider(1, newCard());
  for (uint32_t i = 0; i < 4; ++i) {
    session.consider(10 + i, reviewCard(kNoon - (20 - i) * kDay, kNoon - kDay));
  }
  session.finishLoading();
  EXPECT_EQ((std::vector<uint32_t>{10, 0, 11, 12, 1, 13}), studyAll());

  begin(limits);  // after reviews
  session.consider(0, newCard());
  session.consider(10, reviewCard(kNoon - 20 * kDay, kNoon - kDay));
  session.finishLoading();
  EXPECT_EQ((std::vector<uint32_t>{10, 0}), studyAll());
}

TEST_F(SessionTest, LoadsAtMostTheSessionCaps) {
  Limits limits;
  limits.newPerDay = 9999;
  limits.reviewsPerDay = 9999;
  begin(limits);
  for (uint32_t i = 0; i < 500; ++i) session.consider(i, newCard());
  for (uint32_t i = 0; i < 500; ++i) session.consider(1000 + i, reviewCard(kNoon - 20 * kDay, kNoon - kDay));
  session.finishLoading();
  EXPECT_EQ(Limits{}.maxSessionNew, session.newCount());
  EXPECT_EQ(Limits{}.maxSessionReviews, session.dueCount());
}

namespace {

// Every character (not byte) is 10 pixels wide.
int measureChars(void*, const char* text, size_t length) {
  int chars = 0;
  for (size_t i = 0; i < length; ++i) {
    if ((static_cast<uint8_t>(text[i]) & 0xC0) != 0x80) ++chars;
  }
  return chars * 10;
}

// Lines of `text` at `chars` characters wide.
std::vector<std::string> lines(const std::string& text, int chars) {
  std::vector<std::string> out;
  size_t pos = 0;
  while (pos < text.size()) {
    const flashcards::LineSpan span =
        flashcards::breakLine(text.data() + pos, text.size() - pos, chars * 10, measureChars, nullptr);
    out.emplace_back(text, pos, span.length);
    pos += span.next;
  }
  return out;
}

using Lines = std::vector<std::string>;

}  // namespace

TEST(LineBreak, WrapsEnglishAtSpaces) {
  EXPECT_EQ((Lines{"the quick", "brown fox", "jumps"}), lines("the quick brown fox jumps", 10));
  EXPECT_EQ((Lines{"fits"}), lines("fits  ", 10)) << "trailing spaces dropped";
}

TEST(LineBreak, WrapsChineseBetweenCharacters) {
  EXPECT_EQ((Lines{"我喜欢学习", "中文"}), lines("我喜欢学习中文", 5));
  // Closing punctuation never starts a line: 。 takes 文 with it.
  EXPECT_EQ((Lines{"我喜欢学习中", "文。"}), lines("我喜欢学习中文。", 7));
  // Opening punctuation never ends a line: 「 goes with 你.
  EXPECT_EQ((Lines{"他说", "「你", "好」"}), lines("他说「你好」", 3));
}

TEST(LineBreak, MixesChineseAndEnglish) {
  EXPECT_EQ((Lines{"apple 苹果", "是水果"}), lines("apple 苹果是水果", 8));
  EXPECT_EQ((Lines{"学习", "English", "很有趣"}), lines("学习English很有趣", 7));
}

TEST(LineBreak, SplitsWordsWiderThanTheLine) {
  EXPECT_EQ((Lines{"abcde", "fghij", "kl"}), lines("abcdefghijkl", 5));
  EXPECT_EQ((Lines{"a"}), lines("a", 0)) << "always makes progress";
  EXPECT_TRUE(flashcards::isCjk(0x4E2D));   // 中
  EXPECT_TRUE(flashcards::isCjk(0xFF0C));   // ，
  EXPECT_FALSE(flashcards::isCjk(0x00E9));  // é
}
