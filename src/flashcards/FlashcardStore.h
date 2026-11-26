#pragma once

// SD-card access for flashcard decks. File formats are in
// lib/Flashcards/FlashcardFormat.h and docs/flashcards.md.

#include <Fsrs7.h>

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

#include "FlashcardFormat.h"
#include "ReviewSession.h"

namespace flashcards {

// True when /flashcards exists on the SD card (shows the Home menu entry).
bool hasFlashcardsFolder();

// Names of the decks in /flashcards (the .inki files, without extension), sorted.
void listDecks(std::vector<std::string>& out);

// Current Unix time (UTC) from the RTC (X3) or the synced system clock.
// Returns false when no trustworthy time is available.
bool currentTime(uint32_t& now);

// The user's UTC offset from Settings > Clock, in seconds.
int32_t utcOffsetSeconds();

class Deck {
 public:
  // Opens /flashcards/<name>.inki and reads its settings. Returns false (and
  // logs) on error.
  bool open(const std::string& name);

  const std::string& name() const { return name_; }
  // After a failed open(): the file is a deck from a newer version of inki.
  bool needsNewerFirmware() const { return needsNewerFirmware_; }
  uint32_t cardCount() const { return header_.cardCount; }
  // FSRS-7 parameters and desired retention stored in the deck.
  const fsrs7::Config& config() const { return settings_.fsrs; }
  // Daily limits stored in the deck.
  Limits limits() const;
  // New-card order stored in the deck.
  Ordering ordering() const;

  // Answers already given in `day`, read from the end of the answer log.
  void countToday(const StudyDay& day, uint16_t& newDone, uint16_t& reviewsDone) const;

  // Streams the state records into session.consider(), then finishLoading().
  bool loadSession(ReviewSession& session) const;

  // Reads a card's text (front, NUL, back) into `buf`, cut to bufSize - 1
  // bytes and NUL-terminated.
  bool readCard(uint32_t index, char* buf, size_t bufSize, size_t& length, uint64_t& ankiCardId) const;

  // Appends the answer to the log, then updates the card's state record.
  bool saveAnswer(uint32_t index, const StateRecord& state, const ReviewLogEntry& log) const;

 private:
  std::string name_;
  std::string path_;
  DeckHeader header_{};
  DeckSettings settings_{};
  bool needsNewerFirmware_ = false;
};

}  // namespace flashcards
