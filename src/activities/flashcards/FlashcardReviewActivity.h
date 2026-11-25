#pragma once

#include <Fsrs7.h>

#include <memory>
#include <string>

#include "activities/Activity.h"
#include "flashcards/FlashcardStore.h"

// Studies one deck: shows a card's front, then its back with the four FSRS
// ratings on the front buttons (Again, Hard, Good, Easy from left to right).
class FlashcardReviewActivity final : public Activity {
 public:
  FlashcardReviewActivity(GfxRenderer& renderer, MappedInputManager& mappedInput, std::string deckName);

  void onEnter() override;
  void onExit() override;
  void loop() override;
  void render(RenderLock&&) override;

 private:
  enum class Phase : uint8_t { Error, Front, Back, Done };

  void showNextCard();
  void reveal();
  void answer(fsrs7::Rating rating);
  // Unix time with seconds: minute-resolution RTC time plus millis() since entering.
  uint32_t now() const;
  void formatInterval(float days, char* buf, size_t size) const;
  int drawText(const char* text, size_t length, int fontId, int top, int bottom, bool center) const;

  std::string deckName;
  flashcards::Deck deck;
  flashcards::ReviewSession session;
  fsrs7::NextStates preview{};
  // Card text (front, NUL, back), allocated once for the whole session.
  std::unique_ptr<char[]> cardText;
  size_t cardLength = 0;
  uint64_t ankiCardId = 0;
  Phase phase = Phase::Error;
  const char* errorText = nullptr;
  bool saveFailed = false;
  uint32_t enterTime = 0;
  unsigned long enterMillis = 0;
  unsigned long shownAtMillis = 0;
  bool backHoldHandled = false;
};
