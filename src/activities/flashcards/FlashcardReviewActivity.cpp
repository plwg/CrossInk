#include "FlashcardReviewActivity.h"

#include <Arduino.h>
#include <GfxRenderer.h>
#include <I18n.h>
#include <Logging.h>
#include <Memory.h>

#include <algorithm>
#include <cstdio>
#include <utility>
#include <vector>

#include "CrossPointSettings.h"
#include "LineBreak.h"
#include "MappedInputManager.h"
#include "SdCardFontSystem.h"
#include "components/CompactHeader.h"
#include "components/UITheme.h"
#include "fontIds.h"

namespace {
constexpr char TAG[] = "FLC";
constexpr int STATUS_FONT_ID = SMALL_FONT_ID;
// Holding Back this long leaves the session without answering the card.
constexpr unsigned long EXIT_HOLD_MS = 1000;
// Anki also caps recorded answer time; longer usually means the reader walked away.
constexpr unsigned long MAX_ANSWER_MS = 60UL * 60UL * 1000UL;

struct MeasureContext {
  const GfxRenderer* renderer;
  int fontId;
  std::string buffer;
};

int measureText(void* context, const char* text, size_t length) {
  auto* c = static_cast<MeasureContext*>(context);
  c->buffer.assign(text, length);
  return c->renderer->getTextWidth(c->fontId, c->buffer.c_str());
}
}  // namespace

FlashcardReviewActivity::FlashcardReviewActivity(GfxRenderer& renderer, MappedInputManager& mappedInput,
                                                 std::string deckName)
    : Activity("FlashcardReview", renderer, mappedInput), deckName(std::move(deckName)) {}

void FlashcardReviewActivity::onEnter() {
  Activity::onEnter();
  // Cards use the Flashcard Font (e.g. a Chinese font pack) when one is set,
  // swapped in the way the dictionary swaps in its font; otherwise the
  // reading font, loaded as the book readers do.
  usingFlashcardFont = false;
  if (SETTINGS.flashcardSdFontFamilyName[0] != '\0') {
    const DictionaryFontActivation font =
        sdFontSystem.activateDictionaryFont(renderer, SETTINGS.flashcardSdFontFamilyName);
    cardFontId = font.fontId;
    usingFlashcardFont = font.usingDictionaryFont;
    if (!usingFlashcardFont) LOG_ERR(TAG, "Flashcard font %s unavailable", SETTINGS.flashcardSdFontFamilyName);
  } else {
    sdFontSystem.ensureLoaded(renderer);
    cardFontId = SETTINGS.getReaderFontId();
  }
  phase = Phase::Error;
  errorText = tr(STR_FLASHCARDS_DECK_FAILED);
  saveFailed = false;
  backHoldHandled = false;

  // One buffer for the whole session, so showing cards does not churn the heap.
  cardText = makeUniqueNoThrow<char[]>(flashcards::kMaxCardTextBytes + 1);
  if (!cardText) {
    LOG_ERR(TAG, "OOM: %u bytes for card text", static_cast<unsigned>(flashcards::kMaxCardTextBytes + 1));
    requestUpdate();
    return;
  }
  if (!flashcards::currentTime(enterTime)) {
    errorText = tr(STR_FLASHCARDS_NEEDS_CLOCK);
    requestUpdate();
    return;
  }
  enterMillis = millis();

  const flashcards::StudyDay day = flashcards::studyDayFor(enterTime, flashcards::utcOffsetSeconds());
  uint16_t newDone = 0;
  uint16_t reviewsDone = 0;
  if (!deck.open(deckName)) {
    if (deck.needsNewerFirmware()) errorText = tr(STR_FLASHCARDS_DECK_TOO_NEW);
    requestUpdate();
    return;
  }
  deck.countToday(day, newDone, reviewsDone);
  if (!session.begin(deck.config(), deck.limits(), deck.ordering(), day, enterTime, newDone, reviewsDone) ||
      !deck.loadSession(session)) {
    LOG_ERR(TAG, "Cannot start a session for %s", deckName.c_str());
    session.clear();
    requestUpdate();
    return;
  }
  LOG_INF(TAG, "Studying %s: %u due, %u new", deckName.c_str(), static_cast<unsigned>(session.dueCount()),
          static_cast<unsigned>(session.newCount()));
  showNextCard();
}

