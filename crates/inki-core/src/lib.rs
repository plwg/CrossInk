//! Shared by the inki PC tool and the CrossInk e-reader firmware:
//!
//! - [`fsrs7`]: FSRS-7 scheduling (a port of fsrs-rs, small enough for the ESP32-C3)
//! - [`format`]: the `.inki` deck file
//! - [`params`]: the `fsrs7-params.txt` parameter file
//!
//! `no_std` and allocation-free. The firmware links it through the
//! `inki-ffi` crate's C interface.

#![no_std]

pub mod format;
pub mod fsrs7;
pub mod params;
