//! C interface to inki-core for the CrossInk firmware; see
//! `include/inki_core.h`. The structs there match inki-core's `#[repr(C)]`
//! types field for field, so values cross the boundary as-is.

#![no_std]

use core::slice;

use inki_core::format::{
    self, CardIndexEntry, DeckHeader, DeckSettings, HEADER_SIZE, INDEX_ENTRY_SIZE,
    REVIEW_LOG_ENTRY_SIZE, ReviewLogEntry, SETTINGS_SIZE, STATE_RECORD_SIZE, StateRecord,
};
use inki_core::fsrs7::{self, MemoryState, NextStates, Params, Rating};

// Layouts the C header relies on.
const _: () = {
    assert!(size_of::<MemoryState>() == 12);
    assert!(size_of::<fsrs7::Scheduled>() == 16);
    assert!(size_of::<NextStates>() == 64);
    assert!(size_of::<DeckHeader>() == 20);
    assert!(size_of::<DeckSettings>() == 148);
    assert!(size_of::<StateRecord>() == 20);
    assert!(size_of::<CardIndexEntry>() == 16);
    assert!(size_of::<ReviewLogEntry>() == 32);
};

unsafe extern "C" {
    fn abort() -> !;
}

/// The library never panics on valid input; if it does, stop like a failed
/// assertion in C instead of continuing in an unknown state.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: abort() from the C library takes no arguments and never returns.
    unsafe { abort() }
}

/// # Safety
/// `params` points at INKI_PARAM_COUNT floats.
unsafe fn params_ref<'a>(params: *const f32) -> &'a Params {
    // SAFETY: guaranteed by the caller; [f32; 34] has f32 alignment.
    unsafe { &*params.cast::<Params>() }
}

/// # Safety
/// `ptr` is null or points at `N` readable bytes.
unsafe fn bytes_in<'a, const N: usize>(ptr: *const u8) -> Option<&'a [u8; N]> {
    // SAFETY: guaranteed by the caller; byte arrays have alignment 1.
    unsafe { ptr.cast::<[u8; N]>().as_ref() }
}

/// # Safety
/// `ptr` is null or points at `N` writable bytes.
unsafe fn write_bytes<const N: usize>(ptr: *mut u8, bytes: [u8; N]) {
    // SAFETY: guaranteed by the caller.
    if let Some(out) = unsafe { ptr.cast::<[u8; N]>().as_mut() } {
        *out = bytes;
    }
}

/// # Safety
/// `out` is null or valid for writes.
unsafe fn write<T>(out: *mut T, value: T) {
    // SAFETY: guaranteed by the caller.
    if let Some(out) = unsafe { out.as_mut() } {
        *out = value;
    }
}

