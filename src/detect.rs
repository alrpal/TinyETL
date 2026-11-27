//! Type detection for text: what a value in a CSV file (or a JSON string) can be, and what a
//! column of them is.
//!
//! A value is converted only when that keeps what it says:
//! - Numbers: `42`, ` 42 ` (padding is ignored), `-0.5`, `1.50`, `1.5e3`, and US thousands
//!   separators when every group has three digits (`1,234`, `12,345.67`). Codes with a leading
//!   zero or a leading `+` (`01826`, `+15551234`) are text, and so is a decimal with more digits
//!   than a Decimal holds exactly. Currency and percent signs make text.
//! - Booleans: `true` / `false` in any case.
//! - Dates and datetimes in the layouts of [`DateParser`]. A column is Date when no value has a
//!   time of day, and reads numeric dates day first when one of them only reads that way
//!   (`13/04/2024`); a column that needs both orders is text.
//! - Anything else is text. Only empty (or blank) values are NULL.

use crate::date_parser::{DateOrder, DateParser};
use crate::schema::{number_keeps_text, DataType, SchemaInferer, Value};
use rust_decimal::Decimal;

/// `text` as a number, if it is one: an Integer when it fits i64, else an exact Decimal
pub fn parse_number(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() || !number_keeps_text(text) {
        return None;
    }
    let numeric = |c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | ',' | 'e' | 'E' | '+');
    if !text.chars().all(numeric) || !text.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let plain = if text.contains(',') {
        ungroup(text)?
    } else {
        text.to_string()
    };
    if plain.contains(['e', 'E']) {
        let mantissa_digits = plain
            .split(['e', 'E'])
            .next()
            .is_some_and(|m| m.chars().any(|c| c.is_ascii_digit()));
        if !mantissa_digits {
            return None;
        }
        return Decimal::from_scientific(&plain).ok().map(Value::Decimal);
    }
    if plain.contains('+') {
        return None; // a + only belongs in an exponent
    }
    if let Ok(i) = plain.parse::<i64>() {
        return Some(Value::Integer(i));
    }
    Decimal::from_str_exact(&plain).ok().map(Value::Decimal)
}

