//! Checks inki-core's FSRS-7 port, which the firmware runs, against fsrs-rs
//! v7, the reference implementation.

use fsrs::{DEFAULT_PARAMETERS, FSRS, FSRSItem, FSRSReview, MemoryState};

/// The scheduler API as the firmware calls it: parameters are clipped on
/// every call, as the reader does when it loads them.
mod device {
    use inki_core::fsrs7::{self, Rating};

    pub type Params = fsrs7::Params;
    pub type State = fsrs7::MemoryState;

    fn clipped(w: &Params) -> Params {
        let mut p = *w;
        fsrs7::clip_params(&mut p);
        p
    }

    pub fn default_params() -> Params {
        fsrs7::DEFAULT_PARAMS
    }

    pub fn clip(w: Params) -> Params {
        clipped(&w)
    }

    pub fn retrievability(w: &Params, state: State, elapsed: f32) -> f32 {
        fsrs7::retrievability(&clipped(w), &state, elapsed)
    }

    pub fn initial_state(w: &Params, rating: u32) -> State {
        fsrs7::initial_state(&clipped(w), Rating::from_u8(rating as u8))
    }

    pub fn next_state(w: &Params, state: State, elapsed: f32, rating: u32) -> State {
        fsrs7::next_state(&clipped(w), &state, elapsed, Rating::from_u8(rating as u8))
    }

    pub fn next_states(
        w: &Params,
        current: Option<State>,
        retention: f32,
        elapsed: f32,
    ) -> ([State; 4], [f32; 4]) {
        let n = fsrs7::next_states(&clipped(w), current.as_ref(), retention, elapsed);
        let all = [n.again, n.hard, n.good, n.easy];
        (all.map(|s| s.memory), all.map(|s| s.interval_days))
    }
}

const CASES: usize = 2_000;
/// fsrs-rs caps stability and intervals at 100 years.
const MAX_INTERVAL_DAYS: f32 = 36_500.0;
const ONE_SECOND: f32 = 1.0 / 86_400.0;

/// Small deterministic PRNG so the tests need no extra dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    fn rating(&mut self) -> u32 {
        // Weighted like real reviews: mostly Good, some Again.
        match self.next_u64() % 10 {
            0 | 1 => 1,
            2 => 2,
            9 => 4,
            _ => 3,
        }
    }

    /// Elapsed days: same-day minutes, a few days, or months.
    fn elapsed(&mut self) -> f32 {
        match self.next_u64() % 3 {
            0 => self.range(1.0 / 1440.0, 0.5),
            1 => self.range(0.5, 10.0),
            _ => self.range(10.0, 400.0),
        }
    }
}

fn to_device(state: MemoryState) -> device::State {
    device::State {
        stability: state.stability,
        difficulty: state.difficulty,
        stability_fast: state.stability_fast,
    }
}

fn assert_close(what: &str, actual: f32, expected: f32, rel_tol: f32) {
    let scale = expected.abs().max(1e-6);
    assert!(
        actual.is_finite() && ((actual - expected).abs() / scale) <= rel_tol,
        "{what}: device {actual} vs fsrs-rs {expected}"
    );
}

fn assert_state_close(what: &str, actual: device::State, expected: MemoryState) {
    assert_close(
        &format!("{what} stability"),
        actual.stability,
        expected.stability,
        1e-4,
    );
    assert_close(
        &format!("{what} difficulty"),
        actual.difficulty,
        expected.difficulty,
        1e-4,
    );
    assert_close(
        &format!("{what} stability_fast"),
        actual.stability_fast,
        expected.stability_fast,
        1e-4,
    );
}

/// Default parameters, plus random ones spread over (and beyond) the valid ranges.
fn parameter_sets(rng: &mut Rng) -> Vec<device::Params> {
    let mut sets = vec![DEFAULT_PARAMETERS];
    for _ in 0..20 {
        let mut w = DEFAULT_PARAMETERS;
        for value in &mut w {
            *value *= rng.range(0.5, 1.6);
        }
        sets.push(w);
    }
    sets
}

#[test]
fn default_parameters_match() {
    assert_eq!(device::default_params(), DEFAULT_PARAMETERS);
}

