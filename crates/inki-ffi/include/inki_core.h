// C interface to inki-core: FSRS-7 scheduling, the .inki deck format and the
// fsrs7-params.txt parser, shared by the inki PC tool and the firmware.
// Implemented in Rust (crates/inki-ffi); see crates/inki-core for details.
//
// All functions are allocation-free and never block. Pointers must be valid;
// functions return without writing when given a null output pointer.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define INKI_PARAM_COUNT 34
#define INKI_DECK_HEADER_SIZE 32
#define INKI_INDEX_ENTRY_SIZE 16
#define INKI_STATE_RECORD_SIZE 20
#define INKI_REVIEW_LOG_ENTRY_SIZE 32

// ---- FSRS-7 ---------------------------------------------------------------

typedef struct {
  float stability;       // long-term stability, days
  float difficulty;      // 1..10
  float stability_fast;  // short-term stability, days
} InkiMemoryState;

typedef struct {
  InkiMemoryState memory;
  float interval_days;
} InkiScheduled;

typedef struct {
  InkiScheduled again;
  InkiScheduled hard;
  InkiScheduled good;
  InkiScheduled easy;
} InkiNextStates;

typedef struct {
  float params[INKI_PARAM_COUNT];
  float desired_retention;
} InkiConfig;

// FSRS-7 defaults and 90% desired retention.
void inki_default_config(InkiConfig* out);
// Clamps parameters into the ranges FSRS-7 accepts.
void inki_clip_params(float* params);
// Parses fsrs7-params.txt into `config`: keys missing from the file get the
// defaults, parameters are clipped. Returns false, leaving `config`
// unchanged, if a value is malformed.
bool inki_parse_config(const char* text, size_t length, InkiConfig* config);

// `rating`: 1 Again, 2 Hard, 3 Good, 4 Easy (others are clamped).
float inki_retrievability(const float* params, const InkiMemoryState* state, float elapsed_days);
void inki_initial_state(const float* params, uint8_t rating, InkiMemoryState* out);
void inki_next_state(const float* params, const InkiMemoryState* state, float elapsed_days, uint8_t rating,
                     InkiMemoryState* out);
float inki_next_interval_days(const float* params, const InkiMemoryState* state, float desired_retention);
// `current` is null for a new card.
void inki_next_states(const float* params, const InkiMemoryState* current, float desired_retention,
                      float elapsed_days, InkiNextStates* out);
// Seconds for an interval in days: at least 60, saturating.
uint32_t inki_interval_seconds(float days);

// ---- .inki deck file ------------------------------------------------------

typedef struct {
  uint32_t card_count;
  uint32_t index_offset;
  uint32_t state_offset;
  uint32_t log_offset;
  uint16_t name_length;
} InkiDeckHeader;

typedef struct {
  uint64_t anki_card_id;
  uint32_t text_offset;
  uint32_t text_length;
} InkiCardIndexEntry;

typedef struct {
  InkiMemoryState memory;
  uint32_t last_review;  // Unix seconds; 0 = new card
  uint32_t due;          // Unix seconds
} InkiStateRecord;

#define INKI_REVIEW_FLAG_WAS_NEW 0x01

typedef struct {
  uint64_t anki_card_id;
  uint32_t card_index;
  uint32_t review_time;  // Unix seconds
  uint8_t rating;        // 1..4
  uint8_t flags;         // INKI_REVIEW_FLAG_WAS_NEW
  uint16_t answer_seconds;
  float elapsed_days;
  float stability;
  float difficulty;
} InkiReviewLogEntry;

void inki_encode_deck_header(const InkiDeckHeader* header, uint8_t* out /* 32 bytes */);
// Checks magic, version and that every region fits in `file_size` bytes.
bool inki_decode_deck_header(const uint8_t* in /* 32 bytes */, uint64_t file_size, InkiDeckHeader* out);
// Complete answer-log entries; a partly written last entry is not counted.
uint64_t inki_review_log_entries(const InkiDeckHeader* header, uint64_t file_size);
// Where the next answer goes: after the last complete entry.
uint64_t inki_review_log_append_offset(const InkiDeckHeader* header, uint64_t file_size);

void inki_encode_card_index_entry(const InkiCardIndexEntry* entry, uint8_t* out /* 16 bytes */);
void inki_decode_card_index_entry(const uint8_t* in /* 16 bytes */, InkiCardIndexEntry* out);
void inki_encode_state_record(const InkiStateRecord* record, uint8_t* out /* 20 bytes */);
void inki_decode_state_record(const uint8_t* in /* 20 bytes */, InkiStateRecord* out);
void inki_encode_review_log_entry(const InkiReviewLogEntry* entry, uint8_t* out /* 32 bytes */);
void inki_decode_review_log_entry(const uint8_t* in /* 32 bytes */, InkiReviewLogEntry* out);

// Splits card text at the first NUL into question and answer. The answer
// starts at text + *back_offset. Text without a NUL is all question.
void inki_split_card_text(const char* text, size_t length, size_t* front_length, size_t* back_offset,
                          size_t* back_length);

#ifdef __cplusplus
}
#endif
