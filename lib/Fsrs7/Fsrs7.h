#pragma once

// FSRS-7 spaced-repetition scheduling for flashcards.
//
// A C++ view of the Rust crate inki-core (in the inki repository, linked by
// scripts/build_inki_core.py), which the inki PC tool uses too, so the PC
// and the reader schedule cards with the same code. inki-core ports FSRS-7
// from fsrs-rs v7.0.0 (BSD-3-Clause, Open Spaced Repetition). Floats only,
// no allocation, 12 bytes of state per card. Time is in (fractional) days.

#include <inki_core.h>

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace fsrs7 {

constexpr size_t kParamCount = INKI_PARAM_COUNT;
constexpr float kDefaultDesiredRetention = 0.9f;

enum class Rating : uint8_t { Again = 1, Hard = 2, Good = 3, Easy = 4 };

struct Params {
  float w[kParamCount];
};

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

  const ScheduledState& forRating(Rating rating) const {
    switch (rating) {
      case Rating::Again:
        return again;
      case Rating::Hard:
        return hard;
      case Rating::Easy:
        return easy;
      case Rating::Good:
      default:
        return good;
    }
  }
};

struct Config {
  Params params;
  float desiredRetention;
};

namespace detail {
inline InkiMemoryState toInki(const MemoryState& s) {
  return InkiMemoryState{s.stability, s.difficulty, s.stabilityFast};
}
inline MemoryState fromInki(const InkiMemoryState& s) {
  return MemoryState{s.stability, s.difficulty, s.stability_fast};
}
inline ScheduledState fromInki(const InkiScheduled& s) { return ScheduledState{fromInki(s.memory), s.interval_days}; }
}  // namespace detail

// FSRS-7 defaults with 90% desired retention.
inline Config defaultConfig() {
  InkiDeckSettings defaults{};
  inki_default_deck_settings(&defaults);
  Config out{};
  std::memcpy(out.params.w, defaults.params, sizeof(out.params.w));
  out.desiredRetention = defaults.desired_retention;
  return out;
}

// FSRS-7 defaults from fsrs-rs v7.0.0.
inline const Params kDefaultParams = defaultConfig().params;

// Probability of recall after `elapsedDays` days.
inline float retrievability(const Params& params, const MemoryState& state, float elapsedDays) {
  const InkiMemoryState s = detail::toInki(state);
  return inki_retrievability(params.w, &s, elapsedDays);
}

// State after the first review of a new card.
inline MemoryState initialState(const Params& params, Rating rating) {
  InkiMemoryState out{};
  inki_initial_state(params.w, static_cast<uint8_t>(rating), &out);
  return detail::fromInki(out);
}

// State after reviewing a card with `rating`, `elapsedDays` after its previous review.
inline MemoryState nextState(const Params& params, const MemoryState& state, float elapsedDays, Rating rating) {
  const InkiMemoryState s = detail::toInki(state);
  InkiMemoryState out{};
  inki_next_state(params.w, &s, elapsedDays, static_cast<uint8_t>(rating), &out);
  return detail::fromInki(out);
}

// Days until recall probability falls to `desiredRetention` (0.0001..0.9999).
inline float nextIntervalDays(const Params& params, const MemoryState& state, float desiredRetention) {
  const InkiMemoryState s = detail::toInki(state);
  return inki_next_interval_days(params.w, &s, desiredRetention);
}

// Outcome of each possible rating for a card that may be new (`current` == nullptr).
inline NextStates nextStates(const Params& params, const MemoryState* current, float desiredRetention,
                             float elapsedDays) {
  InkiMemoryState s{};
  if (current != nullptr) s = detail::toInki(*current);
  InkiNextStates out{};
  inki_next_states(params.w, current != nullptr ? &s : nullptr, desiredRetention, elapsedDays, &out);
  return NextStates{detail::fromInki(out.again), detail::fromInki(out.hard), detail::fromInki(out.good),
                    detail::fromInki(out.easy)};
}

}  // namespace fsrs7
