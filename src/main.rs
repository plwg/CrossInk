use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use fsrs::{FSRS, MemoryState};

use inki::collection::Collection;
use inki::config::{self, Config, check_retention};
use inki::deck_export::{ExportOptions, build_decks, write_deck};

/// PC companion for the CrossInk flashcard app.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Train FSRS-7 parameters on your Anki review history and write the
    /// parameter file for the device's SD card.
    Params {
        /// Anki collection: a .colpkg (File > Export > "Anki Collection
        /// Package"), an .apkg exported with scheduling information, or the
        /// collection.anki2 file from your Anki profile folder.
        collection: PathBuf,
        /// Where to write the parameter file.
        #[arg(short, long, default_value = config::FILE_NAME)]
        output: PathBuf,
        /// Target probability of remembering a card when it comes due.
        #[arg(long, default_value_t = 0.9, value_parser = parse_retention)]
        desired_retention: f32,
    },
    /// Convert Anki decks into .inki deck files for the e-reader. Copy them
    /// into the SD card's flashcards folder.
    Deck {
        /// Anki package: an .apkg (File > Export > "Anki Deck Package"; tick
        /// "Include scheduling information" to keep your progress) or a
        /// .colpkg, or the collection.anki2 file from your Anki profile.
        collection: PathBuf,
        /// Folder to write deck files into, e.g. the SD card's flashcards folder.
        #[arg(short, long, default_value = "flashcards")]
        output: PathBuf,
        /// Export only this deck (and its subdecks), e.g. "Languages::German".
        /// By default every top-level deck becomes its own file.
        #[arg(long)]
        deck: Option<String>,
        /// Start every card as new instead of carrying over Anki progress.
        #[arg(long)]
        no_progress: bool,
        /// Parameter file from `inki params`, used to carry over progress.
        #[arg(short, long)]
        params: Option<PathBuf>,
        /// Replace deck files that already exist, discarding progress made
        /// on the e-reader since they were copied.
        #[arg(long)]
        force: bool,
    },
    /// Open the web app for adding decks, syncing with Anki and training.
    Serve {
        /// Port on localhost.
        #[arg(short, long, default_value_t = 8787)]
        port: u16,
    },
    /// Show how FSRS-7 schedules a card after a sequence of answers.
    Preview {
        /// Answers, oldest first, as RATING@DAYS where RATING is 1-4
        /// (Again/Hard/Good/Easy) and DAYS the time since the previous
        /// answer. The first answer needs no @DAYS. Example: 3,3@0.01,1@4,3@1.
        history: String,
        /// Parameter file to use instead of the FSRS-7 defaults.
        #[arg(short, long)]
        params: Option<PathBuf>,
        /// Override the desired retention.
        #[arg(long, value_parser = parse_retention)]
        desired_retention: Option<f32>,
    },
}

fn parse_retention(text: &str) -> Result<f32> {
    check_retention(text.parse().context("not a number")?)
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Params {
            collection,
            output,
            desired_retention,
        } => train(&collection, &output, desired_retention),
        Command::Deck {
            collection,
            output,
            deck,
            no_progress,
            params,
            force,
        } => export_decks(
            &collection,
            &output,
            deck.as_deref(),
            !no_progress,
            params,
            force,
        ),
        Command::Serve { port } => {
            tokio::runtime::Runtime::new()?.block_on(inki::server::serve(port))
        }
        Command::Preview {
            history,
            params,
            desired_retention,
        } => preview(&history, params, desired_retention),
    }
}

fn train(collection: &std::path::Path, output: &std::path::Path, retention: f32) -> Result<()> {
    eprintln!("Training FSRS-7 on {}...", collection.display());
    let result = inki::train::train(collection)?;
    eprintln!("Used {} reviews of {} cards.", result.reviews, result.cards);
    eprintln!("                 log loss   RMSE (bins)");
    eprintln!(
        "  defaults       {:8.4}   {:8.4}",
        result.defaults.log_loss, result.defaults.rmse_bins
    );
    eprintln!(
        "  your params    {:8.4}   {:8.4}   (lower is better)",
        result.trained.log_loss, result.trained.rmse_bins
    );
    fs::write(output, result.config_text(retention))
        .with_context(|| format!("cannot write {}", output.display()))?;
    eprintln!("Wrote {}", output.display());
    Ok(())
}

fn load_config(params: Option<&PathBuf>) -> Result<Config> {
    match params {
        Some(path) => Config::parse(
            &fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?,
        ),
        None => Ok(Config::default()),
    }
}

