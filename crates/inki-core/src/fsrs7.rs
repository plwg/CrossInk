//! FSRS-7 spaced-repetition scheduling.
//!
//! A port of the FSRS-7 scalar model from fsrs-rs v7.0.0 (BSD-3-Clause,
//! Open Spaced Repetition; see LICENSE-fsrs-rs). Single-precision floats, no
//! allocation and 12 bytes of state per card, so it runs on the ESP32-C3
//! without a hardware FPU. Time is measured in (fractional) days.

use libm::{expf, fabsf, fmaf, fmaxf, fminf, logf, powf};

pub const PARAM_COUNT: usize = 34;

pub const STABILITY_MIN: f32 = 0.0001;
pub const STABILITY_MAX: f32 = 36_500.0;
pub const DIFFICULTY_MIN: f32 = 1.0;
pub const DIFFICULTY_MAX: f32 = 10.0;
pub const DEFAULT_DESIRED_RETENTION: f32 = 0.9;
pub const RETENTION_MIN: f32 = 0.0001;
pub const RETENTION_MAX: f32 = 0.9999;

const NEWTON_ITERATIONS: usize = 7;
const BISECTION_ITERATIONS: usize = 50;
const MIN_INTERVAL_DAYS: f32 = 1.0 / 86_400.0;
const INITIAL_STABILITY_MAX: f32 = 100.0;

pub type Params = [f32; PARAM_COUNT];

/// FSRS-7 defaults from fsrs-rs v7.0.0, used until the user trains their own.
pub const DEFAULT_PARAMS: Params = [
    0.1104, 2.2395, 3.9221, 11.7841, 6.1686, 0.6457, 3.6807, 1.9795, 0.0, 1.3826, 0.7024, 0.5999,
    0.8146, 0.6398, 1.0, 1.3207, 0.6707, 3.8668, 0.4416, 0.0934, 1.8631, 0.6162, 1.0869, 0.1567,
    0.0801, 0.2421, 0.9464, 0.1433, 0.7145, 0.0, 0.5667, 0.3734, 0.5333, 0.3048,
];

/// Answer buttons, as Anki numbers them.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    /// Out-of-range values are clamped, as fsrs-rs does.
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 | 1 => Self::Again,
            2 => Self::Hard,
            3 => Self::Good,
            _ => Self::Easy,
        }
    }
}

