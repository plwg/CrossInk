//! PC companion for an Anki-style flashcard app on CrossInk e-readers.
//!
//! Turns Anki decks into `.inki` files, updates them with new Anki cards, and
//! trains FSRS-7 parameters. The scheduler and the deck format live in the
//! `inki-core` crate, which the reader's firmware uses too.

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
