//! The FSRS-7 parameter file copied to the device's SD card.
//!
//! Plain text, one `key value` per line, read on the device by
//! `fsrs7::parseConfig` in `firmware/lib/Fsrs7`:
//!
//! ```text
//! # comment
//! fsrs7 0.1104,2.2395,...      (34 values)
//! desired_retention 0.9
//! ```

use anyhow::{Context, Result, bail, ensure};

pub const PARAM_COUNT: usize = 34;
pub const FILE_NAME: &str = "fsrs7-params.txt";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub params: Vec<f32>,
    pub desired_retention: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            params: fsrs::DEFAULT_PARAMETERS.to_vec(),
            desired_retention: 0.9,
        }
    }
}

pub fn check_retention(value: f32) -> Result<f32> {
    ensure!(
        (0.0001..=0.9999).contains(&value),
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

    /// Parses the file. Missing keys keep their defaults; unknown keys are ignored.
    pub fn parse(text: &str) -> Result<Self> {
        let mut config = Self::default();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once([' ', '\t']).unwrap_or((line, ""));
            let value = value.trim();
            let context = || format!("line {}: {line}", number + 1);
            match key {
                "fsrs7" => {
                    let params = value
                        .split(',')
                        .map(|v| v.trim().parse::<f32>())
                        .collect::<Result<Vec<_>, _>>()
                        .with_context(context)?;
                    if params.len() != PARAM_COUNT || params.iter().any(|v| !v.is_finite()) {
                        bail!("{}: expected {PARAM_COUNT} finite numbers", context());
                    }
                    config.params = params;
                }
                "desired_retention" => {
                    let value = value.parse::<f32>().with_context(context)?;
                    config.desired_retention = check_retention(value).with_context(context)?;
                }
                _ => {}
            }
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_exactly() {
        let mut config = Config::default();
        config.params[3] = 1.0 / 3.0;
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
