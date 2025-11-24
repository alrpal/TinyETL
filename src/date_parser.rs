use crate::schema::Value;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// A reusable date parser that attempts to parse common date/datetime formats
/// and returns a standardized DateTime<Utc> value
pub struct DateParser;

impl DateParser {
    /// Attempts to parse a string value as a date/datetime in various common formats
    /// Returns Some(Value::Date) if successful, None otherwise
    pub fn try_parse(value: &str) -> Option<Value> {
        let trimmed = value.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Try various formats in order of likelihood/specificity

        // 1. RFC 3339 / ISO 8601 with timezone (most specific)
        if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
            return Some(Value::Date(dt.with_timezone(&Utc)));
        }

        // 2. ISO 8601 without timezone (assume UTC)
        if let Some(dt) = Self::try_iso_formats(trimmed) {
            return Some(Value::Date(dt));
        }

        // 3. Common date formats (YYYY-MM-DD, MM/DD/YYYY, DD/MM/YYYY, etc.)
        if let Some(dt) = Self::try_date_formats(trimmed) {
            return Some(Value::Date(dt));
        }

        // 4. Common datetime formats without timezone
        if let Some(dt) = Self::try_datetime_formats(trimmed) {
            return Some(Value::Date(dt));
        }

        None
    }

    /// Try ISO 8601 formats without explicit timezone
    fn try_iso_formats(value: &str) -> Option<DateTime<Utc>> {
        // ISO 8601 datetime without timezone: 2023-12-25T10:30:00
        if let Ok(ndt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
            return Some(Utc.from_utc_datetime(&ndt));
        }

        // ISO 8601 datetime with milliseconds: 2023-12-25T10:30:00.123
        if let Ok(ndt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.3f") {
            return Some(Utc.from_utc_datetime(&ndt));
        }

        // ISO 8601 datetime with microseconds: 2023-12-25T10:30:00.123456
        if let Ok(ndt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.6f") {
            return Some(Utc.from_utc_datetime(&ndt));
        }

        None
    }

    /// Try common date-only formats (assume midnight UTC)
    fn try_date_formats(value: &str) -> Option<DateTime<Utc>> {
        let date_formats = vec![
            "%Y-%m-%d",  // 2023-12-25
            "%m/%d/%Y",  // 12/25/2023
            "%d/%m/%Y",  // 25/12/2023
            "%m-%d-%Y",  // 12-25-2023
            "%d-%m-%Y",  // 25-12-2023
            "%Y/%m/%d",  // 2023/12/25
            "%d.%m.%Y",  // 25.12.2023
            "%Y.%m.%d",  // 2023.12.25
            "%b %d, %Y", // Dec 25, 2023
            "%B %d, %Y", // December 25, 2023
            "%d %b %Y",  // 25 Dec 2023
            "%d %B %Y",  // 25 December 2023
        ];

        for format in date_formats {
            if let Ok(date) = NaiveDate::parse_from_str(value, format) {
                if let Some(datetime) = date.and_hms_opt(0, 0, 0) {
                    return Some(Utc.from_utc_datetime(&datetime));
                }
            }
        }

        None
    }

    /// Try common datetime formats without timezone (assume UTC)
    fn try_datetime_formats(value: &str) -> Option<DateTime<Utc>> {
        let datetime_formats = vec![
            "%Y-%m-%d %H:%M:%S",     // 2023-12-25 10:30:00
            "%Y-%m-%d %H:%M:%S%.3f", // 2023-12-25 10:30:00.123
            "%m/%d/%Y %H:%M:%S",     // 12/25/2023 10:30:00
            "%d/%m/%Y %H:%M:%S",     // 25/12/2023 10:30:00
            "%Y-%m-%d %H:%M",        // 2023-12-25 10:30
            "%m/%d/%Y %H:%M",        // 12/25/2023 10:30
            "%d/%m/%Y %H:%M",        // 25/12/2023 10:30
            "%Y-%m-%d %I:%M:%S %p",  // 2023-12-25 10:30:00 AM
            "%m/%d/%Y %I:%M:%S %p",  // 12/25/2023 10:30:00 AM
            "%d/%m/%Y %I:%M:%S %p",  // 25/12/2023 10:30:00 AM
            "%Y-%m-%d %I:%M %p",     // 2023-12-25 10:30 AM
            "%m/%d/%Y %I:%M %p",     // 12/25/2023 10:30 AM
            "%d/%m/%Y %I:%M %p",     // 25/12/2023 10:30 AM
        ];

        for format in datetime_formats {
            if let Ok(ndt) = NaiveDateTime::parse_from_str(value, format) {
                return Some(Utc.from_utc_datetime(&ndt));
            }
        }

        None
    }

    /// Checks if a string might be a date based on simple heuristics
    /// This can be used as a quick pre-filter before attempting full parsing
    pub fn might_be_date(value: &str) -> bool {
        let trimmed = value.trim();

        if trimmed.len() < 6 || trimmed.len() > 30 {
            return false;
        }

        // Exclude pure decimal numbers (just digits and a single dot)
        if trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
            && trimmed.matches('.').count() == 1
        {
            return false;
        }

        // Look for date-like patterns
        let has_date_separators = trimmed.contains('/')
            || trimmed.contains('-')
            || trimmed.contains('.')
            || trimmed.contains('T')
            || trimmed.contains(' ');

        let has_digits = trimmed.chars().any(|c| c.is_ascii_digit());

        // Look for month names
        let has_month_name = [
            "jan",
            "feb",
            "mar",
            "apr",
            "may",
            "jun",
            "jul",
            "aug",
            "sep",
            "oct",
            "nov",
            "dec",
            "january",
            "february",
            "march",
            "april",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
        ]
        .iter()
        .any(|month| trimmed.to_lowercase().contains(month));

        has_digits && (has_date_separators || has_month_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn test_rfc3339_formats() {
        let cases = vec![
            "2023-12-25T10:30:00Z",
            "2023-12-25T10:30:00+00:00",
            "2023-12-25T10:30:00-05:00",
            "2023-12-25T10:30:00.123Z",
        ];

        for case in cases {
            let result = DateParser::try_parse(case);
            assert!(result.is_some(), "Failed to parse: {}", case);
            if let Some(Value::Date(dt)) = result {
                assert_eq!(dt.year(), 2023);
                assert_eq!(dt.month(), 12);
                assert_eq!(dt.day(), 25);
            }
        }
    }

    #[test]
    fn test_iso_formats() {
        let cases = vec![
            ("2023-12-25T10:30:00", 10, 30, 0),
            ("2023-12-25T10:30:00.123", 10, 30, 0),
            ("2023-12-25T10:30:00.123456", 10, 30, 0),
        ];

        for (case, expected_hour, expected_min, expected_sec) in cases {
            let result = DateParser::try_parse(case);
            assert!(result.is_some(), "Failed to parse: {}", case);
            if let Some(Value::Date(dt)) = result {
                assert_eq!(dt.year(), 2023);
                assert_eq!(dt.month(), 12);
                assert_eq!(dt.day(), 25);
                assert_eq!(dt.hour(), expected_hour);
                assert_eq!(dt.minute(), expected_min);
                assert_eq!(dt.second(), expected_sec);
            }
        }
    }

    #[test]
    fn test_date_only_formats() {
        let cases = vec![
            "2023-12-25",
            "12/25/2023",
            "12-25-2023",
            "2023/12/25",
            "25.12.2023",
            "Dec 25, 2023",
            "December 25, 2023",
            "25 Dec 2023",
            "25 December 2023",
        ];

        for case in cases {
            let result = DateParser::try_parse(case);
            assert!(result.is_some(), "Failed to parse: {}", case);
            if let Some(Value::Date(dt)) = result {
                assert_eq!(dt.year(), 2023);
                // Note: For ambiguous formats like "25/12/2023", this assumes DD/MM/YYYY
                // and for "12/25/2023" it assumes MM/DD/YYYY
                assert!(
                    dt.month() == 12 || dt.day() == 25,
                    "Date parsing failed for {}: got month={}, day={}",
                    case,
                    dt.month(),
                    dt.day()
                );
            }
        }
    }

    #[test]
    fn test_datetime_formats() {
        let cases = vec![
            "2023-12-25 10:30:00",
            "12/25/2023 10:30:00",
            "2023-12-25 10:30",
            "12/25/2023 10:30:00 AM",
            "2023-12-25 10:30 AM",
        ];

        for case in cases {
            let result = DateParser::try_parse(case);
            assert!(result.is_some(), "Failed to parse: {}", case);
            if let Some(Value::Date(dt)) = result {
                assert_eq!(dt.year(), 2023);
                // Basic validation that we got some reasonable time
                assert!(dt.hour() <= 23);
                assert!(dt.minute() <= 59);
            }
        }
    }

    #[test]
    fn test_might_be_date_heuristics() {
        let positive_cases = vec![
            "2023-12-25",
            "12/25/2023",
            "Dec 25, 2023",
            "2023-12-25T10:30:00",
            "25.12.2023",
        ];

        for case in positive_cases {
            assert!(
                DateParser::might_be_date(case),
                "{} should be detected as date-like",
                case
            );
        }

        let negative_cases = vec![
            "hello",
            "123",
            "true",
            "3.14159",
            "",
            "a",
            "this is just text with no dates",
        ];

        for case in negative_cases {
            assert!(
                !DateParser::might_be_date(case),
                "{} should not be detected as date-like",
                case
            );
        }
    }

    #[test]
    fn test_invalid_dates() {
        let invalid_cases = vec![
            "not a date",
            "123abc",
            "13/25/2023", // Invalid month
            "12/32/2023", // Invalid day
            "2023-13-01", // Invalid month
            "",
            "just text",
        ];

        for case in invalid_cases {
            let result = DateParser::try_parse(case);
            assert!(result.is_none(), "Should not parse invalid date: {}", case);
        }
    }
}
