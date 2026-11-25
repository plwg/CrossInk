#pragma once

// FSRS-7 spaced-repetition scheduler for the e-reader firmware.
//
// A port of the FSRS-7 scalar model from fsrs-rs v7.0.0 (BSD-3-Clause,
// Open Spaced Repetition; see LICENSE-fsrs-rs). It uses single-precision
// floats only, allocates nothing and keeps 12 bytes of state per card, so it
// runs on the ESP32-C3 without a hardware FPU. Time is measured in
// (fractional) days.
//
// Parameters are trained on a PC by the `inki params` command and stored on
// the SD card in the text format read by parseConfig().

#include <cstddef>
#include <cstdint>

namespace fsrs7 {

constexpr size_t kParamCount = 34;

constexpr float kStabilityMin = 0.0001f;
constexpr float kStabilityMax = 36500.0f;
constexpr float kDifficultyMin = 1.0f;
constexpr float kDifficultyMax = 10.0f;
constexpr float kDefaultDesiredRetention = 0.9f;

enum class Rating : uint8_t { Again = 1, Hard = 2, Good = 3, Easy = 4 };

struct Params {
  float w[kParamCount];
};

// FSRS-7 defaults from fsrs-rs v7.0.0, used until the user trains their own.
extern const Params kDefaultParams;

// Memory state of one card. A card that has never been reviewed has no state;
// use initialState() for its first review.
struct MemoryState {
  float stability;      // long-term stability, in days
  float difficulty;     // 1..10
  float stabilityFast;  // short-term stability, in days
};

struct ScheduledState {
  MemoryState memory;
  float intervalDays;
};

struct NextStates {
  ScheduledState again;
  ScheduledState hard;
  ScheduledState good;
  ScheduledState easy;

  const ScheduledState& forRating(Rating rating) const;
};

// Clamps parameters into the ranges fsrs-rs accepts. Non-finite values are
// replaced by the lower bound, as fsrs-rs does.
void clipParams(Params& params);

// Returns true when every parameter is finite.
bool paramsFinite(const Params& params);

// Probability of recall after `elapsedDays` days.
float retrievability(const Params& params, const MemoryState& state, float elapsedDays);

// State after the first review of a new card.
MemoryState initialState(const Params& params, Rating rating);

// State after reviewing a card with `rating`, `elapsedDays` after its previous review.
MemoryState nextState(const Params& params, const MemoryState& state, float elapsedDays, Rating rating);

// Days until recall probability falls to `desiredRetention` (0.0001..0.9999).
float nextIntervalDays(const Params& params, const MemoryState& state, float desiredRetention);

// Outcome of each possible rating for a card that may be new (`current` == nullptr).
NextStates nextStates(const Params& params, const MemoryState* current, float desiredRetention, float elapsedDays);

// Settings read from the SD-card parameter file.
struct Config {
  Params params;
  float desiredRetention;
};

// Parses the parameter file written by `inki params`:
//
//   # comment
//   fsrs7 <34 comma-separated floats>
//   desired_retention 0.9
//
// Unknown keys are ignored. Missing keys keep the defaults. Returns false if
// a known key has a malformed value; `out` is then left unchanged. Parameters
// are clipped before being stored.
bool parseConfig(const char* text, size_t length, Config& out);

// Default config: kDefaultParams with kDefaultDesiredRetention.
Config defaultConfig();

}  // namespace fsrs7