/// Memory state of one card. A card that has never been reviewed has none;
/// use [`initial_state`] for its first review.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MemoryState {
    /// Long-term stability, in days.
    pub stability: f32,
    /// 1..10.
    pub difficulty: f32,
    /// Short-term stability, in days.
    pub stability_fast: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scheduled {
    pub memory: MemoryState,
    pub interval_days: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NextStates {
    pub again: Scheduled,
    pub hard: Scheduled,
    pub good: Scheduled,
    pub easy: Scheduled,
}

impl NextStates {
    pub fn for_rating(&self, rating: Rating) -> &Scheduled {
        match rating {
            Rating::Again => &self.again,
            Rating::Hard => &self.hard,
            Rating::Good => &self.good,
            Rating::Easy => &self.easy,
        }
    }
}

/// Rust's `f32::clamp` without the panic on bad bounds; NaN passes through.
fn clampf(value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

fn clamp_safe(value: f32, low: f32, high: f32) -> f32 {
    let low = if low.is_finite() { low } else { 0.0 };
    let high = if high.is_finite() { high } else { low };
    let (low, high) = if low <= high {
        (low, high)
    } else {
        (high, low)
    };
    let value = if value.is_finite() { value } else { low };
    clampf(value, low, high)
}

/// Clamps parameters into the ranges fsrs-rs accepts. Non-finite values are
/// replaced by the lower bound, as fsrs-rs does.
pub fn clip_params(p: &mut Params) {
    p[0] = clamp_safe(p[0], STABILITY_MIN, INITIAL_STABILITY_MAX / 2.0);
    p[1] = clamp_safe(p[1], p[0], INITIAL_STABILITY_MAX);
    p[2] = clamp_safe(p[2], p[1], INITIAL_STABILITY_MAX);
    p[3] = clamp_safe(p[3], p[2], INITIAL_STABILITY_MAX);

    p[4] = clamp_safe(p[4], 1.0, 10.0);
    p[5] = clamp_safe(p[5], 0.001, 4.0);
    p[6] = clamp_safe(p[6], 0.1, 4.0);

    p[7] = clamp_safe(p[7], 0.0, 4.0);
    p[8] = clamp_safe(p[8], 0.0, 1.2);
    p[9] = clamp_safe(p[9], 0.3, 3.0);
    p[10] = clamp_safe(p[10], 0.01, 1.5);
    p[11] = clamp_safe(p[11], 0.1, 1.0);
    p[12] = clamp_safe(p[12], 0.0, 3.5);
    p[13] = clamp_safe(p[13], 0.0, 1.0);
    p[14] = clamp_safe(p[14], 1.0, 7.0);

    p[15] = clamp_safe(p[15], 0.0, 4.0);
    p[16] = clamp_safe(p[16], 0.0, 2.0);
    p[17] = clamp_safe(p[17], 0.5, 6.0);
    p[18] = clamp_safe(p[18], 0.001, 1.5);
    p[19] = clamp_safe(p[19], 0.001, 1.0);
    p[20] = clamp_safe(p[20], 0.0, 5.0);
    p[21] = clamp_safe(p[21], 0.0, 1.0);
    p[22] = clamp_safe(p[22], 1.0, 7.0);

    p[23] = clamp_safe(p[23], 0.01, 0.25);
    p[24] = clamp_safe(p[24], 0.01, 0.95);
    p[25] = clamp_safe(p[25], 0.2, 0.85);
    p[26] = clamp_safe(p[26], fmaxf(p[25], 0.5), 0.99);
    p[27] = clamp_safe(p[27], 0.01, 1.0);
    p[28] = clamp_safe(p[28], 0.1, 1.0);
    p[29] = clamp_safe(p[29], 0.0, 0.9);
    p[30] = clamp_safe(p[30], 0.1, 1.1);
    p[31] = clamp_safe(p[31], 0.0, 1.0);
    p[32] = clamp_safe(p[32], 0.0, 0.6);
    p[33] = clamp_safe(p[33], 0.0, 0.6);
}

fn init_difficulty(w: &Params, rating: u8) -> f32 {
    w[4] - expf(w[5] * f32::from(rating - 1)) + 1.0
}

fn next_difficulty(w: &Params, d: f32, rating: u8, retention: f32) -> f32 {
    let mut delta = -w[6] * (f32::from(rating) - 3.0);
    if rating == 1 {
        delta *= retention + 0.1;
    }
    let damped = d + (10.0 - d) * delta / 9.0;
    let reverted = init_difficulty(w, 4) * 0.01 + damped * 0.99;
    clampf(reverted, DIFFICULTY_MIN, DIFFICULTY_MAX)
}

/// Stability update for one trace: `start` 7 for long-term, 15 for short-term.
fn next_stability(w: &Params, last_s: f32, last_d: f32, r: f32, rating: u8, start: usize) -> f32 {
    let hard_penalty = if rating == 2 { w[start + 6] } else { 1.0 };
    let easy_bonus = if rating == 4 { w[start + 7] } else { 1.0 };
    let new_s_fail =
        w[start + 3] * (powf(last_s + 1.0, w[start + 4]) - 1.0) * expf((1.0 - r) * w[start + 5]);
    let pls = fminf(last_s, new_s_fail);
    // exp(x) - 1 rather than expm1(x), as fsrs-rs computes it.
    let recall_gain = expf((1.0 - r) * w[start + 2]) - 1.0;
    let sinc = expf(w[start] - 1.5)
        * (11.0 - last_d)
        * powf(last_s, -w[start + 1])
        * recall_gain
        * hard_penalty
        * easy_bonus
        + 1.0;
    let new_s_success = fmaxf(pls, last_s * sinc);
    clampf(
        if rating > 1 { new_s_success } else { pls },
        STABILITY_MIN,
        STABILITY_MAX,
    )
}

fn fast_decay(w: &Params, s_fast: f32) -> f32 {
    -clampf(w[23] * powf(s_fast, w[33] - 0.3), 0.01, 0.95)
}

fn fast_factor(w: &Params, decay1: f32) -> f32 {
    expf(fminf(logf(w[25]) / decay1, 60.0)) - 1.0
}

fn fast_component_recall(w: &Params, t: f32, s_fast: f32) -> f32 {
    let t = fmaxf(t, 0.0);
    let s_fast = clampf(s_fast, STABILITY_MIN, STABILITY_MAX);
    let decay1 = fast_decay(w, s_fast);
    let factor1 = fast_factor(w, decay1);
    powf(1.0 + factor1 * (t / s_fast), decay1)
}

struct CurveTerms {
    weight1: f32,
    weight2: f32,
    factor1: f32,
    factor2: f32,
    decay1: f32,
    decay2: f32,
    d_timescale: f32,
    s: f32,
    s_fast: f32,
}

fn curve_terms(w: &Params, state: &MemoryState) -> CurveTerms {
    let s = fmaxf(state.stability, STABILITY_MIN);
    let s_fast = fmaxf(state.stability_fast, STABILITY_MIN);
    let d = clampf(state.difficulty, DIFFICULTY_MIN, DIFFICULTY_MAX);
    let decay1 = fast_decay(w, s_fast);
    let decay2 = -clampf(w[24], 0.01, 0.95);
    CurveTerms {
        weight1: w[27] * powf(s_fast, -w[29]),
        weight2: w[28] * powf(s, w[30]) * expf((d - 5.0) * (w[31] - 0.5)),
        factor1: fast_factor(w, decay1),
        factor2: powf(w[26], 1.0 / decay2) - 1.0,
        decay1,
        decay2,
        d_timescale: expf((d - 5.0) * (w[32] - 0.3)),
        s,
        s_fast,
    }
}

fn curve(w: &Params, t: f32, state: &MemoryState) -> f32 {
    let t = fmaxf(t, 0.0);
    let c = curve_terms(w, state);
    let r1 = powf(1.0 + c.factor1 * (t / c.s_fast), c.decay1);
    let r2 = powf(1.0 + c.factor2 * c.d_timescale * (t / c.s), c.decay2);
    let retention = (c.weight1 * r1 + c.weight2 * r2) / (c.weight1 + c.weight2);
    fmaf(retention, 1.0 - 2e-5, 1e-5)
}

fn curve_and_derivative(w: &Params, t: f32, state: &MemoryState) -> (f32, f32) {
    let t = fmaxf(t, 0.0);
    let c = curve_terms(w, state);
    let b1 = 1.0 + c.factor1 * (t / c.s_fast);
    let r1 = powf(b1, c.decay1);
    let dr1 = c.decay1 * powf(b1, c.decay1 - 1.0) * c.factor1 / c.s_fast;
    let b2 = 1.0 + c.factor2 * c.d_timescale * (t / c.s);
    let r2 = powf(b2, c.decay2);
    let dr2 = c.decay2 * powf(b2, c.decay2 - 1.0) * c.factor2 * c.d_timescale / c.s;
    let weight_sum = fmaxf(c.weight1 + c.weight2, 1e-9);
    let retention = (c.weight1 * r1 + c.weight2 * r2) / weight_sum;
    let derivative = (c.weight1 * dr1 + c.weight2 * dr2) / weight_sum;
    (fmaf(retention, 1.0 - 2e-5, 1e-5), derivative * (1.0 - 2e-5))
}

fn clamp_state(state: &MemoryState) -> MemoryState {
    MemoryState {
        stability: clampf(state.stability, STABILITY_MIN, STABILITY_MAX),
        difficulty: clampf(state.difficulty, DIFFICULTY_MIN, DIFFICULTY_MAX),
        stability_fast: clampf(state.stability_fast, STABILITY_MIN, STABILITY_MAX),
    }
}

fn interval_by_bisection(w: &Params, state: &MemoryState, desired: f32) -> f32 {
    let mut low = 0.0f32;
    let mut high = fmaxf(fmaxf(state.stability, state.stability_fast), 1.0);
    while curve(w, high, state) > desired && high < STABILITY_MAX {
        high = fminf(high * 2.0, STABILITY_MAX);
    }
    for _ in 0..BISECTION_ITERATIONS {
        let mid = (low + high) * 0.5;
        if curve(w, mid, state) > desired {
            low = mid;
        } else {
            high = mid;
        }
    }
    clampf((low + high) * 0.5, 0.0, STABILITY_MAX)
}

/// Probability of recall after `elapsed_days` days.
pub fn retrievability(w: &Params, state: &MemoryState, elapsed_days: f32) -> f32 {
    curve(w, fmaxf(elapsed_days, 0.0), state)
}

/// State after the first review of a new card.
pub fn initial_state(w: &Params, rating: Rating) -> MemoryState {
    let r = rating as u8;
    let s = clampf(w[usize::from(r - 1)], STABILITY_MIN, STABILITY_MAX);
    MemoryState {
        stability: s,
        difficulty: clampf(init_difficulty(w, r), DIFFICULTY_MIN, DIFFICULTY_MAX),
        stability_fast: clampf(s * 0.8, STABILITY_MIN, STABILITY_MAX),
    }
}

/// State after reviewing a card with `rating`, `elapsed_days` after its
/// previous review.
pub fn next_state(
    w: &Params,
    state: &MemoryState,
    elapsed_days: f32,
    rating: Rating,
) -> MemoryState {
    let last = clamp_state(state);
    let t = fmaxf(elapsed_days, 0.0);
    let r = rating as u8;
    let recall = curve(w, t, &last);
    let s_slow = next_stability(w, last.stability, last.difficulty, recall, r, 7);
    let r_fast = fast_component_recall(w, t, last.stability_fast);
    let mut s_fast = next_stability(w, last.stability_fast, last.difficulty, r_fast, r, 15);
    if r == 1 {
        s_fast = fminf(s_fast, s_slow * 0.8);
    }
    MemoryState {
        stability: s_slow,
        difficulty: next_difficulty(w, last.difficulty, r, recall),
        stability_fast: clampf(s_fast, STABILITY_MIN, STABILITY_MAX),
    }
}

/// Days until recall probability falls to `desired_retention`.
pub fn next_interval_days(w: &Params, state: &MemoryState, desired_retention: f32) -> f32 {
    let desired = clampf(desired_retention, RETENTION_MIN, RETENTION_MAX);
    if desired >= RETENTION_MAX {
        return 0.0;
    }
    let state = clamp_state(state);
    let min_log_t = logf(MIN_INTERVAL_DAYS);
    let max_log_t = logf(STABILITY_MAX);
    let mut log_t = logf(fmaxf(
        fmaxf(state.stability, state.stability_fast),
        MIN_INTERVAL_DAYS,
    ));
    for _ in 0..NEWTON_ITERATIONS {
        log_t = clampf(log_t, min_log_t, max_log_t);
        let t = clampf(expf(log_t), MIN_INTERVAL_DAYS, STABILITY_MAX);
        let (recall, derivative) = curve_and_derivative(w, t, &state);
        let slope = fminf(derivative * t, -1e-12);
        log_t -= clampf((recall - desired) / slope, -4.0, 4.0);
        if !log_t.is_finite() {
            return interval_by_bisection(w, &state, desired);
        }
    }
    let interval = clampf(expf(log_t), 0.0, STABILITY_MAX);
    let recall = curve(w, interval, &state);
    if recall.is_finite() && fabsf(recall - desired) <= 1e-3 {
        interval
    } else {
        interval_by_bisection(w, &state, desired)
    }
}

/// Outcome of each rating for a card; `current` is `None` for a new card.
pub fn next_states(
    w: &Params,
    current: Option<&MemoryState>,
    desired_retention: f32,
    elapsed_days: f32,
) -> NextStates {
    let schedule = |rating| {
        let memory = match current {
            None => initial_state(w, rating),
            Some(state) => next_state(w, state, elapsed_days, rating),
        };
        Scheduled {
            memory,
            interval_days: next_interval_days(w, &memory, desired_retention),
        }
    };
    NextStates {
        again: schedule(Rating::Again),
        hard: schedule(Rating::Hard),
        good: schedule(Rating::Good),
        easy: schedule(Rating::Easy),
    }
}

/// Seconds for an interval in days, as cards are scheduled: at least one
/// minute, saturating.
pub fn interval_seconds(days: f32) -> u32 {
    if days.is_nan() || days <= 0.0 {
        return 60;
    }
    // f64 keeps whole-second precision for long intervals.
    let seconds = libm::round(f64::from(days) * 86_400.0);
    if seconds < 60.0 {
        60
    } else if seconds > f64::from(u32::MAX) {
        u32::MAX
    } else {
        seconds as u32
    }
}
