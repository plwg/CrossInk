//! PC companion for an Anki-style flashcard app on CrossInk e-readers.
//!
//! Trains FSRS-7 parameters from an Anki review history and writes them in the
//! format the device's scheduler (`firmware/lib/Fsrs7`) reads.

pub mod anki_model;
pub mod collection;
pub mod config;
pub mod deck_export;
pub mod deck_file;
pub mod revlog;
pub mod server;
pub mod template;
pub mod train;
pub mod update;