void FlashcardReviewActivity::onExit() {
  session.clear();
  cardText.reset();
  if (usingFlashcardFont) {
    sdFontSystem.restoreReaderFont(renderer);
    usingFlashcardFont = false;
  }
  Activity::onExit();
}

uint32_t FlashcardReviewActivity::now() const { return enterTime + (millis() - enterMillis) / 1000UL; }

void FlashcardReviewActivity::showNextCard() {
  RenderLock lock;
  const flashcards::QueueItem* item = session.pick(now());
  if (item == nullptr) {
    phase = Phase::Done;
  } else if (deck.readCard(item->cardIndex, cardText.get(), flashcards::kMaxCardTextBytes + 1, cardLength,
                           ankiCardId)) {
    phase = Phase::Front;
    shownAtMillis = millis();
  } else {
    phase = Phase::Error;
    errorText = tr(STR_FLASHCARDS_DECK_FAILED);
  }
  lock.unlock();
  requestUpdate();
}

void FlashcardReviewActivity::reveal() {
  RenderLock lock;
  preview = session.preview(now());
  phase = Phase::Back;
  lock.unlock();
  requestUpdate();
}

void FlashcardReviewActivity::answer(const fsrs7::Rating rating) {
  const flashcards::QueueItem* item = session.current();
  if (item == nullptr) return;
  const uint32_t index = item->cardIndex;
  const unsigned long answerMs = std::min(millis() - shownAtMillis, MAX_ANSWER_MS);

  flashcards::StateRecord state{};
  flashcards::ReviewLogEntry log{};
  {
    // render() reads the queue counts on the render task.
    RenderLock lock;
    session.answer(now(), rating, static_cast<uint16_t>(std::min<unsigned long>(answerMs / 1000UL, UINT16_MAX)), state,
                   log);
  }
  log.ankiCardId = ankiCardId;
  const bool saved = deck.saveAnswer(index, state, log);
  saveFailed = !saved;
  showNextCard();
}

void FlashcardReviewActivity::loop() {
  using Button = MappedInputManager::Button;

  switch (phase) {
    case Phase::Error:
      if (mappedInput.wasReleased(Button::Back) || mappedInput.wasReleased(Button::Confirm)) finish();
      return;

    case Phase::Done:
      if (mappedInput.wasReleased(Button::Back)) {
        finish();
      } else if (mappedInput.wasReleased(Button::Confirm) && session.nextLearningDue() != 0) {
        showNextCard();
      }
      return;

    case Phase::Front:
      if (mappedInput.wasReleased(Button::Back)) {
        finish();
      } else if (mappedInput.wasReleased(Button::Confirm) || mappedInput.wasReleased(Button::Left) ||
                 mappedInput.wasReleased(Button::Right)) {
        reveal();
      }
      return;

    case Phase::Back:
      // Back doubles as Again, so leaving without answering is a long press.
      if (!backHoldHandled && mappedInput.isPressed(Button::Back) && mappedInput.getHeldTime() >= EXIT_HOLD_MS) {
        backHoldHandled = true;
        // Keeps the deck list from also seeing this Back release.
        finishAfterBackPress();
        return;
      }
      if (mappedInput.wasReleased(Button::Back)) {
        if (backHoldHandled) {
          backHoldHandled = false;
          return;
        }
        answer(fsrs7::Rating::Again);
      } else if (mappedInput.wasReleased(Button::Confirm)) {
        answer(fsrs7::Rating::Hard);
      } else if (mappedInput.wasReleased(Button::Left)) {
        answer(fsrs7::Rating::Good);
      } else if (mappedInput.wasReleased(Button::Right)) {
        answer(fsrs7::Rating::Easy);
      }
      return;
  }
}