fn export_decks(
    collection: &std::path::Path,
    output: &std::path::Path,
    deck: Option<&str>,
    with_progress: bool,
    params: Option<PathBuf>,
    force: bool,
) -> Result<()> {
    let config = load_config(params.as_ref())?;
    let fsrs = FSRS::new(&config.params)?;
    let collection = Collection::open(collection)?;
    let decks = build_decks(
        &collection,
        &ExportOptions {
            deck,
            with_progress,
            fsrs: &fsrs,
            desired_retention: config.desired_retention,
        },
    )?;
    ensure!(!decks.is_empty(), "the collection has no cards to export");

    for deck in &decks {
        let dir = write_deck(output, deck, force)?;
        let studied = deck.studied();
        eprintln!(
            "{}: {} cards ({} studied, {} new) -> {}",
            deck.name,
            deck.cards.len(),
            studied,
            deck.cards.len() - studied,
            dir.display()
        );
        if deck.suspended > 0 {
            eprintln!("  skipped {} suspended cards", deck.suspended);
        }
        if deck.progress_lost > 0 {
            eprintln!(
                "  {} cards were studied in Anki but the export has no review history \
                 for them, so they start as new (export with scheduling information)",
                deck.progress_lost
            );
        }
    }
    if params.is_none() && with_progress {
        eprintln!(
            "Progress was converted with the FSRS-7 defaults; run `inki params` and pass \
             --params for a closer fit."
        );
    }
    Ok(())
}

fn parse_history(text: &str) -> Result<Vec<(u32, f32)>> {
    let mut answers = Vec::new();
    for (i, part) in text.split(',').map(str::trim).enumerate() {
        let (rating, days) = match part.split_once('@') {
            Some((rating, days)) => {
                let days: f32 = days
                    .parse()
                    .with_context(|| format!("bad DAYS in {part:?}"))?;
                ensure!(days.is_finite() && days >= 0.0, "bad DAYS in {part:?}");
                (rating, days)
            }
            None if i == 0 => (part, 0.0),
            None => bail!("answer {part:?} needs @DAYS since the previous answer"),
        };
        let rating: u32 = rating
            .parse()
            .ok()
            .filter(|r| (1..=4).contains(r))
            .with_context(|| format!("rating in {part:?} must be 1-4"))?;
        answers.push((rating, days));
    }
    Ok(answers)
}

fn format_days(days: f32) -> String {
    if days < 1.0 / 24.0 {
        format!("{:.0} min", days * 1440.0)
    } else if days < 1.0 {
        format!("{:.1} h", days * 24.0)
    } else if days < 365.0 {
        format!("{days:.1} d")
    } else {
        format!("{:.1} y", days / 365.0)
    }
}

const RATING_NAMES: [&str; 4] = ["Again", "Hard", "Good", "Easy"];

fn preview(history: &str, params: Option<PathBuf>, retention: Option<f32>) -> Result<()> {
    let mut config = load_config(params.as_ref())?;
    if let Some(retention) = retention {
        config.desired_retention = retention;
    }
    let fsrs = FSRS::new(&config.params)?;
    let dr = config.desired_retention;

    println!(
        "{:>3}  {:<6} {:>9}  {:>6}  {:>9}  {:>9}  {:>5}  {:>9}",
        "#", "answer", "after", "R", "S", "S fast", "D", "next due"
    );
    let mut state: Option<MemoryState> = None;
    for (i, (rating, days)) in parse_history(history)?.into_iter().enumerate() {
        let r = state.map(|s| fsrs.current_retrievability(s, days));
        let next = fsrs.next_states_with_elapsed_days(state, dr, days)?;
        let chosen = [next.again, next.hard, next.good, next.easy][rating as usize - 1].clone();
        println!(
            "{:>3}  {:<6} {:>9}  {:>6}  {:>9}  {:>9}  {:>5.2}  {:>9}",
            i + 1,
            RATING_NAMES[rating as usize - 1],
            if i == 0 {
                "-".into()
            } else {
                format_days(days)
            },
            r.map_or("-".into(), |r| format!("{:.1}%", r * 100.0)),
            format_days(chosen.memory.stability),
            format_days(chosen.memory.stability_fast),
            chosen.memory.difficulty,
            format_days(chosen.interval),
        );
        state = Some(chosen.memory);
    }

    let Some(state) = state else {
        bail!("history is empty")
    };
    let interval = fsrs.next_interval_for_state(state, dr);
    let next = fsrs.next_states_with_elapsed_days(Some(state), dr, interval)?;
    println!(
        "\nReviewed when due ({}, desired retention {:.0}%):",
        format_days(interval),
        dr * 100.0
    );
    for (name, outcome) in RATING_NAMES
        .iter()
        .zip([next.again, next.hard, next.good, next.easy])
    {
        println!(
            "  {name:<6} -> next due in {}",
            format_days(outcome.interval)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_history() {
        assert_eq!(
            parse_history("3, 1@0.5,4@10").unwrap(),
            vec![(3, 0.0), (1, 0.5), (4, 10.0)]
        );
        assert!(parse_history("3,3").is_err());
        assert!(parse_history("5").is_err());
        assert!(parse_history("3,3@-1").is_err());
        assert!(parse_history("3,3@x").is_err());
    }

    #[test]
    fn formats_days() {
        assert_eq!(format_days(10.0 / 1440.0), "10 min");
        assert_eq!(format_days(0.5), "12.0 h");
        assert_eq!(format_days(3.25), "3.2 d");
        assert_eq!(format_days(730.0), "2.0 y");
    }
}
