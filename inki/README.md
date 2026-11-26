# inki

Anki-style flashcards for [CrossInk](https://github.com/uxjulia/CrossInk) e-readers,
scheduled with [FSRS-7](https://github.com/open-spaced-repetition/fsrs-rs).

The FSRS-7 maths runs on the device. Training your personal parameters is too
heavy for the ESP32-C3, so this PC tool does it from your Anki review history.

The scheduler and the `.inki` deck format are written once, in Rust
(`crates/inki-core`), and used by both this tool and the reader's firmware,
so the PC and the reader read and write decks and schedule cards with the
same code.

## Layout

| Path | What |
|---|---|
| `src/` | `inki` command-line tool (Rust) |
| `crates/inki-core/` | Shared with the firmware: FSRS-7 and the `.inki` format (`no_std`, no allocation) |
| `crates/inki-ffi/` | C interface to inki-core, built as a static library for the firmware (`include/inki_core.h`) |
| `../crossink-flashcards/` | [CrossInk firmware fork](https://github.com/plwg/CrossInk/tree/feat/flashcards) with the flashcard screens (separate repo, alongside this one) |
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
2. **Study settings:** new cards and reviews per day, desired retention,
   and optionally FSRS-7 parameters trained on your review history.
3. **Put a deck on the reader:** download a deck as one file, e.g.
   `German.inki`, and copy it into the SD card's `flashcards` folder.
   Subdecks are included. The file carries the settings from step 2.
4. **Update a deck on the reader:** choose the deck's `.inki` file from the
   SD card to change its settings, or, with a fresh export in step 1, to add
   the cards you added in Anki (edited text is updated too). Copy the
   downloaded file back over the old one. Progress made on the reader is
   kept.

Cards are rendered from your Anki templates as plain text; images show as
`[image]`, sound is dropped, suspended cards are left out. Progress made in
Anki carries over to new decks through FSRS-7.

## Command line

The same without the browser:

```sh
cargo run --release -- deck German.apkg -o /path/to/sdcard/flashcards \
    --new-per-day 10 --reviews-per-day 100 --desired-retention 0.9 --train
cargo run --release -- settings /path/to/sdcard/flashcards/German.inki --new-per-day 15
```

`deck` writes one `.inki` file per top-level deck (`--deck
"Languages::German"` for just one) and never overwrites an existing file
without `--force`, since it holds progress made on the reader. `--train`
fits FSRS-7 to the export's review history and stores the parameters in the
decks; without it they get the FSRS-7 defaults.

`settings` shows a deck file's settings, or changes them in place with the
same options (`--train COLLECTION`, `--default-params`), keeping progress.

## Study settings

Each deck file holds its own settings, so the reader needs no other file:

| Setting | Default |
|---|---|
| New cards per day | 20 |
| Reviews per day (cards you forget again are not limited) | 200 |
| Desired retention | 90% |
| FSRS-7 parameters | the FSRS-7 defaults, or trained on your reviews |

A study day starts at 4 am local time, as in Anki. Decks made by earlier
versions of inki (format version 1) have no settings and use the defaults
until you change their settings.

On the reader, decks are under **Home > Flashcards**. The deck file format is
documented in the firmware fork's `docs/flashcards.md`.

## Firmware

The CrossInk fork with the flashcard screens is its own repository,
[plwg/CrossInk](https://github.com/plwg/CrossInk) (branch `feat/flashcards`),
cloned alongside this one in `../crossink-flashcards`. Build it for the X3/X4
with:

```sh
git clone -b feat/flashcards --recurse-submodules https://github.com/plwg/CrossInk ../crossink-flashcards
cd ../crossink-flashcards && pio run -e default
```

The firmware is written to `.pio/build/default/firmware-x3-x4.bin` in that
repository. Use pioarduino's PlatformIO Core 6.1.19, as CrossInk's CI does; PlatformIO
6.2 cannot build it.

## Preview scheduling

Preview how a card would be scheduled:

```sh
cargo run --release -- preview "3,3@0.007,3@1,1@4" --deck German.inki
```

Each answer is `RATING@DAYS`: rating 1-4 (Again/Hard/Good/Easy), days since the
previous answer (fractions allowed, so `0.007` is 10 minutes). `--deck`
uses a deck file's parameters and retention instead of the defaults.

## Shared code in the firmware

The firmware fork's build (`scripts/build_inki_core.py`) compiles
`crates/inki-ffi` for the board's CPU and links it. It fetches the inki tag
pinned in the fork's `inki.version` file (`INKI_DIR=../inki` builds against
a local checkout instead), and needs Rust with:

```sh
rustup target add riscv32imc-unknown-none-elf   # X3 / X4 (ESP32-C3)
```

Its C++ code calls inki-core through thin wrappers in `lib/Fsrs7/Fsrs7.h` and
`lib/Flashcards/FlashcardFormat.cpp`. Rust's maths (`libm`) keeps the
results bit-identical on the PC and the reader; the firmware grows by about
2 KB.

## Versions

inki releases are git tags (`v0.2.0`). The firmware fork pins the tag it is
built with in `inki.version`, and its build fails if the deck format
versions differ, so a firmware release and the inki tag it names always
match. Each firmware release says which inki to use.

| inki | Deck format | Firmware |
|---|---|---|
| v0.2.0 | 2: settings in each deck | v2.0.0-flashcards.2 and later |
| (untagged) | 1: separate `fsrs7-params.txt` | v2.0.0-flashcards.1 |

The reader opens decks of its own format version and older; a deck from a
newer inki shows "This deck needs newer firmware".

## Checks

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

inki-core's tests compare its FSRS-7 with fsrs-rs 7.0.0 over thousands of
random review histories and parameter sets: memory states and recall within
0.01%, intervals within 0.1%. To check the firmware build of the shared code:

```sh
cargo build --release --manifest-path crates/inki-ffi/Cargo.toml --target riscv32imc-unknown-none-elf
```

## Credits

FSRS-7 by [Open Spaced Repetition](https://github.com/open-spaced-repetition),
ported from fsrs-rs (BSD-3-Clause; see `crates/inki-core/LICENSE-fsrs-rs`).

## License

MIT; see `LICENSE`. The FSRS-7 port in `crates/inki-core/src/fsrs7.rs` is
derived from fsrs-rs and also stays under its BSD-3-Clause licence
(`crates/inki-core/LICENSE-fsrs-rs`).