// ---- FSRS-7 ----

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_clip_params(params: *mut f32) {
    // SAFETY: see the header.
    if let Some(p) = unsafe { params.cast::<Params>().as_mut() } {
        fsrs7::clip_params(p);
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_retrievability(
    params: *const f32,
    state: *const MemoryState,
    elapsed_days: f32,
) -> f32 {
    // SAFETY: see the header.
    unsafe { fsrs7::retrievability(params_ref(params), &*state, elapsed_days) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_initial_state(params: *const f32, rating: u8, out: *mut MemoryState) {
    // SAFETY: see the header.
    unsafe {
        write(
            out,
            fsrs7::initial_state(params_ref(params), Rating::from_u8(rating)),
        );
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_next_state(
    params: *const f32,
    state: *const MemoryState,
    elapsed_days: f32,
    rating: u8,
    out: *mut MemoryState,
) {
    // SAFETY: see the header.
    unsafe {
        let next = fsrs7::next_state(
            params_ref(params),
            &*state,
            elapsed_days,
            Rating::from_u8(rating),
        );
        write(out, next);
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_next_interval_days(
    params: *const f32,
    state: *const MemoryState,
    desired_retention: f32,
) -> f32 {
    // SAFETY: see the header.
    unsafe { fsrs7::next_interval_days(params_ref(params), &*state, desired_retention) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_next_states(
    params: *const f32,
    current: *const MemoryState,
    desired_retention: f32,
    elapsed_days: f32,
    out: *mut NextStates,
) {
    // SAFETY: see the header; `current` may be null.
    unsafe {
        let next = fsrs7::next_states(
            params_ref(params),
            current.as_ref(),
            desired_retention,
            elapsed_days,
        );
        write(out, next);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn inki_interval_seconds(days: f32) -> u32 {
    fsrs7::interval_seconds(days)
}

// ---- .inki deck file ----

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_encode_deck_header(header: *const DeckHeader, out: *mut u8) {
    // SAFETY: see the header.
    unsafe { write_bytes::<HEADER_SIZE>(out, (*header).encode()) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_decode_deck_header(
    input: *const u8,
    file_size: u64,
    out: *mut DeckHeader,
) -> bool {
    // SAFETY: see the header.
    let Some(bytes) = (unsafe { bytes_in::<HEADER_SIZE>(input) }) else {
        return false;
    };
    match DeckHeader::decode(bytes, file_size) {
        Some(header) => {
            // SAFETY: see the header.
            unsafe { write(out, header) };
            true
        }
        None => false,
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_deck_settings_offset(header: *const DeckHeader) -> u32 {
    // SAFETY: see the header.
    unsafe { (*header).settings_offset().unwrap_or(0) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_default_deck_settings(out: *mut DeckSettings) {
    // SAFETY: see the header.
    unsafe { write(out, DeckSettings::default()) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_encode_deck_settings(settings: *const DeckSettings, out: *mut u8) {
    // SAFETY: see the header.
    unsafe { write_bytes::<SETTINGS_SIZE>(out, (*settings).encode()) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_decode_deck_settings(
    input: *const u8,
    length: usize,
    out: *mut DeckSettings,
) {
    let settings = if input.is_null() {
        DeckSettings::default()
    } else {
        // SAFETY: `input` points at `length` bytes.
        DeckSettings::decode(unsafe { slice::from_raw_parts(input, length) })
    };
    // SAFETY: see the header.
    unsafe { write(out, settings) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_deck_settings_size(header: *const DeckHeader) -> u32 {
    // SAFETY: see the header.
    unsafe { (*header).settings_size() as u32 }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_review_log_entries(header: *const DeckHeader, file_size: u64) -> u64 {
    // SAFETY: see the header.
    unsafe { (*header).review_log_entries(file_size) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_review_log_append_offset(
    header: *const DeckHeader,
    file_size: u64,
) -> u64 {
    // SAFETY: see the header.
    unsafe { (*header).review_log_append_offset(file_size) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_encode_card_index_entry(entry: *const CardIndexEntry, out: *mut u8) {
    // SAFETY: see the header.
    unsafe { write_bytes::<INDEX_ENTRY_SIZE>(out, (*entry).encode()) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_decode_card_index_entry(input: *const u8, out: *mut CardIndexEntry) {
    // SAFETY: see the header.
    unsafe {
        if let Some(bytes) = bytes_in::<INDEX_ENTRY_SIZE>(input) {
            write(out, CardIndexEntry::decode(bytes));
        }
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_encode_state_record(record: *const StateRecord, out: *mut u8) {
    // SAFETY: see the header.
    unsafe { write_bytes::<STATE_RECORD_SIZE>(out, (*record).encode()) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_decode_state_record(input: *const u8, out: *mut StateRecord) {
    // SAFETY: see the header.
    unsafe {
        if let Some(bytes) = bytes_in::<STATE_RECORD_SIZE>(input) {
            write(out, StateRecord::decode(bytes));
        }
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_encode_review_log_entry(entry: *const ReviewLogEntry, out: *mut u8) {
    // SAFETY: see the header.
    unsafe { write_bytes::<REVIEW_LOG_ENTRY_SIZE>(out, (*entry).encode()) }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_decode_review_log_entry(input: *const u8, out: *mut ReviewLogEntry) {
    // SAFETY: see the header.
    unsafe {
        if let Some(bytes) = bytes_in::<REVIEW_LOG_ENTRY_SIZE>(input) {
            write(out, ReviewLogEntry::decode(bytes));
        }
    }
}

/// # Safety
/// See `inki_core.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inki_split_card_text(
    text: *const u8,
    length: usize,
    front_length: *mut usize,
    back_offset: *mut usize,
    back_length: *mut usize,
) {
    if text.is_null() {
        return;
    }
    // SAFETY: `text` points at `length` bytes.
    let text = unsafe { slice::from_raw_parts(text, length) };
    let (front, back) = format::split_card_text(text);
    let offset = if back.is_empty() && front.len() == length {
        length
    } else {
        front.len() + 1
    };
    // SAFETY: see the header.
    unsafe {
        write(front_length, front.len());
        write(back_offset, offset);
        write(back_length, back.len());
    }
}