void FlashcardReviewActivity::formatInterval(const float days, char* buf, const size_t size) const {
  const uint32_t seconds = flashcards::intervalSeconds(days);
  if (seconds < 3600) {
    snprintf(buf, size, tr(STR_FLASHCARDS_MINUTES), static_cast<unsigned>((seconds + 30) / 60));
  } else if (seconds < 86400) {
    snprintf(buf, size, tr(STR_FLASHCARDS_HOURS), static_cast<unsigned>((seconds + 1800) / 3600));
  } else if (days < 30.0f) {
    snprintf(buf, size, tr(STR_FLASHCARDS_DAYS), static_cast<unsigned>(days + 0.5f));
  } else if (days < 365.0f) {
    snprintf(buf, size, tr(STR_FLASHCARDS_MONTHS), static_cast<double>(days / 30.0f));
  } else {
    snprintf(buf, size, tr(STR_FLASHCARDS_YEARS), static_cast<double>(days / 365.0f));
  }
}

// Draws wrapped text between top and bottom; '\n' starts a new line. Lines
// break at spaces and between Chinese/Japanese/Korean characters.
// Returns the y just below the last line drawn.
int FlashcardReviewActivity::drawText(const char* text, const size_t length, const int fontId, const int top,
                                      const int bottom, const bool center) const {
  const auto& metrics = UITheme::getInstance().getMetrics();
  const int side = metrics.contentSidePadding;
  const int width = renderer.getScreenWidth() - 2 * side;
  const int lineHeight = renderer.getLineHeight(fontId);
  const int maxLines = lineHeight > 0 ? (bottom - top) / lineHeight : 0;
  if (maxLines <= 0 || width <= 0) return top;

  MeasureContext measure{&renderer, fontId, {}};
  std::vector<std::string> lines;
  size_t start = 0;
  while (start <= length && static_cast<int>(lines.size()) < maxLines) {
    const char* nl = static_cast<const char*>(memchr(text + start, '\n', length - start));
    const size_t end = nl ? static_cast<size_t>(nl - text) : length;
    if (end == start) lines.emplace_back();
    size_t pos = start;
    while (pos < end && static_cast<int>(lines.size()) < maxLines) {
      const flashcards::LineSpan span = flashcards::breakLine(text + pos, end - pos, width, measureText, &measure);
      const bool more = pos + span.next < end || nl != nullptr;
      if (more && static_cast<int>(lines.size()) == maxLines - 1) {
        // Last line with text left over: end it with an ellipsis.
        const std::string rest(text + pos, end - pos);
        lines.push_back(renderer.truncatedText(fontId, rest.c_str(), width));
        break;
      }
      lines.emplace_back(text + pos, span.length);
      pos += span.next;
    }
    if (!nl) break;
    start = end + 1;
  }

  const int blockHeight = static_cast<int>(lines.size()) * lineHeight;
  int y = center ? top + std::max(0, (bottom - top - blockHeight) / 2) : top;
  for (const auto& line : lines) {
    if (center) {
      renderer.drawCenteredText(fontId, y, line.c_str());
    } else {
      renderer.drawText(fontId, side, y, line.c_str());
    }
    y += lineHeight;
  }
  return y;
}

