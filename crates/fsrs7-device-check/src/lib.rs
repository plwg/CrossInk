//! Safe Rust wrappers around the firmware FSRS-7 C++ library
//! (`firmware/lib/Fsrs7`), compiled for the host so tests can compare it with
//! the fsrs-rs reference implementation.

use std::os::raw::{c_char, c_int, c_uint};

pub const PARAM_COUNT: usize = 34;

/// Memory state, laid out like the C shim's `Fsrs7State`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub stability: f32,
    pub difficulty: f32,
    pub stability_fast: f32,
}

unsafe extern "C" {
    fn fsrs7_default_params(out: *mut f32);
    fn fsrs7_clip(w: *mut f32);
    fn fsrs7_retrievability(w: *const f32, state: State, elapsed: f32) -> f32;
    fn fsrs7_initial_state(w: *const f32, rating: c_uint) -> State;
    fn fsrs7_next_state(w: *const f32, state: State, elapsed: f32, rating: c_uint) -> State;
    fn fsrs7_next_interval(w: *const f32, state: State, desired_retention: f32) -> f32;
    fn fsrs7_next_states(
        w: *const f32,
        current: *const State,
        desired_retention: f32,
        elapsed: f32,
        states_out: *mut State,
        intervals_out: *mut f32,
    );
    fn fsrs7_parse_config(
        text: *const c_char,
        len: usize,
        params_out: *mut f32,
        retention_out: *mut f32,
    ) -> c_int;
}

pub type Params = [f32; PARAM_COUNT];

pub fn default_params() -> Params {
    let mut out = [0.0; PARAM_COUNT];
    // SAFETY: `out` holds PARAM_COUNT floats, as the C function writes.
    unsafe { fsrs7_default_params(out.as_mut_ptr()) };
    out
}

pub fn clip(mut w: Params) -> Params {
    // SAFETY: `w` holds PARAM_COUNT floats.
    unsafe { fsrs7_clip(w.as_mut_ptr()) };
    w
}

pub fn retrievability(w: &Params, state: State, elapsed_days: f32) -> f32 {
    // SAFETY: `w` holds PARAM_COUNT floats; the call does not retain the pointer.
    unsafe { fsrs7_retrievability(w.as_ptr(), state, elapsed_days) }
}

pub fn initial_state(w: &Params, rating: u32) -> State {
    // SAFETY: as above.
    unsafe { fsrs7_initial_state(w.as_ptr(), rating) }
}

pub fn next_state(w: &Params, state: State, elapsed_days: f32, rating: u32) -> State {
    // SAFETY: as above.
    unsafe { fsrs7_next_state(w.as_ptr(), state, elapsed_days, rating) }
}

pub fn next_interval(w: &Params, state: State, desired_retention: f32) -> f32 {
    // SAFETY: as above.
    unsafe { fsrs7_next_interval(w.as_ptr(), state, desired_retention) }
}

/// States and intervals for ratings Again, Hard, Good, Easy.
pub fn next_states(
    w: &Params,
    current: Option<State>,
    desired_retention: f32,
    elapsed_days: f32,
) -> ([State; 4], [f32; 4]) {
    let zero = State {
        stability: 0.0,
        difficulty: 0.0,
        stability_fast: 0.0,
    };
    let mut states = [zero; 4];
    let mut intervals = [0.0; 4];
    let current_ptr = current
        .as_ref()
        .map_or(std::ptr::null(), |state| state as *const State);
    // SAFETY: output arrays hold 4 entries each; `current_ptr` is null or
    // points at a live State for the duration of the call.
    unsafe {
        fsrs7_next_states(
            w.as_ptr(),
            current_ptr,
            desired_retention,
            elapsed_days,
            states.as_mut_ptr(),
            intervals.as_mut_ptr(),
        );
    }
    (states, intervals)
}

/// Parses a parameter file with the firmware parser.
/// Returns (clipped params, desired retention), or None on a parse error.
pub fn parse_config(text: &str) -> Option<(Params, f32)> {
    let mut params = [0.0; PARAM_COUNT];
    let mut retention = 0.0;
    // SAFETY: the text pointer/length pair describes a live byte slice;
    // outputs are sized as the C function expects.
    let ok = unsafe {
        fsrs7_parse_config(
            text.as_ptr().cast(),
            text.len(),
            params.as_mut_ptr(),
            &mut retention,
        )
    };
    (ok == 1).then_some((params, retention))
}
