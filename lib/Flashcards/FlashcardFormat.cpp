#include "FlashcardFormat.h"

#include <cstring>

namespace flashcards {

namespace {

constexpr char kMagic[4] = {'I', 'N', 'K', 'D'};

void putU16(uint8_t* out, uint16_t v) {
  out[0] = static_cast<uint8_t>(v);
  out[1] = static_cast<uint8_t>(v >> 8);
}

void putU32(uint8_t* out, uint32_t v) {
  for (int i = 0; i < 4; ++i) out[i] = static_cast<uint8_t>(v >> (8 * i));
}

void putU64(uint8_t* out, uint64_t v) {
  for (int i = 0; i < 8; ++i) out[i] = static_cast<uint8_t>(v >> (8 * i));
}

void putF32(uint8_t* out, float v) {
  uint32_t bits;
  std::memcpy(&bits, &v, sizeof(bits));
  putU32(out, bits);
}

uint16_t getU16(const uint8_t* in) { return static_cast<uint16_t>(in[0] | (in[1] << 8)); }

uint32_t getU32(const uint8_t* in) {
  uint32_t v = 0;
  for (int i = 3; i >= 0; --i) v = (v << 8) | in[i];
  return v;
}

uint64_t getU64(const uint8_t* in) {
  uint64_t v = 0;
  for (int i = 7; i >= 0; --i) v = (v << 8) | in[i];
  return v;
}

float getF32(const uint8_t* in) {
  const uint32_t bits = getU32(in);
  float v;
  std::memcpy(&v, &bits, sizeof(v));
  return v;
}

}  // namespace

void encodeDeckHeader(const DeckHeader& header, uint8_t* out) {
  std::memcpy(out, kMagic, 4);
  putU16(out + 4, kFormatVersion);
  putU16(out + 6, 0);
  putU32(out + 8, header.cardCount);
  putU32(out + 12, header.indexOffset);
  putU32(out + 16, header.stateOffset);
  putU32(out + 20, header.logOffset);
  putU16(out + 24, header.nameLength);
  putU16(out + 26, 0);
  putU32(out + 28, 0);
}

bool decodeDeckHeader(const uint8_t* in, size_t fileSize, DeckHeader& out) {
  if (std::memcmp(in, kMagic, 4) != 0 || getU16(in + 4) != kFormatVersion) return false;
  DeckHeader h{};
  h.cardCount = getU32(in + 8);
  h.indexOffset = getU32(in + 12);
  h.stateOffset = getU32(in + 16);
  h.logOffset = getU32(in + 20);
  h.nameLength = getU16(in + 24);
  // 64-bit arithmetic: a damaged count must not wrap around.
  const uint64_t count = h.cardCount;
  const bool fits = h.indexOffset >= kDeckHeaderSize + h.nameLength &&
                    h.stateOffset >= h.indexOffset + count * kCardIndexEntrySize &&
                    h.logOffset == h.stateOffset + count * kStateRecordSize && h.logOffset <= fileSize;
  if (!fits) return false;
  out = h;
  return true;
}

size_t reviewLogEntries(const DeckHeader& header, size_t fileSize) {
  return fileSize > header.logOffset ? (fileSize - header.logOffset) / kReviewLogEntrySize : 0;
}

size_t reviewLogAppendOffset(const DeckHeader& header, size_t fileSize) {
  return header.logOffset + reviewLogEntries(header, fileSize) * kReviewLogEntrySize;
}

void encodeCardIndexEntry(const CardIndexEntry& entry, uint8_t* out) {
  putU64(out, entry.ankiCardId);
  putU32(out + 8, entry.textOffset);
  putU32(out + 12, entry.textLength);
}

void decodeCardIndexEntry(const uint8_t* in, CardIndexEntry& out) {
  out.ankiCardId = getU64(in);
  out.textOffset = getU32(in + 8);
  out.textLength = getU32(in + 12);
}

void splitCardText(const char* text, size_t length, size_t& frontLength, const char*& back, size_t& backLength) {
  const void* nul = std::memchr(text, '\0', length);
  if (nul == nullptr) {
    frontLength = length;
    back = text + length;
    backLength = 0;
    return;
  }
  frontLength = static_cast<size_t>(static_cast<const char*>(nul) - text);
  back = text + frontLength + 1;
  backLength = length - frontLength - 1;
}

void encodeStateRecord(const StateRecord& record, uint8_t* out) {
  putF32(out, record.memory.stability);
  putF32(out + 4, record.memory.difficulty);
  putF32(out + 8, record.memory.stabilityFast);
  putU32(out + 12, record.lastReview);
  putU32(out + 16, record.due);
}

void decodeStateRecord(const uint8_t* in, StateRecord& out) {
  out.memory.stability = getF32(in);
  out.memory.difficulty = getF32(in + 4);
  out.memory.stabilityFast = getF32(in + 8);
  out.lastReview = getU32(in + 12);
  out.due = getU32(in + 16);
}

void encodeReviewLogEntry(const ReviewLogEntry& entry, uint8_t* out) {
  putU64(out, entry.ankiCardId);
  putU32(out + 8, entry.cardIndex);
  putU32(out + 12, entry.reviewTime);
  out[16] = entry.rating;
  out[17] = entry.flags;
  putU16(out + 18, entry.answerSeconds);
  putF32(out + 20, entry.elapsedDays);
  putF32(out + 24, entry.stability);
  putF32(out + 28, entry.difficulty);
}

void decodeReviewLogEntry(const uint8_t* in, ReviewLogEntry& out) {
  out.ankiCardId = getU64(in);
  out.cardIndex = getU32(in + 8);
  out.reviewTime = getU32(in + 12);
  out.rating = in[16];
  out.flags = in[17];
  out.answerSeconds = getU16(in + 18);
  out.elapsedDays = getF32(in + 20);
  out.stability = getF32(in + 24);
  out.difficulty = getF32(in + 28);
}

}  // namespace flashcards
