use core::{fmt, str::FromStr, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseOffsetError {
    WrongFormat,
    NotANumber,
    OutOfRange,
}

impl fmt::Display for ParseOffsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseOffsetError::WrongFormat => write!(f, "time offset isn't H:MM:SS.mmm"),
            ParseOffsetError::NotANumber => write!(f, "time offset contains a non-digit"),
            ParseOffsetError::OutOfRange => write!(f, "minutes or seconds are 60 or more"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseLineError {
    MissingJson,
    BadOffset(ParseOffsetError),
}

impl fmt::Display for ParseLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseLineError::MissingJson => write!(f, "line has no JSON object"),
            ParseLineError::BadOffset(e) => write!(f, "bad time offset: {e}"),
        }
    }
}

impl From<ParseOffsetError> for ParseLineError {
    fn from(e: ParseOffsetError) -> Self {
        ParseLineError::BadOffset(e)
    }
}

/// Time since the start of the stream, written `HH:MM:SS.mmm`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offset(pub Duration);

impl FromStr for Offset {
    type Err = ParseOffsetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use ParseOffsetError::{OutOfRange, WrongFormat};

        let mut parts = s.split(':');
        let h = parts.next().ok_or(WrongFormat)?;
        let m = parts.next().ok_or(WrongFormat)?;
        let rest = parts.next().ok_or(WrongFormat)?;
        if parts.next().is_some() {
            return Err(WrongFormat);
        }

        let (secs, ms) = rest.split_once('.').ok_or(WrongFormat)?;
        // "11.23" would otherwise read as 23 ms instead of 230.
        if ms.len() != 3 {
            return Err(WrongFormat);
        }

        let h = digits(h)?;
        let m = digits(m)?;
        let secs = digits(secs)?;
        let ms = digits(ms)?;
        if m >= 60 || secs >= 60 {
            return Err(OutOfRange);
        }

        // Checked, so absurd hours are an error instead of an overflow panic.
        let total = h
            .checked_mul(3600)
            .and_then(|s| s.checked_add(m * 60 + secs))
            .ok_or(OutOfRange)?;
        Ok(Offset(
            Duration::from_secs(total) + Duration::from_millis(ms),
        ))
    }
}

/// Parses a non-empty run of ASCII digits. Unlike `u64::from_str` alone, it
/// rejects a leading `+`.
fn digits(part: &str) -> Result<u64, ParseOffsetError> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseOffsetError::NotANumber);
    }
    // Only overflow can fail here.
    part.parse().map_err(|_| ParseOffsetError::OutOfRange)
}

/// One archive line, split. `json` borrows from the input, so nothing is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line<'a> {
    pub offset: Duration,
    pub json: &'a str,
}