void FlashcardReviewActivity::render(RenderLock&&) {
  const auto& metrics = UITheme::getInstance().getMetrics();
  const int screenHeight = renderer.getScreenHeight();
  const int screenWidth = renderer.getScreenWidth();
  renderer.clearScreen();
  CompactHeader::drawTitle(renderer, deckName.c_str());

  int top = CompactHeader::contentTop(metrics) + metrics.verticalSpacing;
  const int bottom = screenHeight - metrics.buttonHintsHeight - metrics.verticalSpacing;

  if (phase == Phase::Front || phase == Phase::Back) {
    char status[64];
    snprintf(status, sizeof(status), tr(STR_FLASHCARDS_DECK_COUNTS), static_cast<unsigned>(session.dueCount()),
             static_cast<unsigned>(session.newCount()));
    renderer.drawCenteredText(STATUS_FONT_ID, top, saveFailed ? tr(STR_FLASHCARDS_SAVE_FAILED) : status);
    top += renderer.getLineHeight(STATUS_FONT_ID) + metrics.verticalSpacing;
  }

  const char* labels[4] = {"", "", "", ""};
  char ratingLabels[4][32];

  switch (phase) {
    case Phase::Error:
      drawText(errorText, strlen(errorText), UI_12_FONT_ID, top, bottom, true);
      labels[0] = tr(STR_BACK);
      break;

    case Phase::Done: {
      const uint32_t nextDue = session.nextLearningDue();
      if (nextDue == 0) {
        drawText(tr(STR_FLASHCARDS_ALL_DONE), strlen(tr(STR_FLASHCARDS_ALL_DONE)), UI_12_FONT_ID, top, bottom, true);
      } else {
        char wait[24];
        const uint32_t current = now();
        formatInterval(nextDue > current ? static_cast<float>(nextDue - current) / 86400.0f : 0.0f, wait, sizeof(wait));
        char message[96];
        snprintf(message, sizeof(message), tr(STR_FLASHCARDS_NEXT_LEARNING), wait);
        drawText(message, strlen(message), UI_12_FONT_ID, top, bottom, true);
        labels[1] = tr(STR_FLASHCARDS_CHECK);
      }
      labels[0] = tr(STR_BACK);
      break;
    }

    case Phase::Front: {
      size_t frontLength = 0;
      size_t backLength = 0;
      const char* back = nullptr;
      flashcards::splitCardText(cardText.get(), cardLength, frontLength, back, backLength);
      drawText(cardText.get(), frontLength, cardFontId, top, bottom, true);
      labels[0] = tr(STR_EXIT);
      labels[1] = tr(STR_FLASHCARDS_SHOW);
      break;
    }

    case Phase::Back: {
      size_t frontLength = 0;
      size_t backLength = 0;
      const char* back = nullptr;
      flashcards::splitCardText(cardText.get(), cardLength, frontLength, back, backLength);
      // The question keeps at most 40% of the space; the answer gets the rest.
      const int frontBottom = top + (bottom - top) * 2 / 5;
      const int afterFront = drawText(cardText.get(), frontLength, cardFontId, top, frontBottom, false);
      const int divider = afterFront + metrics.verticalSpacing;
      renderer.drawLine(metrics.contentSidePadding, divider, screenWidth - metrics.contentSidePadding, divider);
      drawText(back, backLength, cardFontId, divider + metrics.verticalSpacing, bottom, false);

      const StrId names[4] = {StrId::STR_FLASHCARDS_AGAIN, StrId::STR_FLASHCARDS_HARD, StrId::STR_FLASHCARDS_GOOD,
                              StrId::STR_FLASHCARDS_EASY};
      const fsrs7::ScheduledState* outcomes[4] = {&preview.again, &preview.hard, &preview.good, &preview.easy};
      for (int i = 0; i < 4; ++i) {
        char interval[16];
        formatInterval(outcomes[i]->intervalDays, interval, sizeof(interval));
        snprintf(ratingLabels[i], sizeof(ratingLabels[i]), "%s %s", I18N.get(names[i]), interval);
        labels[i] = ratingLabels[i];
      }
      break;
    }
  }

  // Back, Confirm, Left, Right: Again, Hard, Good, Easy on the default layout.
  const auto mapped = mappedInput.mapLabels(labels[0], labels[1], labels[2], labels[3]);
  GUI.drawButtonHints(renderer, mapped.btn1, mapped.btn2, mapped.btn3, mapped.btn4);
  renderer.displayBuffer();
}