#[test]
fn review_histories_match() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for (set, w) in parameter_sets(&mut rng).into_iter().enumerate() {
        let reference = FSRS::new(&w).expect("valid parameters");
        for case in 0..CASES / 20 {
            let len = 1 + (rng.next_u64() % 12) as usize;
            let reviews: Vec<FSRSReview> = (0..len)
                .map(|i| FSRSReview {
                    rating: rng.rating(),
                    delta_t: if i == 0 { 0.0 } else { rng.elapsed() },
                })
                .collect();
            let expected = reference
                .historical_memory_states(
                    FSRSItem {
                        reviews: reviews.clone(),
                    },
                    None,
                )
                .expect("finite states");

            let mut state = device::initial_state(&w, reviews[0].rating);
            for (i, review) in reviews.iter().enumerate() {
                if i > 0 {
                    state = device::next_state(&w, state, review.delta_t, review.rating);
                }
                let what = format!("set {set} case {case} review {i}");
                assert_state_close(&what, state, expected[i]);

                let t = rng.elapsed();
                assert_close(
                    &format!("{what} R({t})"),
                    device::retrievability(&w, state, t),
                    reference.current_retrievability(expected[i], t),
                    1e-4,
                );
            }
        }
    }
}

#[test]
fn next_states_match() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for (set, w) in parameter_sets(&mut rng).into_iter().enumerate() {
        let reference = FSRS::new(&w).expect("valid parameters");
        for case in 0..CASES / 20 {
            let retention = rng.range(0.7, 0.98);
            let elapsed = rng.elapsed();
            let current = if case % 5 == 0 {
                None
            } else {
                let first = reference
                    .next_states(None, 0.9, 0)
                    .expect("new card states");
                let mut state = [first.again, first.hard, first.good, first.easy]
                    [rng.rating() as usize - 1]
                    .memory;
                for _ in 0..(rng.next_u64() % 6) {
                    let next = reference
                        .next_states_with_elapsed_days(Some(state), 0.9, rng.elapsed())
                        .expect("finite");
                    state = [next.again, next.hard, next.good, next.easy]
                        [rng.rating() as usize - 1]
                        .memory;
                }
                Some(state)
            };

            let expected = reference
                .next_states_with_elapsed_days(current, retention, elapsed)
                .expect("finite");
            let (states, intervals) =
                device::next_states(&w, current.map(to_device), retention, elapsed);
            let expected = [expected.again, expected.hard, expected.good, expected.easy];
            for (i, name) in ["again", "hard", "good", "easy"].iter().enumerate() {
                let what = format!("set {set} case {case} {name}");
                assert_state_close(&what, states[i], expected[i].memory);
                assert_close(
                    &format!("{what} interval"),
                    intervals[i],
                    expected[i].interval,
                    1e-3,
                );
                // The interval must land on the desired retention, unless it
                // hit the 100-year cap, or recall drops below the target
                // within a second (possible with extreme parameters).
                let r = device::retrievability(&w, states[i], intervals[i]);
                let unreachable = intervals[i] >= MAX_INTERVAL_DAYS || intervals[i] < ONE_SECOND;
                assert!(
                    unreachable || (r - retention).abs() < 2e-3,
                    "{what}: R at interval {} is {r}, wanted {retention}",
                    intervals[i]
                );
            }
        }
    }
}

#[test]
fn clipping_matches() {
    // fsrs-rs clips inside FSRS::new; the device shim clips the same raw
    // input. Matching outputs for wild parameters shows the clipping agrees.
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for _ in 0..200 {
        let mut w = DEFAULT_PARAMETERS;
        for value in &mut w {
            *value = rng.range(-2.0, 12.0);
        }
        let clipped = device::clip(w);
        assert_eq!(device::clip(clipped), clipped, "clipping is idempotent");
        let reference = FSRS::new(&w).expect("finite parameters");
        let expected = reference.next_states(None, 0.9, 0).expect("finite");
        let (states, _) = device::next_states(&w, None, 0.9, 0.0);
        let good = expected.good.memory;
        for (state, expected) in
            states
                .iter()
                .zip([expected.again, expected.hard, expected.good, expected.easy])
        {
            assert_state_close("clipped", *state, expected.memory);
        }
        let t = rng.range(0.01, 50.0);
        assert_close(
            "clipped R",
            device::retrievability(&w, to_device(good), t),
            reference.current_retrievability(good, t),
            1e-4,
        );
    }
}

#[test]
fn non_finite_parameters_are_replaced() {
    let mut w = DEFAULT_PARAMETERS;
    w[5] = f32::NAN;
    w[24] = f32::INFINITY;
    let clipped = device::clip(w);
    assert!(clipped.iter().all(|v| v.is_finite()));
    assert_eq!(clipped[5], 0.001);
    assert_eq!(clipped[24], 0.01);
}
