//! The FSRS-7 parameter file, `fsrs7-params.txt` in the SD card's
//! `flashcards` folder:
//!
//! ```text
//! # comment
//! fsrs7 0.1104,2.2395,...      (34 numbers)
//! desired_retention 0.9
//! ```
//!
//! Unknown keys are ignored and missing keys keep their defaults.

use crate::fsrs7::{
    DEFAULT_DESIRED_RETENTION, DEFAULT_PARAMS, PARAM_COUNT, Params, RETENTION_MAX, RETENTION_MIN,
    clip_params,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub params: Params,
    pub desired_retention: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            params: DEFAULT_PARAMS,
            desired_retention: DEFAULT_DESIRED_RETENTION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The `fsrs7` line does not hold exactly 34 finite numbers.
    Params,
    /// `desired_retention` is not a number between 0.0001 and 0.9999.
    Retention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    pub kind: ParseErrorKind,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            ParseErrorKind::Params => write!(
                f,
                "line {}: expected {PARAM_COUNT} comma-separated numbers",
                self.line
            ),
            ParseErrorKind::Retention => write!(
                f,
                "line {}: desired retention must be a number from {RETENTION_MIN} to {RETENTION_MAX}",
                self.line
            ),
        }
    }
}

pub fn valid_retention(value: f32) -> bool {
    (RETENTION_MIN..=RETENTION_MAX).contains(&value)
}

/// Parses a decimal number such as `-1.25`, `0.9` or `3e-2`.
///
/// Hand-written instead of `str::parse`, whose general float parser adds
/// about 12 KB to the firmware. Up to 19 significant digits are used; the
/// value is computed in f64 with one exact power of ten, then rounded to f32,
/// which is within one f32 step of exact and identical on PC and reader.
fn parse_number(text: &[u8]) -> Option<f32> {
    let text = text.trim_ascii();
    let (negative, rest) = match text.first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let mut mantissa: u64 = 0;
    let mut digits = 0u32;
    let mut exponent: i32 = 0;
    let mut seen_digit = false;
    let mut seen_point = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            c @ b'0'..=b'9' => {
                seen_digit = true;
                if digits < 19 {
                    mantissa = mantissa * 10 + u64::from(c - b'0');
                    if mantissa != 0 {
                        digits += 1;
                    }
                    if seen_point {
                        exponent -= 1;
                    }
                } else if !seen_point {
                    exponent += 1; // digits beyond 19 only scale the value
                }
            }
            b'.' if !seen_point => seen_point = true,
            b'e' | b'E' => break,
            _ => return None,
        }
        i += 1;
    }
    if !seen_digit {
        return None;
    }
    if i < rest.len() {
        let exp_text = core::str::from_utf8(&rest[i + 1..]).ok()?;
        let exp: i32 = exp_text.parse().ok()?;
        exponent = exponent.checked_add(exp)?;
    }
    let value = scale(mantissa as f64, exponent) as f32;
    let value = if negative { -value } else { value };
    value.is_finite().then_some(value)
}

/// `value × 10^exponent`, with exact powers of ten where possible.
fn scale(value: f64, exponent: i32) -> f64 {
    const POWERS: [f64; 23] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
    ];
    if value == 0.0 {
        return 0.0;
    }
    let mut value = value;
    let mut exponent = exponent;
    while exponent > 22 {
        value *= 1e22;
        exponent -= 22;
        if value.is_infinite() {
            return value;
        }
    }
    while exponent < -22 {
        value /= 1e22;
        exponent += 22;
        if value == 0.0 {
            return value;
        }
    }
    if exponent >= 0 {
        value * POWERS[exponent as usize]
    } else {
        value / POWERS[(-exponent) as usize]
    }
}

fn parse_params(values: &[u8]) -> Option<Params> {
    let mut params = [0.0f32; PARAM_COUNT];
    let mut count = 0;
    for value in values.split(|&b| b == b',') {
        *params.get_mut(count)? = parse_number(value)?;
        count += 1;
    }
    (count == PARAM_COUNT).then_some(params)
}

