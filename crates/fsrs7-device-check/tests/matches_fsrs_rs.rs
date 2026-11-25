//! Checks the firmware FSRS-7 port against fsrs-rs v7, the reference implementation.

use fsrs::{DEFAULT_PARAMETERS, FSRS, FSRSItem, FSRSReview, MemoryState};
use fsrs7_device_check as device;

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

fn params_line(w: &[f32]) -> String {
    let values: Vec<String> = w.iter().map(|v| v.to_string()).collect();
    format!("fsrs7 {}", values.join(","))
}

#[test]
fn parses_config_file() {
    let mut w = DEFAULT_PARAMETERS;
    w[0] = 0.25;
    let text = format!(
        "# inki FSRS-7 parameters\r\n{}\r\ndesired_retention 0.85\nunknown_key 1\n",
        params_line(&w)
    );
    let (params, retention) = device::parse_config(&text).expect("valid file");
    assert_eq!(params, w);
    assert_eq!(retention, 0.85);
}

#[test]
fn config_defaults_and_errors() {
    let (params, retention) = device::parse_config("# empty\n").expect("valid");
    assert_eq!(params, DEFAULT_PARAMETERS);
    assert_eq!(retention, 0.9);

    let short = params_line(&DEFAULT_PARAMETERS[..33]);
    assert!(device::parse_config(&short).is_none(), "33 values");
    let long = format!("{},1.0", params_line(&DEFAULT_PARAMETERS));
    assert!(device::parse_config(&long).is_none(), "35 values");
    assert!(device::parse_config("desired_retention abc").is_none());
    assert!(device::parse_config("desired_retention 1.5").is_none());
    assert!(device::parse_config("fsrs7 nan").is_none());
}

#[test]
fn out_of_range_values_in_file_are_clipped() {
    let mut w = DEFAULT_PARAMETERS;
    w[24] = 3.0;
    let (params, _) = device::parse_config(&params_line(&w)).expect("valid");
    assert_eq!(params[24], 0.95);
}

#[test]
fn reads_file_written_by_inki() {
    let mut rng = Rng(0x0BAD_5EED_1234_5678);
    for _ in 0..50 {
        let mut params = DEFAULT_PARAMETERS.to_vec();
        for value in &mut params {
            *value *= rng.range(0.5, 1.5);
        }
        let config = inki::config::Config {
            params: params.clone(),
            desired_retention: rng.range(0.7, 0.99),
        };
        let text = config.to_text(&["inki FSRS-7 parameters".into()]);
        let (device_params, retention) = device::parse_config(&text).expect("device parses it");
        let mut raw = [0.0; device::PARAM_COUNT];
        raw.copy_from_slice(&params);
        assert_eq!(
            device_params,
            device::clip(raw),
            "exact round trip, then clipped"
        );
        assert_eq!(retention, config.desired_retention);
    }
}
