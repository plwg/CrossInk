---
title: Flashcards
nav_order: 8
---

# Flashcards

Study Anki decks on the reader. Cards are scheduled on the device with
[FSRS-7](https://github.com/open-spaced-repetition/fsrs-rs), the same
algorithm family Anki uses, and every answer is saved immediately.

Flashcards need the real-time clock, so they are meant for the **X3**. On the
X4 the clock drifts during sleep; use **Sync Clock** in Settings before studying.

## Getting decks onto the reader

Decks are made on a computer with `inki`. Run `inki serve` and open
http://localhost:8787:

1. In Anki, choose **File > Export**, pick **Anki Deck Package (.apkg)**, and
   tick **Include scheduling information** to keep your progress. Choose that
   file in step 1 of the page.
2. Download a deck. You get one file, such as `German.inki`. Copy it into the
   `flashcards` folder on the SD card (create the folder the first time).
   Subdecks are included.
3. Optional: train FSRS-7 on your reviews and copy the downloaded
   `fsrs7-params.txt` into the same folder.

**Flashcards** appears on the Home screen once the `flashcards` folder exists.

To add cards you created in Anki later, export again, choose the deck's
`.inki` file from the SD card in step 3 of the page, and copy the downloaded
file back over it. Progress made on the reader is kept. Avoid studying on the
reader between taking the file off the card and putting the new one back.

The command line does the same: `inki deck German.apkg -o /path/to/sdcard/flashcards`.

## Studying

The deck list shows how many cards are due and new today. Open a deck to study.

| Screen | Back | Confirm | Left | Right |
| --- | --- | --- | --- | --- |
| Question | Exit | Show answer | Show answer | Show answer |
| Answer | Again | Hard | Good | Easy |

Each answer button shows when the card will come back. To leave while the
answer is showing, hold **Back** for one second.

Limits follow Anki's defaults: 20 new cards and 200 reviews a day, with the day
starting at 4 am local time (set **Clock UTC Offset** in Settings). Cards you get wrong come
back later in the same session.

Only text is shown. Formatting, images (shown as `[image]`), sound and
type-in answers are not available on e-ink.

## Building

The scheduler (FSRS-7), the deck file format and the parameter-file parser
are shared with the `inki` PC tool as the Rust crate `inki-core`. The build
compiles it with `scripts/build_inki_core.py` and links it, so building the
firmware needs:

- the `inki` repository next to this one (`../inki`), or its location in `INKI_DIR`
- Rust, with the target for the board: `rustup target add riscv32imc-unknown-none-elf`
  for the X3 and X4. The ESP32-S3 boards need Espressif's Xtensa Rust toolchain
  (`espup`); that path is untested.

The firmware's C++ reaches it through `lib/Fsrs7/Fsrs7.h` and
`lib/Flashcards/FlashcardFormat.cpp`; the C interface is
`inki/crates/inki-ffi/include/inki_core.h`. FSRS-7 is ported from fsrs-rs
v7.0.0 (BSD-3-Clause, Open Spaced Repetition).

## Files

```text
/flashcards/fsrs7-params.txt   FSRS-7 parameters (optional)
/flashcards/<deck>.inki        one file per deck: cards, progress and answers
```

All integers are little-endian. A deck file is laid out as:

| Part | Where | Contents |
| --- | --- | --- |
| Header | 0, 32 bytes | see below |
| Deck name | 32 | the Anki deck name, UTF-8 (not used by the reader) |
| Card index | `indexOffset` | 16 bytes per card |
| Card text | after the index | question, NUL, answer per card |
| State | `stateOffset` | 20 bytes per card, rewritten in place after each answer |
| Answer log | `logOffset` to the end | 32 bytes per answer, appended |

The reader only ever changes a card's state record and appends to the answer
log, so the file never needs to be rewritten on the card. A half-written last
log entry (power lost mid-answer) is ignored and overwritten by the next one.

### Header

| Offset | Type | Field |
| --- | --- | --- |
| 0 | char[4] | magic `INKD` |
| 4 | u16 | version (1) |
| 6 | u16 | flags (0) |
| 8 | u32 | card count |
| 12 | u32 | `indexOffset` |
| 16 | u32 | `stateOffset` |
| 20 | u32 | `logOffset` (= `stateOffset` + 20 × card count) |
| 24 | u16 | deck name length |
| 26 | u16 | reserved (0) |
| 28 | u32 | reserved (0) |

### Card index entry

| Offset | Type | Field |
| --- | --- | --- |
| 0 | u64 | Anki card id |
| 8 | u32 | text offset from the start of the file |
| 12 | u32 | text length in bytes (at most 4096) |

Card text is UTF-8: the question, a NUL byte, the answer. Lines are separated
by `\n`. New cards are introduced in file order. When `inki` updates a deck,
existing cards keep their position and new cards are appended.

### State record

| Offset | Type | Field |
| --- | --- | --- |
| 0 | f32 | long-term stability (days) |
| 4 | f32 | difficulty (1-10) |
| 8 | f32 | short-term stability (days) |
| 12 | u32 | last review, Unix seconds (0 = new card) |
| 16 | u32 | due time, Unix seconds |

An all-zero record is a new card.

### Answer log entry

| Offset | Type | Field |
| --- | --- | --- |
| 0 | u64 | Anki card id |
| 8 | u32 | card index |
| 12 | u32 | answer time, Unix seconds |
| 16 | u8 | rating: 1 Again, 2 Hard, 3 Good, 4 Easy |
| 17 | u8 | flags: bit 0 = the card was new |
| 18 | u16 | seconds spent on the card |
| 20 | f32 | days since the previous answer |
| 24 | f32 | stability after the answer |
| 28 | f32 | difficulty after the answer |

The reader counts today's answers from the end of the log to apply the daily
limits.