pub fn parse_line(line: &str) -> Result<Line<'_>, ParseLineError> {
    let line = line.trim_start_matches('\u{feff}').trim_end();
    let start = line.find('{').ok_or(ParseLineError::MissingJson)?;
    let (offset, json) = line.split_at(start);
    let Offset(offset) = offset.parse()?;
    Ok(Line { offset, json })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(s: &str) -> Result<Duration, ParseOffsetError> {
        s.parse::<Offset>().map(|Offset(d)| d)
    }

    #[test]
    fn offset_parses_hours_minutes_seconds_and_millis() {
        assert_eq!(
            offset("02:48:11.236"),
            Ok(Duration::from_millis(10_091_236))
        );
    }

    #[test]
    fn offset_zero_parses_to_zero_duration() {
        assert_eq!(offset("00:00:00.000"), Ok(Duration::ZERO));
    }

    #[test]
    fn offset_accepts_single_digit_and_long_hours() {
        assert_eq!(offset("1:00:00.000"), Ok(Duration::from_secs(3600)));
        assert_eq!(offset("100:00:00.000"), Ok(Duration::from_secs(360_000)));
    }

    #[test]
    fn offset_accepts_59_minutes_and_59_seconds() {
        assert_eq!(offset("00:59:59.999"), Ok(Duration::from_millis(3_599_999)));
    }

    #[test]
    fn offset_rejects_minutes_over_59() {
        assert_eq!(offset("00:60:00.000"), Err(ParseOffsetError::OutOfRange));
    }

    #[test]
    fn offset_rejects_seconds_over_59() {
        assert_eq!(offset("00:00:60.000"), Err(ParseOffsetError::OutOfRange));
    }

    #[test]
    fn offset_rejects_two_digit_millis() {
        assert_eq!(offset("00:00:01.23"), Err(ParseOffsetError::WrongFormat));
    }

    #[test]
    fn offset_rejects_four_digit_millis() {
        assert_eq!(offset("00:00:01.2345"), Err(ParseOffsetError::WrongFormat));
    }

    #[test]
    fn offset_rejects_missing_millis() {
        assert_eq!(offset("00:00:01"), Err(ParseOffsetError::WrongFormat));
    }

    #[test]
    fn offset_rejects_missing_part() {
        assert_eq!(offset("00:01.000"), Err(ParseOffsetError::WrongFormat));
    }

    #[test]
    fn offset_rejects_extra_colon() {
        assert_eq!(offset("1:00:00:00.000"), Err(ParseOffsetError::WrongFormat));
    }

    #[test]
    fn offset_rejects_empty_string() {
        assert_eq!(offset(""), Err(ParseOffsetError::WrongFormat));
    }

    #[test]
    fn offset_rejects_letters() {
        assert_eq!(offset("aa:00:00.000"), Err(ParseOffsetError::NotANumber));
    }

    #[test]
    fn offset_rejects_empty_part() {
        assert_eq!(offset(":00:00.000"), Err(ParseOffsetError::NotANumber));
    }

    #[test]
    fn offset_rejects_plus_sign() {
        assert_eq!(offset("+1:00:00.000"), Err(ParseOffsetError::NotANumber));
    }

    #[test]
    fn offset_rejects_hours_that_overflow() {
        assert_eq!(
            offset("99999999999999999999:00:00.000"),
            Err(ParseOffsetError::OutOfRange)
        );
    }

    #[test]
    fn offset_rejects_hours_too_large_for_seconds() {
        // Fits in a u64, but times 3600 doesn't.
        assert_eq!(
            offset("10000000000000000:00:00.000"),
            Err(ParseOffsetError::OutOfRange)
        );
    }

    #[test]
    fn line_splits_offset_and_json() {
        let line = parse_line(r#"00:56:55.761{"Status":"Started"}"#).unwrap();
        assert_eq!(line.offset, Duration::from_millis(3_415_761));
        assert_eq!(line.json, r#"{"Status":"Started"}"#);
    }

    #[test]
    fn line_with_bom_parses_like_line_without_bom() {
        let plain = r#"00:00:05.043{"Status":"Inactive"}"#;
        let with_bom = "\u{feff}00:00:05.043{\"Status\":\"Inactive\"}";
        assert_eq!(parse_line(with_bom), parse_line(plain));
    }

    #[test]
    fn line_trailing_carriage_return_is_dropped() {
        let line = parse_line("00:00:01.000{\"Status\":\"1\"}\r").unwrap();
        assert_eq!(line.json, r#"{"Status":"1"}"#);
    }

    #[test]
    fn line_json_is_returned_unchanged() {
        let json = r#"{"Messages":{"34":{"Flag":"CLEAR","Message":"A {nested} brace"}}}"#;
        let raw = format!("01:00:14.225{json}");
        assert_eq!(parse_line(&raw).unwrap().json, json);
    }

    #[test]
    fn line_without_json_is_missing_json() {
        assert_eq!(parse_line("00:00:01.000"), Err(ParseLineError::MissingJson));
    }

    #[test]
    fn empty_line_is_missing_json() {
        assert_eq!(parse_line(""), Err(ParseLineError::MissingJson));
    }

    #[test]
    fn line_with_bad_offset_reports_why() {
        assert_eq!(
            parse_line(r#"xx:00:01.000{"Status":"1"}"#),
            Err(ParseLineError::BadOffset(ParseOffsetError::NotANumber))
        );
    }

    #[test]
    fn line_error_message_includes_offset_reason() {
        let err = ParseLineError::BadOffset(ParseOffsetError::OutOfRange);
        assert_eq!(
            err.to_string(),
            "bad time offset: minutes or seconds are 60 or more"
        );
    }
}
