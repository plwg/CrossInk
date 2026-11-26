use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use fsrs::{FSRS, MemoryState};

use inki::collection::Collection;
use inki::deck_export::{ExportOptions, build_decks, write_deck};
use inki::deck_file::DeckFile;
use inki::settings::{
    self, DeckSettings, MAX_PER_DAY, NewCardOrder, NewCardPlacement, SettingsSummary,
    StudySettings, check_retention,
};
use inki::train::TrainResult;

/// PC companion for the CrossInk flashcard app.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Study settings stored in each deck file.
#[derive(Args)]
struct StudyArgs {
    /// New cards introduced per day [default for new decks: 20].
    #[arg(long, value_parser = clap::value_parser!(u16).range(0..=i64::from(MAX_PER_DAY)))]
    new_per_day: Option<u16>,
    /// Reviews per day; relearning cards are not limited [default for new decks: 200].
    #[arg(long, value_parser = clap::value_parser!(u16).range(0..=i64::from(MAX_PER_DAY)))]
    reviews_per_day: Option<u16>,
    /// Target probability of remembering a card when it comes due. Higher
    /// means more reviews [default for new decks: 0.9].
    #[arg(long, value_parser = parse_retention)]
    desired_retention: Option<f32>,
    /// Order of new cards [default for new decks: deck].
    #[arg(long, value_enum)]
    new_order: Option<NewCardOrder>,
    /// When new cards are shown [default for new decks: after-reviews].
    #[arg(long, value_enum)]
    new_placement: Option<NewCardPlacement>,
}

impl StudyArgs {
    /// Applies the options given; returns whether any were.
    fn apply_to(&self, settings: &mut DeckSettings) -> bool {
        if let Some(n) = self.new_per_day {
            settings.new_per_day = n;
        }
        if let Some(n) = self.reviews_per_day {
            settings.reviews_per_day = n;
        }
        if let Some(r) = self.desired_retention {
            settings.desired_retention = r;
        }
        let mut study = StudySettings::of(settings);
        if let Some(order) = self.new_order {
            study.new_card_order = order;
        }
        if let Some(placement) = self.new_placement {
            study.new_card_placement = placement;
        }
        study.apply_to(settings);
        self.new_per_day.is_some()
            || self.reviews_per_day.is_some()
            || self.desired_retention.is_some()
            || self.new_order.is_some()
            || self.new_placement.is_some()
    }
}

#[derive(Subcommand)]
enum Command {
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
        /// Train FSRS-7 on the review history in the export and store your
        /// parameters in the decks (FSRS-7 defaults otherwise).
        #[arg(long)]
        train: bool,
        #[command(flatten)]
        study: StudyArgs,
        /// Replace deck files that already exist, discarding progress made
        /// on the e-reader since they were copied.
        #[arg(long)]
        force: bool,
    },
    /// Show or change the study settings of a deck file. Progress is kept.
    /// Don't study on the reader between copying the file off and back.
    Settings {
        /// The deck's .inki file.
        deck_file: PathBuf,
        #[command(flatten)]
        study: StudyArgs,
        /// Train FSRS-7 on the review history of this Anki export and store
        /// the parameters in the deck.
        #[arg(long, value_name = "COLLECTION", conflicts_with = "default_params")]
        train: Option<PathBuf>,
        /// Go back to the FSRS-7 default parameters.
        #[arg(long)]
        default_params: bool,
    },
    /// Open the web app for making and updating decks.
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
        /// Use this deck file's parameters and retention instead of the defaults.
        #[arg(long, value_name = "DECK_FILE")]
        deck: Option<PathBuf>,
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
        Command::Deck {
            collection,
            output,
            deck,
            no_progress,
            train,
            study,
            force,
        } => export_decks(
            &collection,
            &output,
            deck.as_deref(),
            !no_progress,
            train,
            &study,
            force,
        ),
        Command::Settings {
            deck_file,
            study,
            train,
            default_params,
        } => change_settings(&deck_file, &study, train.as_deref(), default_params),
        Command::Serve { port } => {
            tokio::runtime::Runtime::new()?.block_on(inki::server::serve(port))
        }
        Command::Preview {
            history,
            deck,
            desired_retention,
        } => preview(&history, deck.as_deref(), desired_retention),
    }
}

fn train(collection: &Path) -> Result<TrainResult> {
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
    Ok(result)
}

fn read_deck(path: &Path) -> Result<DeckFile> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    DeckFile::decode(&bytes).with_context(|| format!("cannot read {}", path.display()))
}

fn export_decks(
    collection: &Path,
    output: &Path,
    deck: Option<&str>,
    with_progress: bool,
    trained: bool,
    study: &StudyArgs,
    force: bool,
) -> Result<()> {
    let mut deck_settings = DeckSettings::default();
    study.apply_to(&mut deck_settings);
    if trained {
        settings::set_params(&mut deck_settings, &train(collection)?.params)?;
    }
    let fsrs = settings::fsrs(&deck_settings)?;
    let collection = Collection::open(collection)?;
    let decks = build_decks(
        &collection,
        &ExportOptions {
            deck,
            with_progress,
            fsrs: &fsrs,
            desired_retention: deck_settings.desired_retention,
        },
    )?;
    ensure!(!decks.is_empty(), "the collection has no cards to export");

    for deck in &decks {
        let dir = write_deck(output, deck, &deck_settings, force)?;
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
    eprintln!("Settings: {}.", SettingsSummary::of(&deck_settings));
    Ok(())
}

fn change_settings(
    path: &Path,
    study: &StudyArgs,
    train_on: Option<&Path>,
    default_params: bool,
) -> Result<()> {
    let mut deck = read_deck(path)?;
    let mut changed = study.apply_to(&mut deck.settings);
    if let Some(collection) = train_on {
        settings::set_params(&mut deck.settings, &train(collection)?.params)?;
        changed = true;
    }
    if default_params {
        deck.settings.params = DeckSettings::default().params;
        changed = true;
    }
    if changed {
        fs::write(path, deck.encode()?)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    println!(
        "{} ({}): {}.",
        path.display(),
        deck.anki_deck,
        SettingsSummary::of(&deck.settings)
    );
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

fn preview(history: &str, deck: Option<&Path>, retention: Option<f32>) -> Result<()> {
    let settings = match deck {
        Some(path) => read_deck(path)?.settings,
        None => DeckSettings::default(),
    };
    let fsrs = FSRS::new(&settings.params)?;
    let dr = retention.unwrap_or(settings.desired_retention);

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
