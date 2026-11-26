//! Per-deck study settings, stored in each deck file: daily limits, desired
//! retention and the FSRS-7 parameters (`inki_core::format::DeckSettings`).

use anyhow::{Result, ensure};
use fsrs::FSRS;
use serde::{Deserialize, Serialize};

pub use inki_core::format::DeckSettings;
use inki_core::fsrs7::{PARAM_COUNT, RETENTION_MAX, RETENTION_MIN, valid_retention};

/// Highest daily limit accepted.
pub const MAX_PER_DAY: u16 = 9999;

pub fn check_retention(value: f32) -> Result<f32> {
    ensure!(
        valid_retention(value),
        "desired retention must be between {RETENTION_MIN} and {RETENTION_MAX}, got {value}"
    );
    Ok(value)
}

/// The settings a person picks; the parameters come from training.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StudySettings {
    pub new_per_day: u16,
    pub reviews_per_day: u16,
    pub desired_retention: f32,
}

impl Default for StudySettings {
    fn default() -> Self {
        Self::of(&DeckSettings::default())
    }
}

impl StudySettings {
    pub fn of(settings: &DeckSettings) -> Self {
        Self {
            new_per_day: settings.new_per_day,
            reviews_per_day: settings.reviews_per_day,
            desired_retention: settings.desired_retention,
        }
    }

    pub fn check(self) -> Result<Self> {
        ensure!(
            self.new_per_day <= MAX_PER_DAY && self.reviews_per_day <= MAX_PER_DAY,
            "daily limits must be at most {MAX_PER_DAY}"
        );
        check_retention(self.desired_retention)?;
        Ok(self)
    }

    /// Sets the limits and retention, keeping the parameters.
    pub fn apply_to(self, settings: &mut DeckSettings) {
        settings.new_per_day = self.new_per_day;
        settings.reviews_per_day = self.reviews_per_day;
        settings.desired_retention = self.desired_retention;
    }
}

/// Puts trained parameters into `settings`.
pub fn set_params(settings: &mut DeckSettings, params: &[f32]) -> Result<()> {
    ensure!(
        params.len() == PARAM_COUNT,
        "expected {PARAM_COUNT} FSRS-7 parameters, got {}",
        params.len()
    );
    settings.params.copy_from_slice(params);
    inki_core::fsrs7::clip_params(&mut settings.params);
    Ok(())
}

/// The scheduler for these settings, used to carry over Anki progress.
pub fn fsrs(settings: &DeckSettings) -> Result<FSRS> {
    Ok(FSRS::new(&settings.params)?)
}

/// For display: the settings and whether the parameters are trained.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SettingsSummary {
    #[serde(flatten)]
    pub study: StudySettings,
    pub trained_params: bool,
}

impl SettingsSummary {
    pub fn of(settings: &DeckSettings) -> Self {
        Self {
            study: StudySettings::of(settings),
            trained_params: !settings.uses_default_params(),
        }
    }
}

impl std::fmt::Display for SettingsSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reviews = self.study.reviews_per_day;
        write!(
            f,
            "{} new and {} review{} a day, {:.0}% desired retention, {} FSRS-7 parameters",
            self.study.new_per_day,
            reviews,
            if reviews == 1 { "" } else { "s" },
            self.study.desired_retention * 100.0,
            if self.trained_params {
                "trained"
            } else {
                "default"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_and_applies() {
        let mut settings = DeckSettings::default();
        let study = StudySettings {
            new_per_day: 5,
            reviews_per_day: 50,
            desired_retention: 0.85,
        };
        study.check().unwrap().apply_to(&mut settings);
        assert_eq!(StudySettings::of(&settings), study);
        assert!(settings.uses_default_params());
        assert!(
            StudySettings {
                new_per_day: 10_000,
                ..study
            }
            .check()
            .is_err()
        );
        assert!(
            StudySettings {
                desired_retention: 1.0,
                ..study
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn trained_params() {
        let mut settings = DeckSettings::default();
        let mut params = settings.params;
        params[0] = 0.5;
        set_params(&mut settings, &params).unwrap();
        let summary = SettingsSummary::of(&settings);
        assert!(summary.trained_params);
        assert_eq!(
            summary.to_string(),
            "20 new and 200 reviews a day, 90% desired retention, trained FSRS-7 parameters"
        );
        assert!(set_params(&mut settings, &[1.0]).is_err());
        fsrs(&settings).unwrap();
    }
}
