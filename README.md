# inki

Anki-style flashcards for [CrossInk](https://github.com/uxjulia/CrossInk) e-readers,
scheduled with [FSRS-7](https://github.com/open-spaced-repetition/fsrs-rs).

The FSRS-7 maths runs on the device. Training your personal parameters is too
heavy for the ESP32-C3, so this PC tool does it from your Anki review history.

## Layout

| Path | What |
|---|---|
| `src/` | `inki` command-line tool (Rust) |
| `firmware/lib/Fsrs7/` | FSRS-7 scheduler for the device (C++17, no allocation, floats only) |
| `crossink/` | CrossInk firmware fork with the flashcard screens (separate repo) |
| `crates/fsrs7-device-check/` | Builds the C++ library on the PC and tests it against fsrs-rs |
| `WISHLIST.md` | RWKV-Instant / RWKV-Curve plans |

## Web app

```sh
cargo run --release -- serve
```

Open http://localhost:8787. You upload files and download files; the app
never reads or writes anything on your computer by itself, and never
changes your Anki collection. It runs only on your machine (localhost) and
refuses requests from other sites.

1. **Your Anki export:** in Anki, **File > Export > Anki Deck Package
   (.apkg)** with **Include scheduling information** ticked (a `.colpkg`
   works too). Choose it on the page.
2. **Put a deck on the reader:** download a deck as one file, e.g.
   `German.inki`, and copy it into the SD card's `flashcards` folder.
   Subdecks are included.
3. **Add new cards to a deck on the reader:** after adding cards in Anki,
   export again, choose the deck's `.inki` file from the SD card, and copy the
   downloaded file back over it. Progress made on the reader is kept; new
   cards are appended and edited text is updated.
4. **Scheduling (optional):** train FSRS-7 on your review history, set the
   desired retention, and copy the downloaded `fsrs7-params.txt` into the
   `flashcards` folder.

Cards are rendered from your Anki templates as plain text; images show as
`[image]`, sound is dropped, suspended cards are left out. Progress made in
Anki carries over to new decks through FSRS-7.

## Command line

The same without the browser:

```sh
cargo run --release -- deck German.apkg -o /path/to/sdcard/flashcards
cargo run --release -- params collection.colpkg -o fsrs7-params.txt
```

`deck` writes one `.inki` file per top-level deck (`--deck
"Languages::German"` for just one) and never overwrites an existing file
without `--force`, since it holds progress made on the reader.

On the reader, decks are under **Home > Flashcards**. The deck file format is
documented in the firmware fork's `docs/flashcards.md`.

## Firmware

The CrossInk fork with the flashcard screens is in `crossink/` (branch
`feat/flashcards`, its own git repository, ignored here). Build it for the
X3/X4 with:

```sh
cd crossink && pio run -e default
```

The firmware is written to `crossink/.pio/build/default/firmware-x3-x4.bin`.
Use pioarduino's PlatformIO Core 6.1.19, as CrossInk's CI does; PlatformIO
6.2 cannot build it.

## Preview scheduling

Preview how a card would be scheduled:

```sh
cargo run --release -- preview "3,3@0.007,3@1,1@4" -p fsrs7-params.txt
```

Each answer is `RATING@DAYS`: rating 1-4 (Again/Hard/Good/Easy), days since the
previous answer (fractions allowed, so `0.007` is 10 minutes).

## Parameter file

```text
# comment
fsrs7 <34 comma-separated numbers>
desired_retention 0.9
```

## Device library

Copy `firmware/lib/Fsrs7` into the CrossInk fork's `lib/` folder; PlatformIO
picks it up automatically. Per card the app stores a `fsrs7::MemoryState`
(12 bytes) and the time of the last review.

```cpp
#include <Fsrs7.h>

fsrs7::Config config = fsrs7::defaultConfig();
fsrs7::parseConfig(fileText, fileLength, config);  // keeps defaults on error

// On an answer. `card.isNew` cards have no state yet.
const fsrs7::NextStates next = fsrs7::nextStates(
    config.params, card.isNew ? nullptr : &card.memory,
    config.desiredRetention, elapsedDays);
const fsrs7::ScheduledState& chosen = next.forRating(fsrs7::Rating::Good);
card.memory = chosen.memory;
card.due = now + chosen.intervalDays;
```

`nextStates` also gives the interval for each button, to show on screen before
the user answers.

## Checks

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The device tests compare the C++ code with fsrs-rs 7.0.0 over thousands of
random review histories and parameter sets: memory states and recall within
0.01%, intervals within 0.1%.

## Credits

FSRS-7 by [Open Spaced Repetition](https://github.com/open-spaced-repetition),
ported from fsrs-rs (BSD-3-Clause).
