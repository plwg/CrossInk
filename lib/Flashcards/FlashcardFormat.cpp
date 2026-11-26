// The .inki format is implemented once, in the Rust crate inki-core (shared
// with the inki PC tool); these functions convert to and from its C types.
#include "FlashcardFormat.h"

#include <inki_core.h>

#include <cstring>

namespace flashcards {

static_assert(kFormatVersion == INKI_DECK_VERSION, "inki-core writes a different deck format version");
static_assert(kDeckHeaderSize == INKI_DECK_HEADER_SIZE && kDeckSettingsSize == INKI_DECK_SETTINGS_SIZE,
              "deck layout differs from inki's");

namespace {

InkiDeckHeader toInki(const DeckHeader& h) {
  return InkiDeckHeader{h.cardCount, h.indexOffset, h.stateOffset, h.logOffset, h.nameLength, h.version};
}

DeckSettings fromInki(const InkiDeckSettings& s) {
  DeckSettings out{};
  out.newPerDay = s.new_per_day;
  out.reviewsPerDay = s.reviews_per_day;
  out.fsrs.desiredRetention = s.desired_retention;
  std::memcpy(out.fsrs.params.w, s.params, sizeof(out.fsrs.params.w));
  out.randomNewOrder = s.new_card_order == INKI_NEW_ORDER_RANDOM;
  out.mixNewWithReviews = s.new_card_placement == INKI_NEW_MIXED;
  return out;
}

}  // namespace

void encodeDeckHeader(const DeckHeader& header, uint8_t* out) {
  const InkiDeckHeader h = toInki(header);
  inki_encode_deck_header(&h, out);
}

bool decodeDeckHeader(const uint8_t* in, size_t fileSize, DeckHeader& out) {
  InkiDeckHeader h{};
  if (!inki_decode_deck_header(in, fileSize, &h)) return false;
  out = DeckHeader{h.card_count, h.index_offset, h.state_offset, h.log_offset, h.name_length, h.version};
  return true;
}

DeckSettings defaultDeckSettings() {
  InkiDeckSettings s{};
  inki_default_deck_settings(&s);
  return fromInki(s);
}

uint32_t deckSettingsOffset(const DeckHeader& header) {
  const InkiDeckHeader h = toInki(header);
  return inki_deck_settings_offset(&h);
}

size_t deckSettingsSize(const DeckHeader& header) {
  const InkiDeckHeader h = toInki(header);
  return inki_deck_settings_size(&h);
}

void encodeDeckSettings(const DeckSettings& settings, uint8_t* out) {
  InkiDeckSettings s{};
  s.new_per_day = settings.newPerDay;
  s.reviews_per_day = settings.reviewsPerDay;
  s.desired_retention = settings.fsrs.desiredRetention;
  std::memcpy(s.params, settings.fsrs.params.w, sizeof(s.params));
  s.new_card_order = settings.randomNewOrder ? INKI_NEW_ORDER_RANDOM : INKI_NEW_ORDER_DECK;
  s.new_card_placement = settings.mixNewWithReviews ? INKI_NEW_MIXED : INKI_NEW_AFTER_REVIEWS;
  inki_encode_deck_settings(&s, out);
}

void decodeDeckSettings(const uint8_t* in, size_t length, DeckSettings& out) {
  InkiDeckSettings s{};
  inki_decode_deck_settings(in, length, &s);
  out = fromInki(s);
}

size_t reviewLogEntries(const DeckHeader& header, size_t fileSize) {
  const InkiDeckHeader h = toInki(header);
  return static_cast<size_t>(inki_review_log_entries(&h, fileSize));
}

size_t reviewLogAppendOffset(const DeckHeader& header, size_t fileSize) {
  const InkiDeckHeader h = toInki(header);
  return static_cast<size_t>(inki_review_log_append_offset(&h, fileSize));
}

void encodeCardIndexEntry(const CardIndexEntry& entry, uint8_t* out) {
  const InkiCardIndexEntry e{entry.ankiCardId, entry.textOffset, entry.textLength};
  inki_encode_card_index_entry(&e, out);
}

void decodeCardIndexEntry(const uint8_t* in, CardIndexEntry& out) {
  InkiCardIndexEntry e{};
  inki_decode_card_index_entry(in, &e);
  out = CardIndexEntry{e.anki_card_id, e.text_offset, e.text_length};
}

void splitCardText(const char* text, size_t length, size_t& frontLength, const char*& back, size_t& backLength) {
  size_t backOffset = length;
  inki_split_card_text(text, length, &frontLength, &backOffset, &backLength);
  back = text + backOffset;
}

void encodeStateRecord(const StateRecord& record, uint8_t* out) {
  const InkiStateRecord r{fsrs7::detail::toInki(record.memory), record.lastReview, record.due};
  inki_encode_state_record(&r, out);
}

void decodeStateRecord(const uint8_t* in, StateRecord& out) {
  InkiStateRecord r{};
  inki_decode_state_record(in, &r);
  out = StateRecord{fsrs7::detail::fromInki(r.memory), r.last_review, r.due};
}

void encodeReviewLogEntry(const ReviewLogEntry& entry, uint8_t* out) {
  const InkiReviewLogEntry e{entry.ankiCardId,    entry.cardIndex,   entry.reviewTime, entry.rating,    entry.flags,
                             entry.answerSeconds, entry.elapsedDays, entry.stability,  entry.difficulty};
  inki_encode_review_log_entry(&e, out);
}

void decodeReviewLogEntry(const uint8_t* in, ReviewLogEntry& out) {
  InkiReviewLogEntry e{};
  inki_decode_review_log_entry(in, &e);
  out = ReviewLogEntry{e.anki_card_id,   e.card_index,   e.review_time, e.rating,    e.flags,
                       e.answer_seconds, e.elapsed_days, e.stability,   e.difficulty};
}

}  // namespace flashcards
