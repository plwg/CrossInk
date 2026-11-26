#pragma once

// Flashcard deck files on the SD card. All integers are little-endian.
// Documented in docs/flashcards.md; the PC tool `inki` writes them.
//
//   /flashcards/<deck>.inki   one file per deck:
//
//   header       32 bytes (DeckHeader)
//   settings     160 bytes (DeckSettings); 144 in version 2, none in version 1
//   deck name    the Anki deck name, nameLength bytes (not used by the reader)
//   card index   cardCount entries at indexOffset
//   card text    question, NUL, answer for each card
//   state        cardCount records at stateOffset, updated in place
//   answer log   entries from logOffset to the end of the file, appended

#include <Fsrs7.h>

#include <cstddef>
#include <cstdint>

namespace flashcards {

constexpr char kRootDir[] = "/flashcards";
constexpr char kDeckExtension[] = ".inki";

// The version inki writes. Versions 1 and 2 are still read.
constexpr uint16_t kFormatVersion = 3;
constexpr size_t kDeckHeaderSize = 32;
constexpr size_t kDeckSettingsSize = 160;  // the largest settings block
constexpr size_t kCardIndexEntrySize = 16;
constexpr size_t kStateRecordSize = 20;
constexpr size_t kReviewLogEntrySize = 32;

// Largest card text (front + back) the device reads; longer text is cut.
constexpr size_t kMaxCardTextBytes = 4096;

// Deck header: char magic[4] "INKD"; u16 version; u16 flags; u32 cardCount;
// u32 indexOffset; u32 stateOffset; u32 logOffset; u16 nameLength;
// u16 reserved; u32 reserved.
struct DeckHeader {
  uint32_t cardCount;
  uint32_t indexOffset;
  uint32_t stateOffset;
  uint32_t logOffset;
  uint16_t nameLength;
  uint16_t version;  // kFormatVersion when writing; 1 or 2 when read
};

void encodeDeckHeader(const DeckHeader& header, uint8_t* out);
// Returns false on wrong magic, unsupported version, or regions that do not
// fit in a file of `fileSize` bytes.
bool decodeDeckHeader(const uint8_t* in, size_t fileSize, DeckHeader& out);

// Complete answer-log entries in a file of `fileSize` bytes. A partly
// written last entry (power lost mid-write) is not counted.
size_t reviewLogEntries(const DeckHeader& header, size_t fileSize);

// Where the next answer is appended: after the last complete entry, so a
// partly written one is overwritten.
size_t reviewLogAppendOffset(const DeckHeader& header, size_t fileSize);

// How a deck is studied, chosen on the PC with inki: u16 newPerDay;
// u16 reviewsPerDay; f32 desiredRetention; f32 params[34]; u8 newCardOrder;
// u8 newCardPlacement; 14 reserved bytes.
struct DeckSettings {
  uint16_t newPerDay;
  uint16_t reviewsPerDay;
  fsrs7::Config fsrs;      // FSRS-7 parameters and desired retention
  bool randomNewOrder;     // new cards in a fixed random order
  bool mixNewWithReviews;  // new cards spread among the reviews
};

// 20 new cards and 200 reviews a day, 90% retention, default parameters.
// Version 1 decks, which have no settings block, use these.
DeckSettings defaultDeckSettings();

// Offset of the settings block, or 0 for a version 1 deck, which has none.
uint32_t deckSettingsOffset(const DeckHeader& header);
// Size of the settings block: 0, 144 (version 2) or 160 bytes.
size_t deckSettingsSize(const DeckHeader& header);

void encodeDeckSettings(const DeckSettings& settings, uint8_t* out);
// Decodes `length` bytes (deckSettingsSize). A retention FSRS-7 cannot use, a
// parameter that is not a number or an unknown option falls back to the
// default; parameters are clipped.
void decodeDeckSettings(const uint8_t* in, size_t length, DeckSettings& out);

// Card index entry. Card text lives at textOffset
// (from the start of the file): UTF-8 front, a NUL byte, UTF-8 back.
struct CardIndexEntry {
  uint64_t ankiCardId;
  uint32_t textOffset;
  uint32_t textLength;
};

void encodeCardIndexEntry(const CardIndexEntry& entry, uint8_t* out);
void decodeCardIndexEntry(const uint8_t* in, CardIndexEntry& out);

// Splits card text into front and back. A text without a NUL is all front.
void splitCardText(const char* text, size_t length, size_t& frontLength, const char*& back, size_t& backLength);

// State record. All-zero bytes mean a new card.
struct StateRecord {
  fsrs7::MemoryState memory;
  uint32_t lastReview;  // Unix seconds (UTC); 0 = never reviewed
  uint32_t due;         // Unix seconds (UTC)

  bool isNew() const { return lastReview == 0; }
};

void encodeStateRecord(const StateRecord& record, uint8_t* out);
void decodeStateRecord(const uint8_t* in, StateRecord& out);

constexpr uint8_t kReviewFlagWasNew = 0x01;

// Answer-log entry: one answer. The reader counts today's answers from these
// for its daily limits; the PC tool keeps them when it updates a deck.
struct ReviewLogEntry {
  uint64_t ankiCardId;
  uint32_t cardIndex;
  uint32_t reviewTime;  // Unix seconds (UTC)
  uint8_t rating;       // 1 Again .. 4 Easy
  uint8_t flags;        // kReviewFlagWasNew
  uint16_t answerSeconds;
  float elapsedDays;  // since the previous answer; 0 for a new card
  float stability;    // after the answer
  float difficulty;   // after the answer
};

void encodeReviewLogEntry(const ReviewLogEntry& entry, uint8_t* out);
void decodeReviewLogEntry(const uint8_t* in, ReviewLogEntry& out);

}  // namespace flashcards
