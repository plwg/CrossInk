//! The FSRS-7 parameter file copied to the device's SD card. Parsing is
//! shared with the reader through `inki_core::params`; this module adds
//! writing it.
//!
//! ```text
//! # comment
//! fsrs7 0.1104,2.2395,...      (34 values)
//! desired_retention 0.9
//! ```

use anyhow::{Result, anyhow, ensure};
use inki_core::params;

pub const PARAM_COUNT: usize = inki_core::fsrs7::PARAM_COUNT;
pub const FILE_NAME: &str = "fsrs7-params.txt";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub params: Vec<f32>,
    pub desired_retention: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            params: inki_core::fsrs7::DEFAULT_PARAMS.to_vec(),
            desired_retention: inki_core::fsrs7::DEFAULT_DESIRED_RETENTION,
        }
    }
}

pub fn check_retention(value: f32) -> Result<f32> {
    ensure!(
        params::valid_retention(value),
        "desired retention must be between 0.0001 and 0.9999, got {value}"
    );
    Ok(value)
}

impl Config {
    /// Writes the file. `comments` become `#` lines at the top.
    pub fn to_text(&self, comments: &[String]) -> String {
        let mut out = String::new();
        for comment in comments {
            out.push_str("# ");
            out.push_str(comment);
            out.push('\n');
        }
        // `{}` prints the shortest text that parses back to the same f32.
        let values: Vec<String> = self.params.iter().map(f32::to_string).collect();
        out.push_str(&format!("fsrs7 {}\n", values.join(",")));
        out.push_str(&format!("desired_retention {}\n", self.desired_retention));
        out
    }

    /// Parses the file exactly as the reader does: missing keys keep their
    /// defaults, unknown keys are ignored, parameters are clipped to the
    /// ranges FSRS-7 accepts.
    pub fn parse(text: &str) -> Result<Self> {
        let parsed = params::parse(text.as_bytes()).map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            params: parsed.params.to_vec(),
            desired_retention: parsed.desired_retention,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_exactly() {
        let mut config = Config::default();
        config.params[3] = 12.345_679;
        config.desired_retention = 0.87;
        let text = config.to_text(&["trained on 10 reviews".into()]);
        assert!(text.starts_with("# trained on 10 reviews\nfsrs7 "));
        assert_eq!(Config::parse(&text).unwrap(), config);
    }

    #[test]
    fn missing_keys_keep_defaults() {
        assert_eq!(
            Config::parse("# nothing\n\nother 1").unwrap(),
            Config::default()
        );
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Config::parse("fsrs7 1,2,3").is_err());
        assert!(Config::parse("desired_retention 1.2").is_err());
        assert!(Config::parse("desired_retention x").is_err());
        let mut nan = Config::default();
        nan.params[0] = f32::NAN;
        assert!(Config::parse(&nan.to_text(&[])).is_err());
    }
}
