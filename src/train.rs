//! Trains FSRS-7 parameters on an Anki review history.

use std::path::Path;

use anyhow::{Context, Result, ensure};
use fsrs::{ComputeParametersInput, FSRS};
use serde::Serialize;

use crate::collection::read_revlog;
use crate::config::Config;
use crate::revlog::training_set;

/// How well a set of parameters predicts the review history (lower is better).
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Fit {
    pub log_loss: f32,
    pub rmse_bins: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainResult {
    pub reviews: usize,
    pub cards: usize,
    pub defaults: Fit,
    pub trained: Fit,
    pub params: Vec<f32>,
}

impl TrainResult {
    /// The parameter file text for the SD card.
    pub fn config_text(&self, desired_retention: f32) -> String {
        let config = Config {
            params: self.params.clone(),
            desired_retention,
        };
        config.to_text(&[
            "inki FSRS-7 parameters".to_string(),
            format!(
                "trained on {} reviews of {} cards; log loss {:.4} (defaults {:.4})",
                self.reviews, self.cards, self.trained.log_loss, self.defaults.log_loss
            ),
        ])
    }
}

/// Trains on the review history of a collection or package. Takes from a
/// few seconds to a few minutes depending on its size.
pub fn train(collection: &Path) -> Result<TrainResult> {
    let set = training_set(read_revlog(collection)?);
    ensure!(
        !set.items.is_empty(),
        "no usable review history in {}. An .apkg must be exported with \
         \"Include scheduling information\"; a .colpkg always has it.",
        collection.display()
    );
    let params = fsrs::compute_parameters(ComputeParametersInput {
        train_set: set.items.clone(),
        card_ids: Some(set.card_ids.clone()),
        ..Default::default()
    })
    .context("training failed")?;
    let defaults = FSRS::default().evaluate(set.items.clone(), |_| true)?;
    let trained = FSRS::new(&params)?.evaluate(set.items, |_| true)?;
    Ok(TrainResult {
        reviews: set.reviews,
        cards: set.cards,
        defaults: Fit {
            log_loss: defaults.log_loss,
            rmse_bins: defaults.rmse_bins,
        },
        trained: Fit {
            log_loss: trained.log_loss,
            rmse_bins: trained.rmse_bins,
        },
        params,
    })
}