/// `1,234,567.89` without its separators, when the digits are grouped in threes
fn ungroup(text: &str) -> Option<String> {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (unsigned, None),
    };
    let mut groups = whole.split(',');
    let first = groups.next()?;
    let first_ok = (1..=3).contains(&first.len()) && first.chars().all(|c| c.is_ascii_digit());
    let rest: Vec<&str> = groups.collect();
    if !first_ok
        || rest.is_empty()
        || !rest
            .iter()
            .all(|g| g.len() == 3 && g.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    if fraction.is_some_and(|f| f.is_empty() || !f.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let sign = if text.starts_with('-') { "-" } else { "" };
    let digits: String = whole.chars().filter(|c| *c != ',').collect();
    Some(match fraction {
        Some(f) => format!("{}{}.{}", sign, digits, f),
        None => format!("{}{}", sign, digits),
    })
}

/// `true` / `false` in any case
pub fn parse_boolean(text: &str) -> Option<bool> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("true") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// The values seen in one column, and the type they add up to
#[derive(Debug, Default)]
pub struct ColumnTally {
    types: Vec<DataType>,
    /// Some date in the column only reads month first / day first
    needs_month_first: bool,
    needs_day_first: bool,
}

impl ColumnTally {
    /// A text value from a file: numbers, booleans, dates, or text
    pub fn add_text(&mut self, text: &str) {
        let data_type = if text.trim().is_empty() {
            DataType::Null
        } else if let Some(number) = parse_number(text) {
            SchemaInferer::infer_type(&number)
        } else if parse_boolean(text).is_some() {
            DataType::Boolean
        } else {
            self.date_type(text).unwrap_or(DataType::String)
        };
        self.types.push(data_type);
    }

    /// A string whose source already says it's text (a JSON string): only dates are detected
    pub fn add_string(&mut self, text: &str) {
        let data_type = if text.trim().is_empty() {
            DataType::String
        } else {
            self.date_type(text).unwrap_or(DataType::String)
        };
        self.types.push(data_type);
    }

    /// A value that already has a type (a JSON number, a database value)
    pub fn add_value(&mut self, value: &Value) {
        match value {
            Value::String(s) => self.add_string(s),
            other => self.types.push(SchemaInferer::infer_type(other)),
        }
    }

    fn date_type(&mut self, text: &str) -> Option<DataType> {
        let date = DateParser::classify(text)?;
        self.needs_month_first |= !date.day_first;
        self.needs_day_first |= !date.month_first;
        Some(if date.has_time {
            DataType::DateTime
        } else {
            DataType::Date
        })
    }

    /// How numeric dates in this column are read
    pub fn date_order(&self) -> DateOrder {
        if self.needs_day_first && !self.needs_month_first {
            DateOrder::DayFirst
        } else {
            DateOrder::MonthFirst
        }
    }

    /// The column's type. Dates that need both orders (`13/04/2024` and `04/13/2024`) can't
    /// be read consistently, so the column is text.
    pub fn data_type(&self) -> DataType {
        let conflicting_dates = self.needs_month_first && self.needs_day_first;
        let types: Vec<DataType> = self
            .types
            .iter()
            .map(|t| match t {
                DataType::Date | DataType::DateTime if conflicting_dates => DataType::String,
                other => other.clone(),
            })
            .collect();
        SchemaInferer::resolve_column_type(&types).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(text: &str) -> Option<String> {
        parse_number(text).map(|v| match v {
            Value::Integer(i) => format!("int {}", i),
            Value::Decimal(d) => format!("dec {}", d),
            other => format!("{:?}", other),
        })
    }

    #[test]
    fn test_parse_number() {
        let cases = [
            ("42", Some("int 42")),
            (" -7 ", Some("int -7")),
            ("1.50", Some("dec 1.50")),
            ("1.5e3", Some("dec 1500")),
            ("2E-4", Some("dec 0.0002")),
            ("1,234", Some("int 1234")),
            ("-12,345.60", Some("dec -12345.60")),
            ("12345678901234567890", Some("dec 12345678901234567890")),
        ];
        for (text, expected) in cases {
            assert_eq!(number(text).as_deref(), expected, "{:?}", text);
        }
        for text in [
            "01826",
            "+15551234",
            "1,23",
            "1,2345",
            ",123",
            "1,234,",
            "$5",
            "45%",
            "NaN",
            "1_000",
            "e5",
            "1e",
            "1.2.3",
            "-",
            ".",
            "3.14159265358979323846264338327950",
        ] {
            assert_eq!(number(text), None, "{:?}", text);
        }
    }

    #[test]
    fn test_column_types() {
        let tally = |values: &[&str]| {
            let mut tally = ColumnTally::default();
            for v in values {
                tally.add_text(v);
            }
            (tally.data_type(), tally.date_order())
        };
        assert_eq!(tally(&["1", "2.5"]).0, DataType::Decimal);
        assert_eq!(tally(&["TRUE", "false"]).0, DataType::Boolean);
        assert_eq!(tally(&["2024-01-15", ""]).0, DataType::Date);
        assert_eq!(
            tally(&["2024-01-15", "2024-01-15 00:00"]).0,
            DataType::DateTime
        );
        assert_eq!(
            tally(&["03/04/2024", "13/04/2024"]),
            (DataType::Date, DateOrder::DayFirst)
        );
        assert_eq!(
            tally(&["03/04/2024", "05/06/2024"]),
            (DataType::Date, DateOrder::MonthFirst)
        );
        assert_eq!(tally(&["13/04/2024", "04/13/2024"]).0, DataType::String);
        assert_eq!(tally(&["01826", "02134"]).0, DataType::String);
    }
}
