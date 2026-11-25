#include "Fsrs7.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iterator>

namespace fsrs7 {

namespace {

constexpr float kRetentionMin = 0.0001f;
constexpr float kRetentionMax = 0.9999f;
constexpr int kNewtonIterations = 7;
constexpr int kBisectionIterations = 50;
constexpr float kMinIntervalDays = 1.0f / 86400.0f;
constexpr float kInitialStabilityMax = 100.0f;

// Same as Rust's f32::clamp: a NaN value passes through unchanged.
float clampf(float value, float low, float high) {
  if (value < low) return low;
  if (value > high) return high;
  return value;
}

float clampSafe(float value, float low, float high) {
  if (!std::isfinite(low)) low = 0.0f;
  if (!std::isfinite(high)) high = low;
  if (low > high) {
    const float swap = low;
    low = high;
    high = swap;
  }
  if (!std::isfinite(value)) value = low;
  return clampf(value, low, high);
}

int ratingIndex(Rating rating) {
  const int value = static_cast<int>(rating);
  return value < 1 ? 1 : (value > 4 ? 4 : value);
}

float initDifficulty(const float* w, int rating) {
  return w[4] - std::exp(w[5] * static_cast<float>(rating - 1)) + 1.0f;
}

float nextDifficulty(const float* w, float d, int rating, float retention) {
  float deltaD = -w[6] * (static_cast<float>(rating) - 3.0f);
  if (rating == 1) deltaD *= retention + 0.1f;
  const float damped = d + (10.0f - d) * deltaD / 9.0f;
  const float reverted = initDifficulty(w, 4) * 0.01f + damped * 0.99f;
  return clampf(reverted, kDifficultyMin, kDifficultyMax);
}

// Stability update for one trace. `start` is 7 for the long-term trace and
// 15 for the short-term trace.
float nextStability(const float* w, float lastS, float lastD, float r, int rating, int start) {
  const float hardPenalty = rating == 2 ? w[start + 6] : 1.0f;
  const float easyBonus = rating == 4 ? w[start + 7] : 1.0f;
  const float newSFail =
      w[start + 3] * (std::pow(lastS + 1.0f, w[start + 4]) - 1.0f) * std::exp((1.0f - r) * w[start + 5]);
  const float pls = std::fmin(lastS, newSFail);
  // exp(x) - 1 rather than expm1(x) here and in fastFactor() to match fsrs-rs bit for bit.
  // cppcheck-suppress unpreciseMathCall
  const float recallGain = std::exp((1.0f - r) * w[start + 2]) - 1.0f;
  const float sinc = std::exp(w[start] - 1.5f) * (11.0f - lastD) * std::pow(lastS, -w[start + 1]) * recallGain *
                         hardPenalty * easyBonus +
                     1.0f;
  const float newSSuccess = std::fmax(pls, lastS * sinc);
  return clampf(rating > 1 ? newSSuccess : pls, kStabilityMin, kStabilityMax);
}

float fastDecay(const float* w, float sFast) { return -clampf(w[23] * std::pow(sFast, w[33] - 0.3f), 0.01f, 0.95f); }

float fastFactor(const float* w, float decay1) {
  // cppcheck-suppress unpreciseMathCall
  return std::exp(std::fmin(std::log(w[25]) / decay1, 60.0f)) - 1.0f;
}

float fastComponentRecall(const float* w, float t, float sFast) {
  t = std::fmax(t, 0.0f);
  sFast = clampf(sFast, kStabilityMin, kStabilityMax);
  const float decay1 = fastDecay(w, sFast);
  const float factor1 = fastFactor(w, decay1);
  return std::pow(1.0f + factor1 * (t / sFast), decay1);
}

struct CurveTerms {
  float weight1;
  float weight2;
  float factor1;
  float factor2;
  float decay1;
  float decay2;
  float dTimescale;
  float s;
  float sFast;
};

CurveTerms curveTerms(const float* w, const MemoryState& state) {
  CurveTerms c;
  c.s = std::fmax(state.stability, kStabilityMin);
  c.sFast = std::fmax(state.stabilityFast, kStabilityMin);
  const float d = clampf(state.difficulty, kDifficultyMin, kDifficultyMax);
  c.decay1 = fastDecay(w, c.sFast);
  c.factor1 = fastFactor(w, c.decay1);
  c.decay2 = -clampf(w[24], 0.01f, 0.95f);
  c.factor2 = std::pow(w[26], 1.0f / c.decay2) - 1.0f;
  c.dTimescale = std::exp((d - 5.0f) * (w[32] - 0.3f));
  c.weight1 = w[27] * std::pow(c.sFast, -w[29]);
  c.weight2 = w[28] * std::pow(c.s, w[30]) * std::exp((d - 5.0f) * (w[31] - 0.5f));
  return c;
}

float curve(const float* w, float t, const MemoryState& state) {
  t = std::fmax(t, 0.0f);
  const CurveTerms c = curveTerms(w, state);
  const float r1 = std::pow(1.0f + c.factor1 * (t / c.sFast), c.decay1);
  const float r2 = std::pow(1.0f + c.factor2 * c.dTimescale * (t / c.s), c.decay2);
  const float retention = (c.weight1 * r1 + c.weight2 * r2) / (c.weight1 + c.weight2);
  return std::fma(retention, 1.0f - 2e-5f, 1e-5f);
}

void curveAndDerivative(const float* w, float t, const MemoryState& state, float& retentionOut, float& derivativeOut) {
  t = std::fmax(t, 0.0f);
  const CurveTerms c = curveTerms(w, state);
  const float b1 = 1.0f + c.factor1 * (t / c.sFast);
  const float r1 = std::pow(b1, c.decay1);
  const float dr1 = c.decay1 * std::pow(b1, c.decay1 - 1.0f) * c.factor1 / c.sFast;
  const float b2 = 1.0f + c.factor2 * c.dTimescale * (t / c.s);
  const float r2 = std::pow(b2, c.decay2);
  const float dr2 = c.decay2 * std::pow(b2, c.decay2 - 1.0f) * c.factor2 * c.dTimescale / c.s;
  const float weightSum = std::fmax(c.weight1 + c.weight2, 1e-9f);
  const float retention = (c.weight1 * r1 + c.weight2 * r2) / weightSum;
  const float derivative = (c.weight1 * dr1 + c.weight2 * dr2) / weightSum;
  retentionOut = std::fma(retention, 1.0f - 2e-5f, 1e-5f);
  derivativeOut = derivative * (1.0f - 2e-5f);
}

MemoryState clampState(const MemoryState& state) {
  return MemoryState{clampf(state.stability, kStabilityMin, kStabilityMax),
                     clampf(state.difficulty, kDifficultyMin, kDifficultyMax),
                     clampf(state.stabilityFast, kStabilityMin, kStabilityMax)};
}

float intervalByBisection(const float* w, const MemoryState& state, float desiredRetention) {
  float low = 0.0f;
  float high = std::fmax(std::fmax(state.stability, state.stabilityFast), 1.0f);
  while (curve(w, high, state) > desiredRetention && high < kStabilityMax) {
    high = std::fmin(high * 2.0f, kStabilityMax);
  }
  for (int i = 0; i < kBisectionIterations; ++i) {
    const float mid = (low + high) * 0.5f;
    if (curve(w, mid, state) > desiredRetention) {
      low = mid;
    } else {
      high = mid;
    }
  }
  return clampf((low + high) * 0.5f, 0.0f, kStabilityMax);
}

const char* skipSpaces(const char* p, const char* end) {
  while (p < end && (*p == ' ' || *p == '\t')) ++p;
  return p;
}

bool keyIs(const char* p, const char* end, const char* key) {
  const size_t n = std::strlen(key);
  if (static_cast<size_t>(end - p) < n || std::strncmp(p, key, n) != 0) return false;
  return p + n == end || p[n] == ' ' || p[n] == '\t';
}

// Parses one float from [p, end). Advances p past it. The value is copied to a
// small buffer first because the input is not NUL-terminated.
bool parseFloat(const char*& p, const char* end, float& out) {
  p = skipSpaces(p, end);
  char buf[32];
  size_t n = 0;
  while (p + n < end && n < sizeof(buf) - 1 && p[n] != ',' && p[n] != ' ' && p[n] != '\t') {
    buf[n] = p[n];
    ++n;
  }
  if (n == 0 || n == sizeof(buf) - 1) return false;
  buf[n] = '\0';
  char* parsedEnd = nullptr;
  const float value = std::strtof(buf, &parsedEnd);
  if (parsedEnd != buf + n || !std::isfinite(value)) return false;
  out = value;
  p += n;
  p = skipSpaces(p, end);
  return true;
}

}  // namespace

const Params kDefaultParams = {{
    0.1104f, 2.2395f, 3.9221f, 11.7841f, 6.1686f, 0.6457f, 3.6807f, 1.9795f, 0.0f,    1.3826f, 0.7024f, 0.5999f,
    0.8146f, 0.6398f, 1.0f,    1.3207f,  0.6707f, 3.8668f, 0.4416f, 0.0934f, 1.8631f, 0.6162f, 1.0869f, 0.1567f,
    0.0801f, 0.2421f, 0.9464f, 0.1433f,  0.7145f, 0.0f,    0.5667f, 0.3734f, 0.5333f, 0.3048f,
}};

const ScheduledState& NextStates::forRating(Rating rating) const {
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

void clipParams(Params& params) {
  auto& p = params.w;
  p[0] = clampSafe(p[0], kStabilityMin, kInitialStabilityMax / 2.0f);
  p[1] = clampSafe(p[1], p[0], kInitialStabilityMax);
  p[2] = clampSafe(p[2], p[1], kInitialStabilityMax);
  p[3] = clampSafe(p[3], p[2], kInitialStabilityMax);

  p[4] = clampSafe(p[4], 1.0f, 10.0f);
  p[5] = clampSafe(p[5], 0.001f, 4.0f);
  p[6] = clampSafe(p[6], 0.1f, 4.0f);

  p[7] = clampSafe(p[7], 0.0f, 4.0f);
  p[8] = clampSafe(p[8], 0.0f, 1.2f);
  p[9] = clampSafe(p[9], 0.3f, 3.0f);
  p[10] = clampSafe(p[10], 0.01f, 1.5f);
  p[11] = clampSafe(p[11], 0.1f, 1.0f);
  p[12] = clampSafe(p[12], 0.0f, 3.5f);
  p[13] = clampSafe(p[13], 0.0f, 1.0f);
  p[14] = clampSafe(p[14], 1.0f, 7.0f);

  p[15] = clampSafe(p[15], 0.0f, 4.0f);
  p[16] = clampSafe(p[16], 0.0f, 2.0f);
  p[17] = clampSafe(p[17], 0.5f, 6.0f);
  p[18] = clampSafe(p[18], 0.001f, 1.5f);
  p[19] = clampSafe(p[19], 0.001f, 1.0f);
  p[20] = clampSafe(p[20], 0.0f, 5.0f);
  p[21] = clampSafe(p[21], 0.0f, 1.0f);
  p[22] = clampSafe(p[22], 1.0f, 7.0f);

  p[23] = clampSafe(p[23], 0.01f, 0.25f);
  p[24] = clampSafe(p[24], 0.01f, 0.95f);
  p[25] = clampSafe(p[25], 0.2f, 0.85f);
  p[26] = clampSafe(p[26], std::fmax(p[25], 0.5f), 0.99f);
  p[27] = clampSafe(p[27], 0.01f, 1.0f);
  p[28] = clampSafe(p[28], 0.1f, 1.0f);
  p[29] = clampSafe(p[29], 0.0f, 0.9f);
  p[30] = clampSafe(p[30], 0.1f, 1.1f);
  p[31] = clampSafe(p[31], 0.0f, 1.0f);
  p[32] = clampSafe(p[32], 0.0f, 0.6f);
  p[33] = clampSafe(p[33], 0.0f, 0.6f);
}

bool paramsFinite(const Params& params) {
  return std::all_of(std::begin(params.w), std::end(params.w), [](float value) { return std::isfinite(value); });
}

float retrievability(const Params& params, const MemoryState& state, float elapsedDays) {
  return curve(params.w, std::fmax(elapsedDays, 0.0f), state);
}

MemoryState initialState(const Params& params, Rating rating) {
  const int r = ratingIndex(rating);
  const float s = clampf(params.w[r - 1], kStabilityMin, kStabilityMax);
  return MemoryState{s, clampf(initDifficulty(params.w, r), kDifficultyMin, kDifficultyMax),
                     clampf(s * 0.8f, kStabilityMin, kStabilityMax)};
}

MemoryState nextState(const Params& params, const MemoryState& state, float elapsedDays, Rating rating) {
  const float* w = params.w;
  const MemoryState last = clampState(state);
  const float t = std::fmax(elapsedDays, 0.0f);
  const int r = ratingIndex(rating);
  const float recall = curve(w, t, last);
  const float sSlow = nextStability(w, last.stability, last.difficulty, recall, r, 7);
  const float rFast = fastComponentRecall(w, t, last.stabilityFast);
  float sFast = nextStability(w, last.stabilityFast, last.difficulty, rFast, r, 15);
  if (r == 1) sFast = std::fmin(sFast, sSlow * 0.8f);
  return MemoryState{sSlow, nextDifficulty(w, last.difficulty, r, recall), clampf(sFast, kStabilityMin, kStabilityMax)};
}

float nextIntervalDays(const Params& params, const MemoryState& stateIn, float desiredRetention) {
  const float* w = params.w;
  desiredRetention = clampf(desiredRetention, kRetentionMin, kRetentionMax);
  if (desiredRetention >= kRetentionMax) return 0.0f;

  const MemoryState state = clampState(stateIn);
  const float minLogT = std::log(kMinIntervalDays);
  const float maxLogT = std::log(kStabilityMax);
  float logT = std::log(std::fmax(std::fmax(state.stability, state.stabilityFast), kMinIntervalDays));
  for (int i = 0; i < kNewtonIterations; ++i) {
    logT = clampf(logT, minLogT, maxLogT);
    const float t = clampf(std::exp(logT), kMinIntervalDays, kStabilityMax);
    float recall = 0.0f;
    float derivative = 0.0f;
    curveAndDerivative(w, t, state, recall, derivative);
    const float slope = std::fmin(derivative * t, -1e-12f);
    logT -= clampf((recall - desiredRetention) / slope, -4.0f, 4.0f);
    if (!std::isfinite(logT)) return intervalByBisection(w, state, desiredRetention);
  }
  const float interval = clampf(std::exp(logT), 0.0f, kStabilityMax);
  const float recall = curve(w, interval, state);
  if (std::isfinite(recall) && std::fabs(recall - desiredRetention) <= 1e-3f) return interval;
  return intervalByBisection(w, state, desiredRetention);
}

NextStates nextStates(const Params& params, const MemoryState* current, float desiredRetention, float elapsedDays) {
  ScheduledState out[4];
  for (int r = 1; r <= 4; ++r) {
    const Rating rating = static_cast<Rating>(r);
    const MemoryState memory =
        current == nullptr ? initialState(params, rating) : nextState(params, *current, elapsedDays, rating);
    out[r - 1] = ScheduledState{memory, nextIntervalDays(params, memory, desiredRetention)};
  }
  return NextStates{out[0], out[1], out[2], out[3]};
}

Config defaultConfig() { return Config{kDefaultParams, kDefaultDesiredRetention}; }

bool parseConfig(const char* text, size_t length, Config& out) {
  Config parsed = out;
  const char* p = text;
  const char* end = text + length;
  while (p < end) {
    const char* lineEnd = static_cast<const char*>(std::memchr(p, '\n', static_cast<size_t>(end - p)));
    if (lineEnd == nullptr) lineEnd = end;
    const char* next = lineEnd < end ? lineEnd + 1 : end;
    if (lineEnd > p && lineEnd[-1] == '\r') --lineEnd;

    const char* q = skipSpaces(p, lineEnd);
    if (q < lineEnd && *q != '#') {
      if (keyIs(q, lineEnd, "fsrs7")) {
        q += 5;
        for (size_t i = 0; i < kParamCount; ++i) {
          if (!parseFloat(q, lineEnd, parsed.params.w[i])) return false;
          if (i + 1 < kParamCount) {
            if (q >= lineEnd || *q != ',') return false;
            ++q;
          }
        }
        if (skipSpaces(q, lineEnd) != lineEnd) return false;
      } else if (keyIs(q, lineEnd, "desired_retention")) {
        q += std::strlen("desired_retention");
        float value = 0.0f;
        if (!parseFloat(q, lineEnd, value) || q != lineEnd) return false;
        if (value < kRetentionMin || value > kRetentionMax) return false;
        parsed.desiredRetention = value;
      }
    }
    p = next;
  }
  clipParams(parsed.params);
  out = parsed;
  return true;
}

}  // namespace fsrs7
