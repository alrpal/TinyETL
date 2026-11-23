//! Endpoint options (`--source-option` / `--target-option KEY=VALUE`, YAML `options:`).
//!
//! Each protocol and file format declares the keys it reads in an `OPTIONS` list next to the
//! code that reads them. [`check`] runs once per endpoint, before anything is created, and
//! rejects keys that neither the endpoint's protocol nor its format accepts, so a typo such
//! as `auth.baerer` fails instead of being ignored.

use std::collections::HashMap;

use crate::{connectors, protocols, Result, TinyEtlError};

/// Which end of the transfer an endpoint is; used in error messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Source,
    Target,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Source => "source",
            Role::Target => "target",
        }
    }
}

/// Database URL schemes. Except for Snowflake, database endpoints take no options: their
/// settings go in the connection URL.
const DATABASE_SCHEMES: &[&str] = &[
    "duckdb",
    "sqlite",
    "postgres",
    "postgresql",
    "mysql",
    "mssql",
    "sqlserver",
    "odbc",
];

/// The options an endpoint accepts: those of its protocol plus those of its file format.
/// Keys ending in `*` accept any suffix (`header.*`).
pub fn accepted(location: &str, source_type: Option<&str>) -> Vec<&'static str> {
    let scheme = location.split_once("://").map(|(scheme, _)| scheme);
    let mut keys: Vec<&'static str> = Vec::new();
    match scheme {
        Some("snowflake") => return connectors::snowflake::OPTIONS.to_vec(),
        Some(s) if DATABASE_SCHEMES.contains(&s) => return keys,
        Some("http" | "https") => keys.extend(protocols::http::OPTIONS),
        Some("s3") => keys.extend(protocols::s3::OPTIONS),
        _ => {}
    }
    if let Some(format) = format_of(location, source_type) {
        keys.extend(format_options(&format));
    }
    keys
}

/// The file format of a location: `--source-type` if given, else the file extension,
/// without a compression suffix (`data.csv.gz` → `csv`)
fn format_of(location: &str, source_type: Option<&str>) -> Option<String> {
    let format = match source_type {
        Some(t) => t.to_lowercase(),
        None => {
            // Drop a URL's query and the `#table` / `#sheet` suffix
            let path = location.split(['?', '#']).next().unwrap_or(location);
            protocols::extension_from_path(path)?
        }
    };
    let inner = connectors::compressed::Codec::from_path(&format)
        .map_or(format.as_str(), |(_, inner)| inner);
    Some(inner.to_string())
}

fn format_options(format: &str) -> &'static [&'static str] {
    match format {
        "csv" | "tsv" => connectors::csv::OPTIONS,
        _ => &[],
    }
}

/// Reject any option the endpoint doesn't accept
pub fn check(
    role: Role,
    location: &str,
    source_type: Option<&str>,
    options: &HashMap<String, String>,
) -> Result<()> {
    let accepted = accepted(location, source_type);
    let mut unknown: Vec<&str> = options
        .keys()
        .map(String::as_str)
        .filter(|key| !accepted.iter().any(|a| matches(a, key)))
        .collect();
    unknown.sort_unstable();
    let Some(key) = unknown.first() else {
        return Ok(());
    };

    let mut message = format!("Unknown {} option '{}'.", role.name(), key);
    if accepted.is_empty() {
        message.push_str(&format!(" This {} takes no options", role.name()));
        if location
            .split_once("://")
            .is_some_and(|(s, _)| DATABASE_SCHEMES.contains(&s))
        {
            message.push_str("; put database settings in the connection URL");
        }
        message.push('.');
    } else {
        if let Some(suggestion) = closest(key, &accepted) {
            message.push_str(&format!(" Did you mean '{}'?", suggestion));
        }
        message.push_str(&format!(
            " Options for this {}: {}",
            role.name(),
            accepted.join(", ")
        ));
    }
    Err(TinyEtlError::Configuration(message))
}

fn matches(accepted: &str, key: &str) -> bool {
    match accepted.strip_suffix('*') {
        Some(prefix) => key.len() > prefix.len() && key.starts_with(prefix),
        None => accepted == key,
    }
}

/// The accepted key nearest to a misspelled one, if it's close enough to be a typo
fn closest<'a>(key: &str, accepted: &[&'a str]) -> Option<&'a str> {
    accepted
        .iter()
        .map(|a| (edit_distance(key, a.trim_end_matches('*')), *a))
        .filter(|(distance, _)| *distance <= 2.max(key.len() / 4))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, a)| a)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != *cb);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn err(location: &str, pairs: &[(&str, &str)]) -> String {
        check(Role::Source, location, None, &opts(pairs))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn test_accepted_by_location() {
        assert_eq!(accepted("data.csv", None), vec!["delimiter"]);
        assert_eq!(accepted("data.tsv.gz", None), vec!["delimiter"]);
        assert!(accepted("data.parquet", None).is_empty());
        assert!(accepted("out.db#t", None).is_empty());
        assert!(accepted("postgres://u:p@h/db#t", None).is_empty());
        assert!(accepted("https://x.org/a.json", None).contains(&"auth.bearer"));
        assert!(accepted("https://x.org/a.csv?x=1", None).contains(&"delimiter"));
        assert!(accepted("https://x.org/export", Some("csv")).contains(&"delimiter"));
        let s3 = accepted("s3://bucket/a.csv.zst", None);
        assert!(s3.contains(&"region") && s3.contains(&"delimiter"));
        assert!(accepted("snowflake://u@acct/db/s#t", None).contains(&"warehouse"));
    }

    #[test]
    fn test_known_options_pass() {
        let ok = |location: &str, pairs: &[(&str, &str)]| {
            check(Role::Target, location, None, &opts(pairs)).unwrap()
        };
        ok("data.csv", &[]);
        ok("postgres://u:p@h/db#t", &[]);
        ok("data.csv", &[("delimiter", ";")]);
        ok(
            "https://x.org/a.csv",
            &[
                ("header.X-Api-Key", "k"),
                ("auth.bearer", "t"),
                ("delimiter", "|"),
            ],
        );
        ok("s3://b/a.parquet", &[("region", "eu-west-1")]);
    }

    #[test]
    fn test_unknown_options_fail() {
        let e = err("https://x.org/a.json", &[("auth.baerer", "t")]);
        assert!(e.contains("Unknown source option 'auth.baerer'"), "{}", e);
        assert!(e.contains("Did you mean 'auth.bearer'?"), "{}", e);

        let e = err("s3://b/a.csv", &[("regoin", "x")]);
        assert!(e.contains("Did you mean 'region'?"), "{}", e);

        // An option for another format
        let e = err("data.parquet", &[("delimiter", ";")]);
        assert!(e.contains("This source takes no options."), "{}", e);

        let e = err("postgres://u:p@h/db#t", &[("sslmode", "require")]);
        assert!(
            e.contains("put database settings in the connection URL"),
            "{}",
            e
        );
        assert!(!e.contains("u:p"), "{}", e);

        // Far from anything: no suggestion
        let e = err("data.csv", &[("compression", "gzip")]);
        assert!(!e.contains("Did you mean"), "{}", e);
        assert!(e.contains("Options for this source: delimiter"), "{}", e);

        // `header.` alone isn't a header
        assert!(check(
            Role::Source,
            "http://x/a.csv",
            None,
            &opts(&[("header.", "x")])
        )
        .is_err());
    }

    #[test]
    fn test_edit_distance() {
        assert_eq!(edit_distance("delimter", "delimiter"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }
}