/// Parses a parameter file. Parameters are clipped to the ranges FSRS-7
/// accepts.
pub fn parse(text: &[u8]) -> Result<Config, ParseError> {
    let mut config = Config::default();
    for (index, line) in text.split(|&b| b == b'\n').enumerate() {
        let line = line.trim_ascii();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let split = line
            .iter()
            .position(|&b| b == b' ' || b == b'\t')
            .unwrap_or(line.len());
        let (key, value) = (&line[..split], &line[split..]);
        let error = |kind| ParseError {
            line: index + 1,
            kind,
        };
        match key {
            b"fsrs7" => {
                config.params = parse_params(value).ok_or(error(ParseErrorKind::Params))?;
            }
            b"desired_retention" => {
                config.desired_retention = parse_number(value)
                    .filter(|&v| valid_retention(v))
                    .ok_or(error(ParseErrorKind::Retention))?;
            }
            _ => {}
        }
    }
    clip_params(&mut config.params);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(params: &[f32]) -> [u8; 1024] {
        let mut out = [0u8; 1024];
        let mut w = Writer(&mut out, 0);
        use core::fmt::Write;
        write!(w, "fsrs7 ").unwrap();
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                write!(w, ",").unwrap();
            }
            write!(w, "{p}").unwrap();
        }
        out
    }

    struct Writer<'a>(&'a mut [u8], usize);

    impl core::fmt::Write for Writer<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            self.0[self.1..self.1 + s.len()].copy_from_slice(s.as_bytes());
            self.1 += s.len();
            Ok(())
        }
    }

    fn text(buf: &[u8; 1024]) -> &[u8] {
        let end = buf.iter().position(|&b| b == 0).unwrap();
        &buf[..end]
    }

    #[test]
    fn parses_file() {
        let mut params = DEFAULT_PARAMS;
        params[0] = 0.25;
        let buf = line(&params);
        let mut file = [0u8; 1200];
        let head = b"# inki\r\n";
        let tail = b"\r\ndesired_retention 0.85\nunknown 1\n";
        let body = text(&buf);
        file[..head.len()].copy_from_slice(head);
        file[head.len()..head.len() + body.len()].copy_from_slice(body);
        let end = head.len() + body.len();
        file[end..end + tail.len()].copy_from_slice(tail);
        let config = parse(&file[..end + tail.len()]).unwrap();
        assert_eq!(config.params, params);
        assert_eq!(config.desired_retention, 0.85);
    }

    #[test]
    fn defaults_and_errors() {
        assert_eq!(parse(b"# nothing\n\n").unwrap(), Config::default());
        let short = line(&DEFAULT_PARAMS[..33]);
        assert_eq!(
            parse(text(&short)),
            Err(ParseError {
                line: 1,
                kind: ParseErrorKind::Params
            })
        );
        let mut long = [0.0f32; 35];
        long[..34].copy_from_slice(&DEFAULT_PARAMS);
        assert!(parse(text(&line(&long))).is_err());
        assert!(parse(b"fsrs7 nan").is_err());
        assert_eq!(
            parse(b"\ndesired_retention 1.5"),
            Err(ParseError {
                line: 2,
                kind: ParseErrorKind::Retention
            })
        );
        assert!(parse(b"desired_retention abc").is_err());
    }

    #[test]
    fn parses_numbers() {
        assert_eq!(parse_number(b"0.9"), Some(0.9));
        assert_eq!(parse_number(b" -1.25 "), Some(-1.25));
        assert_eq!(parse_number(b"+3"), Some(3.0));
        assert_eq!(parse_number(b".5"), Some(0.5));
        assert_eq!(parse_number(b"5."), Some(5.0));
        assert_eq!(parse_number(b"3e-2"), Some(0.03));
        assert_eq!(parse_number(b"1.5E3"), Some(1500.0));
        assert_eq!(parse_number(b"0.000"), Some(0.0));
        for bad in [
            &b""[..],
            b"-",
            b".",
            b"1.2.3",
            b"abc",
            b"1e",
            b"nan",
            b"inf",
            b"1e99",
            b"1,5",
        ] {
            assert_eq!(parse_number(bad), None, "{:?}", core::str::from_utf8(bad));
        }
    }

    /// Every value Rust prints for an f32 parses back to the same f32 in
    /// these ranges, which cover FSRS parameters.
    #[test]
    fn round_trips_printed_f32() {
        use core::fmt::Write;
        let mut x: u32 = 0x1234_5678;
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let value = f32::from_bits(x);
            if !value.is_finite() || !(1e-6..1e6).contains(&value.abs()) {
                continue;
            }
            let mut buf = [0u8; 64];
            let mut w = Writer(&mut buf, 0);
            write!(w, "{value}").unwrap();
            let len = w.1;
            assert_eq!(parse_number(&buf[..len]), Some(value), "{value}");
        }
    }

    #[test]
    fn clips_out_of_range_params() {
        let mut params = DEFAULT_PARAMS;
        params[24] = 3.0;
        assert_eq!(parse(text(&line(&params))).unwrap().params[24], 0.95);
    }
}
