use crate::schema::Value;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// Which reading of a numeric date like `03/04/2024` to prefer: 4 March (month first, the
/// default) or 3 April (day first). A column is day first when one of its values only reads
/// that way (`13/04/2024`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DateOrder {
    #[default]
    MonthFirst,
    DayFirst,
}

/// Which orders a layout belongs to
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Unambiguous: ISO, month names, `25.12.2023`
    Any,
    MonthFirst,
    DayFirst,
}

/// What a text value is, as a date
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateMatch {
    /// The text has a time of day (`2024-01-15 00:00` counts); otherwise it's a date
    pub has_time: bool,
    /// It reads as a date month first / day first (both, for unambiguous layouts)
    pub month_first: bool,
    pub day_first: bool,
}

/// Date-only layouts (midnight UTC)
const DATES: &[(&str, Layout)] = &[
    ("%Y-%m-%d", Layout::Any),
    ("%Y/%m/%d", Layout::Any),
    ("%Y.%m.%d", Layout::Any),
    ("%m/%d/%Y", Layout::MonthFirst),
    ("%d/%m/%Y", Layout::DayFirst),
    ("%m-%d-%Y", Layout::MonthFirst),
    ("%d-%m-%Y", Layout::DayFirst),
    ("%d.%m.%Y", Layout::Any),
    ("%b %d, %Y", Layout::Any),
    ("%B %d, %Y", Layout::Any),
    ("%d %b %Y", Layout::Any),
    ("%d %B %Y", Layout::Any),
];

/// Date and time layouts without a zone (UTC). `%.f` takes any number of fractional digits.
const DATETIMES: &[(&str, Layout)] = &[
    ("%Y-%m-%dT%H:%M:%S%.f", Layout::Any),
    ("%Y-%m-%dT%H:%M:%S", Layout::Any),
    ("%Y-%m-%dT%H:%M", Layout::Any),
    ("%Y-%m-%d %H:%M:%S%.f", Layout::Any),
    ("%Y-%m-%d %H:%M:%S", Layout::Any),
    ("%Y-%m-%d %H:%M", Layout::Any),
    ("%Y-%m-%d %I:%M:%S %p", Layout::Any),
    ("%Y-%m-%d %I:%M %p", Layout::Any),
    ("%m/%d/%Y %H:%M:%S%.f", Layout::MonthFirst),
    ("%m/%d/%Y %H:%M:%S", Layout::MonthFirst),
    ("%m/%d/%Y %H:%M", Layout::MonthFirst),
    ("%m/%d/%Y %I:%M:%S %p", Layout::MonthFirst),
    ("%m/%d/%Y %I:%M %p", Layout::MonthFirst),
    ("%d/%m/%Y %H:%M:%S%.f", Layout::DayFirst),
    ("%d/%m/%Y %H:%M:%S", Layout::DayFirst),
    ("%d/%m/%Y %H:%M", Layout::DayFirst),
    ("%d/%m/%Y %I:%M:%S %p", Layout::DayFirst),
    ("%d/%m/%Y %I:%M %p", Layout::DayFirst),
];

/// Layouts with a UTC offset, besides RFC 3339 (`2024-01-15 10:30:00+02:00`)
const ZONED: &[&str] = &[
    "%Y-%m-%d %H:%M:%S%.f%:z",
    "%Y-%m-%d %H:%M:%S%:z",
    "%Y-%m-%d %H:%M:%S%#z",
    // SQL Server datetimeoffset text: 2024-01-15 10:30:00.0000000 +02:00
    "%Y-%m-%d %H:%M:%S%.f %:z",
    "%Y-%m-%d %H:%M:%S %:z",
];

/// A reusable date parser that attempts to parse common date/datetime formats
/// and returns a standardized DateTime<Utc> value
pub struct DateParser;

impl DateParser {
    /// Parse a date or date and time, month first for numeric dates like `03/04/2024`.
    /// Returns `Value::Date` (UTC; dates are at midnight).
    pub fn try_parse(value: &str) -> Option<Value> {
        Self::try_parse_ordered(value, DateOrder::MonthFirst)
    }

    /// [`DateParser::try_parse`], reading ambiguous numeric dates in `order`
    pub fn try_parse_ordered(value: &str, order: DateOrder) -> Option<Value> {
        Self::parse(value.trim(), order).map(|(dt, _)| Value::Date(dt))
    }

    /// What `value` is as a date, if it is one
    pub fn classify(value: &str) -> Option<DateMatch> {
        let text = value.trim();
        let (_, layout_month) = Self::parse(text, DateOrder::MonthFirst)?;
        let (_, layout_day) = Self::parse(text, DateOrder::DayFirst)?;
        let has_time = layout_month.1;
        let fits = |order: DateOrder| {
            [layout_month, layout_day]
                .iter()
                .any(|(layout, _)| match layout {
                    Layout::Any => true,
                    Layout::MonthFirst => order == DateOrder::MonthFirst,
                    Layout::DayFirst => order == DateOrder::DayFirst,
                })
        };
        Some(DateMatch {
            has_time,
            month_first: fits(DateOrder::MonthFirst),
            day_first: fits(DateOrder::DayFirst),
        })
    }

    /// The parsed value and the layout it matched (with whether it has a time). Layouts of the
    /// preferred order are tried before the other order's.
    fn parse(text: &str, order: DateOrder) -> Option<(DateTime<Utc>, (Layout, bool))> {
        if text.is_empty() {
            return None;
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
            return Some((dt.with_timezone(&Utc), (Layout::Any, true)));
        }
        for format in ZONED {
            if let Ok(dt) = DateTime::parse_from_str(text, format) {
                return Some((dt.with_timezone(&Utc), (Layout::Any, true)));
            }
        }
        let preferred = match order {
            DateOrder::MonthFirst => Layout::MonthFirst,
            DateOrder::DayFirst => Layout::DayFirst,
        };
        // Unambiguous and preferred layouts first, then the other order
        for pass in [true, false] {
            let wanted = |layout: Layout| (layout == Layout::Any || layout == preferred) == pass;
            for (format, layout) in DATES.iter().filter(|(_, l)| wanted(*l)) {
                if let Ok(date) = NaiveDate::parse_from_str(text, format) {
                    let midnight = date.and_hms_opt(0, 0, 0)?;
                    return Some((Utc.from_utc_datetime(&midnight), (*layout, false)));
                }
            }
            for (format, layout) in DATETIMES.iter().filter(|(_, l)| wanted(*l)) {
                if let Ok(ndt) = NaiveDateTime::parse_from_str(text, format) {
                    return Some((Utc.from_utc_datetime(&ndt), (*layout, true)));
                }
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
