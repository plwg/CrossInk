// C ABI wrapper so Rust tests can call the firmware library.
#include <Fsrs7.h>

extern "C" {

struct Fsrs7State {
  float stability;
  float difficulty;
  float stability_fast;
};

static fsrs7::MemoryState toCpp(const Fsrs7State& s) { return {s.stability, s.difficulty, s.stability_fast}; }
static Fsrs7State toC(const fsrs7::MemoryState& s) { return {s.stability, s.difficulty, s.stabilityFast}; }

static fsrs7::Params paramsFrom(const float* w) {
  fsrs7::Params p;
  for (size_t i = 0; i < fsrs7::kParamCount; ++i) p.w[i] = w[i];
  fsrs7::clipParams(p);
  return p;
}

void fsrs7_default_params(float* out) {
  for (size_t i = 0; i < fsrs7::kParamCount; ++i) out[i] = fsrs7::kDefaultParams.w[i];
}

void fsrs7_clip(float* w) {
  fsrs7::Params p;
  for (size_t i = 0; i < fsrs7::kParamCount; ++i) p.w[i] = w[i];
  fsrs7::clipParams(p);
  for (size_t i = 0; i < fsrs7::kParamCount; ++i) w[i] = p.w[i];
}

float fsrs7_retrievability(const float* w, Fsrs7State state, float elapsed) {
  return fsrs7::retrievability(paramsFrom(w), toCpp(state), elapsed);
}

Fsrs7State fsrs7_initial_state(const float* w, unsigned rating) {
  return toC(fsrs7::initialState(paramsFrom(w), static_cast<fsrs7::Rating>(rating)));
}

Fsrs7State fsrs7_next_state(const float* w, Fsrs7State state, float elapsed, unsigned rating) {
  return toC(fsrs7::nextState(paramsFrom(w), toCpp(state), elapsed, static_cast<fsrs7::Rating>(rating)));
}

float fsrs7_next_interval(const float* w, Fsrs7State state, float desired_retention) {
  return fsrs7::nextIntervalDays(paramsFrom(w), toCpp(state), desired_retention);
}

// Writes 4 states and 4 intervals (again, hard, good, easy). `current` may be null.
void fsrs7_next_states(const float* w, const Fsrs7State* current, float desired_retention, float elapsed,
                       Fsrs7State* states_out, float* intervals_out) {
  fsrs7::MemoryState cur{};
  if (current != nullptr) cur = toCpp(*current);
  const fsrs7::NextStates next =
      fsrs7::nextStates(paramsFrom(w), current != nullptr ? &cur : nullptr, desired_retention, elapsed);
  const fsrs7::ScheduledState* all[4] = {&next.again, &next.hard, &next.good, &next.easy};
  for (int i = 0; i < 4; ++i) {
    states_out[i] = toC(all[i]->memory);
    intervals_out[i] = all[i]->intervalDays;
  }
}

// Returns 1 on success and writes 34 params + retention; 0 on parse error.
int fsrs7_parse_config(const char* text, size_t len, float* params_out, float* retention_out) {
  fsrs7::Config config = fsrs7::defaultConfig();
  if (!fsrs7::parseConfig(text, len, config)) return 0;
  for (size_t i = 0; i < fsrs7::kParamCount; ++i) params_out[i] = config.params.w[i];
  *retention_out = config.desiredRetention;
  return 1;
}
}
