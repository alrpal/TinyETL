//! ODBC source and target.
//!
//! Works with any ODBC driver: identifier quoting comes from the driver (`SQLGetInfo`), rows are
//! streamed from a single `SELECT *` instead of paginated, transactions use the standard ODBC
//! autocommit switch, and inserts use ODBC parameter arrays. The only database-specific part is
//! the column types used for `CREATE TABLE`, chosen from the DBMS name the driver reports, with an
//! ANSI SQL fallback for databases not listed in [`DbKind`].

use async_trait::async_trait;
use odbc_api::{
    handles::ColumnDescription, BindParamDesc, Connection, ConnectionOptions, Cursor, OwnedCursor,
    ResultSetMetadata,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Mutex;
use tracing::{debug, info};

use crate::{
    connectors::{Source, Target},
    date_parser::DateParser,
    schema::{Column, DataType, Row, Schema, Value},
    Result, TinyEtlError,
};

/// Directories where Homebrew keeps `odbcinst.ini` (Apple silicon, then Intel).
const HOMEBREW_ODBC_DIRS: [&str; 2] = ["/opt/homebrew/etc", "/usr/local/etc"];

/// Point the bundled unixODBC driver manager at Homebrew's driver registrations on macOS.
///
/// TinyETL compiles unixODBC into the binary, and that copy reads `odbcinst.ini` / `odbc.ini`
/// from `/etc`. Homebrew-installed drivers (e.g. `msodbcsql18`) register themselves in
/// `$(brew --prefix)/etc` instead, so without this, `Driver={...}` names would not resolve.
/// If `ODBCSYSINI` is already set or `/etc/odbcinst.ini` exists, nothing is changed.
///
/// Must be called before any threads are started, because it modifies the process environment.
pub fn configure_driver_manager() {
    if cfg!(target_os = "macos") {
        let already_set = std::env::var_os("ODBCSYSINI").is_some();
        if let Some(dir) =
            homebrew_odbc_sysini(already_set, |path| std::path::Path::new(path).exists())
        {
            std::env::set_var("ODBCSYSINI", dir);
        }
    }
    #[cfg(unix)]
    use_utf8_ctype_locale();
}

/// Give the process a UTF-8 `LC_CTYPE` locale.
///
/// TinyETL exchanges text with ODBC drivers as UTF-16. Some drivers (psqlODBC on Linux, for
/// example) convert it with the C library's `wcstombs`, which follows `LC_CTYPE`. A program
/// starts in the "C" locale, where anything outside ASCII fails to convert. So adopt the user's
/// locale if it is UTF-8 (normal on desktops and servers), and otherwise fall back to
/// `C.UTF-8` (no `LANG` at all is common in containers and cron jobs). Only character handling
/// changes; number formatting (`LC_NUMERIC`) stays "C".
///
/// The fallback is also exported as `LC_CTYPE`, because some drivers re-read the locale from the
/// environment with `setlocale(LC_CTYPE, "")`. An explicit `LC_ALL` is left alone.
///
/// Must be called before any threads are started: `setlocale` and `set_var` are not thread-safe.
#[cfg(unix)]
fn use_utf8_ctype_locale() {
    fn codeset_is_utf8() -> bool {
        // SAFETY: nl_langinfo returns a pointer to a static, NUL-terminated string
        let codeset = unsafe { std::ffi::CStr::from_ptr(libc::nl_langinfo(libc::CODESET)) };
        let codeset = codeset.to_string_lossy().to_ascii_lowercase();
        codeset == "utf-8" || codeset == "utf8"
    }
    for (index, locale) in [c"", c"C.UTF-8", c"en_US.UTF-8"].into_iter().enumerate() {
        // SAFETY: called before other threads exist; the argument is a valid C string
        let applied = unsafe { !libc::setlocale(libc::LC_CTYPE, locale.as_ptr()).is_null() };
        if applied && codeset_is_utf8() {
            if index > 0 && std::env::var_os("LC_ALL").is_none() {
                std::env::set_var("LC_CTYPE", locale.to_str().unwrap_or("C.UTF-8"));
            }
            return;
        }
    }
    // No UTF-8 locale available: leave the C locale in place
    unsafe { libc::setlocale(libc::LC_CTYPE, c"C".as_ptr()) };
}

/// On Linux, make the connection string's `Driver=` an absolute path when the driver is only
/// known by its file name.
///
/// Debian and Ubuntu register drivers by bare file name (`Driver=psqlodbcw.so` in
/// `/etc/odbcinst.ini`) and install them in `/usr/lib/<arch>/odbc`. Their own unixODBC package
/// knows to look there, but the bundled copy opens drivers with plain `dlopen()`, which doesn't.
/// So `Driver={PostgreSQL Unicode}` and `Driver=psqlodbcw.so` are rewritten to the full path of
/// the library. Absolute paths, DSNs and anything that can't be resolved are left unchanged.
fn resolve_driver_path(connection_string: &str) -> String {
    if !cfg!(target_os = "linux") {
        return connection_string.to_string();
    }
    let dirs = linux_odbc_driver_dirs(|path| std::path::Path::new(path).is_dir());
    let resolved = rewrite_driver_attribute(connection_string, registered_driver_library, |file| {
        dirs.iter()
            .map(|dir| format!("{}/{}", dir, file))
            .find(|path| std::path::Path::new(path).is_file())
    });
    match resolved {
        Some((library, rewritten)) => {
            debug!("ODBC: using driver library {}", library);
            rewritten
        }
        None => connection_string.to_string(),
    }
}

/// Replace the `Driver` attribute of `connection_string` with an absolute library path, if its
/// value (or the library registered under that driver name) is a bare file name that
/// `find_library` can locate. Returns the library path and the new connection string.
fn rewrite_driver_attribute(
    connection_string: &str,
    registered_library: impl Fn(&str) -> Option<String>,
    find_library: impl Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    let mut attributes = split_attributes(connection_string);
    for attribute in attributes.iter_mut() {
        let Some((key, value)) = attribute.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("driver") {
            continue;
        }
        let value = value.trim();
        let name = value
            .strip_prefix('{')
            .and_then(|v| v.strip_suffix('}'))
            .unwrap_or(value);
        let is_file_name = name.ends_with(".so") || name.contains(".so.");
        let library = if is_file_name {
            name.to_string()
        } else {
            registered_library(name)?
        };
        if library.contains('/') {
            return None; // already a path
        }
        let path = find_library(&library)?;
        *attribute = format!("{}={{{}}}", key, path);
        return Some((path, attributes.join(";")));
    }
    None
}

/// Split an ODBC connection string on `;`, keeping `{...}` values (which may contain `;`) intact.
fn split_attributes(connection_string: &str) -> Vec<String> {
    let mut attributes = Vec::new();
    let mut current = String::new();
    let mut in_braces = false;
    for c in connection_string.chars() {
        match c {
            '{' => in_braces = true,
            '}' => in_braces = false,
            ';' if !in_braces => {
                attributes.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    attributes.push(current);
    attributes
}

/// The `Driver` library registered for a driver name in `odbcinst.ini`, looked up the way unixODBC
/// does: `$ODBCSYSINI/$ODBCINSTINI` (default `/etc/odbcinst.ini`), then `~/.odbcinst.ini`.
fn registered_driver_library(driver_name: &str) -> Option<String> {
    let dir = std::env::var("ODBCSYSINI").unwrap_or_else(|_| "/etc".to_string());
    let file = std::env::var("ODBCINSTINI").unwrap_or_else(|_| "odbcinst.ini".to_string());
    let mut candidates = vec![format!("{}/{}", dir, file)];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(format!("{}/.odbcinst.ini", home.to_string_lossy()));
    }
    candidates.iter().find_map(|path| {
        let contents = std::fs::read_to_string(path).ok()?;
        ini_value(&contents, driver_name, "Driver")
    })
}

/// `key` in `[section]` of an INI file (names compared case-insensitively).
fn ini_value(contents: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with(';') || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim().eq_ignore_ascii_case(section);
        } else if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// Directories Linux distributions install ODBC drivers into, that exist on this machine.
fn linux_odbc_driver_dirs(is_dir: impl Fn(&str) -> bool) -> Vec<String> {
    // Debian/Ubuntu multiarch directory for this CPU, then generic locations
    let multiarch = match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "arm" => Some("arm-linux-gnueabihf"),
        "x86" => Some("i386-linux-gnu"),
        "powerpc64" => Some("powerpc64le-linux-gnu"),
        "s390x" => Some("s390x-linux-gnu"),
        _ => None,
    };
    multiarch
        .map(|triplet| format!("/usr/lib/{}/odbc", triplet))
        .into_iter()
        .chain(["/usr/lib/odbc", "/usr/lib64/odbc", "/usr/local/lib/odbc"].map(String::from))
        .filter(|dir| is_dir(dir))
        .collect()
}

/// Decide which directory to use as `ODBCSYSINI`, if any. `exists` checks a file path.
fn homebrew_odbc_sysini(already_set: bool, exists: impl Fn(&str) -> bool) -> Option<&'static str> {
    if already_set || exists("/etc/odbcinst.ini") {
        return None;
    }
    HOMEBREW_ODBC_DIRS
        .into_iter()
        .find(|dir| exists(&format!("{}/odbcinst.ini", dir)))
}

/// Rows per ODBC parameter array when inserting.
const INSERT_CHUNK_ROWS: usize = 1000;

/// Database families that get their own `CREATE TABLE` column types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbKind {
    SqlServer,
    Postgres,
    MySql,
    Sqlite,
    /// Anything else: ANSI SQL types.
    Other,
}

impl DbKind {
    /// Classify the name a driver reports for `SQL_DBMS_NAME`.
    fn from_dbms_name(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        if name.contains("sql server") {
            DbKind::SqlServer
        } else if name.contains("postgres") {
            DbKind::Postgres
        } else if name.contains("mysql") || name.contains("mariadb") {
            DbKind::MySql
        } else if name.contains("sqlite") {
            DbKind::Sqlite
        } else {
            DbKind::Other
        }
    }
}

/// How to write SQL for the database behind a connection.
#[derive(Debug, Clone)]
struct Dialect {
    kind: DbKind,
    /// Identifier quote character; `None` if the driver says the database has none.
    quote: Option<char>,
}

impl Dialect {
    fn new(dbms_name: &str, quote: Option<char>) -> Self {
        Self {
            kind: DbKind::from_dbms_name(dbms_name),
            quote,
        }
    }

    /// Ask the driver which database it talks to and how identifiers are quoted.
    fn detect(conn: &Connection<'_>) -> Self {
        let dbms_name = conn.database_management_system_name().unwrap_or_default();
        // SQL-92 drivers report `"`; fall back to it if the driver can't say.
        let quote = conn.identifier_quote_char().unwrap_or(Some('"'));
        let dialect = Self::new(&dbms_name, quote);
        info!(
            "ODBC: connected to {} ({})",
            if dbms_name.is_empty() {
                "unknown DBMS"
            } else {
                &dbms_name
            },
            if dialect.kind == DbKind::Other {
                "using ANSI SQL column types".to_string()
            } else {
                format!("using {:?} column types", dialect.kind)
            }
        );
        dialect
    }

    /// Quote one identifier, doubling any quote character inside it.
    fn quote_ident(&self, ident: &str) -> String {
        match self.quote {
            Some(q) => {
                let escaped = ident.replace(q, &format!("{q}{q}"));
                format!("{q}{escaped}{q}")
            }
            None => ident.to_string(),
        }
    }

    /// Quote a possibly qualified name (`table`, `schema.table`, `catalog.schema.table`).
    fn qualified(&self, name: &str) -> String {
        name.split('.')
            .map(|part| self.quote_ident(part))
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Column type for `CREATE TABLE`. Mirrors the native connectors where TinyETL has one.
    fn column_type(&self, data_type: &DataType) -> &'static str {
        use DataType::*;
        match self.kind {
            DbKind::SqlServer => match data_type {
                String | Json | Null => "NVARCHAR(MAX)",
                Integer => "BIGINT",
                Decimal => "DECIMAL(18,6)",
                Boolean => "BIT",
                Date => "DATE",
                DateTime => "DATETIME2",
            },
            DbKind::Postgres => match data_type {
                // JSON stays TEXT: psqlODBC sends parameters as varchar, which Postgres won't
                // implicitly cast to JSONB.
                String | Json | Null => "TEXT",
                Integer => "BIGINT",
                Decimal => "DECIMAL",
                Boolean => "BOOLEAN",
                Date => "DATE",
                DateTime => "TIMESTAMP",
            },
            DbKind::MySql => match data_type {
                String | Null => "TEXT",
                Json => "JSON",
                Integer => "BIGINT",
                Decimal => "DECIMAL(65,30)",
                Boolean => "BOOLEAN",
                Date => "DATE",
                DateTime => "DATETIME",
            },
            DbKind::Sqlite => match data_type {
                String | Json | Null | Date | DateTime => "TEXT",
                Integer | Boolean => "INTEGER",
                Decimal => "REAL",
            },
            DbKind::Other => match data_type {
                String | Json | Null => "VARCHAR(4000)",
                Integer => "BIGINT",
                Decimal => "DECIMAL(38,10)",
                Boolean => "BOOLEAN",
                Date => "DATE",
                DateTime => "TIMESTAMP",
            },
        }
    }
}

/// Open a connection through the process-wide ODBC environment.
fn connect(connection_string: &str) -> Result<Connection<'static>> {
    let env = odbc_api::environment().map_err(|e| {
        TinyEtlError::Connection(format!("Failed to create ODBC environment: {}", e))
    })?;
    let connection_string = terminated(&resolve_driver_path(connection_string));
    env.connect_with_connection_string(&connection_string, ConnectionOptions::default())
        .map_err(|e| {
            TinyEtlError::Connection(format!("Failed to connect to ODBC data source: {}", e))
        })
}

/// End the connection string with `;`. The string is passed with its length and no NUL, and
/// some drivers read past the end of the last value: MariaDB Connector/ODBC 3.1 then fails to
/// log in when the password comes last ("Access denied", depending on what's in memory).
fn terminated(connection_string: &str) -> String {
    let mut connection_string = connection_string.trim_end().to_string();
    if !connection_string.is_empty() && !connection_string.ends_with(';') {
        connection_string.push(';');
    }
    connection_string
}

/// Split `odbc://<connection string>#<table>` into its two parts.
fn parse_connection_string(connection_string: &str, role: &str) -> Result<(String, String)> {
    let connection_string = connection_string.trim_start_matches("odbc://");
    match connection_string.rsplit_once('#') {
        Some((conn_part, table_part)) if !table_part.is_empty() => {
            Ok((conn_part.to_string(), table_part.to_string()))
        }
        _ => Err(TinyEtlError::Configuration(format!(
            "ODBC {} requires table specification: odbc://connection_string#table",
            role
        ))),
    }
}

fn map_odbc_type_to_datatype(col_desc: &ColumnDescription) -> DataType {
    use odbc_api::DataType as OdbcDataType;

    match &col_desc.data_type {
        OdbcDataType::Decimal { .. } | OdbcDataType::Numeric { .. } => DataType::Decimal,
        OdbcDataType::SmallInt
        | OdbcDataType::Integer
        | OdbcDataType::TinyInt
        | OdbcDataType::BigInt => DataType::Integer,
        OdbcDataType::Real | OdbcDataType::Float { .. } | OdbcDataType::Double => DataType::Decimal,
        OdbcDataType::Bit => DataType::Boolean,
        OdbcDataType::Date => DataType::Date,
        OdbcDataType::Timestamp { .. } => DataType::DateTime,
        // Text, binary, time-of-day and driver-specific types are passed through as text
        _ => DataType::String,
    }
}

/// How a column's values are read and converted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadAs {
    /// As text, parsed as the column's type ([`text_to_value`])
    Text,
    /// As bytes, written `0x` + lowercase hex (reading binary as text fails in some drivers)
    Binary,
    /// As bytes, a boolean: MySQL's bit(1), which its driver sends as the raw byte (a false one
    /// as an empty string)
    BitBytes,
    /// JSON text, parsed
    Json,
    /// Text in lowercase (UUIDs, which drivers report in uppercase)
    Lowercase,
    /// Money as text (`$1,234.56`, `12.3400`): the digits; `normalize` drops trailing zeros
    Money { normalize: bool },
    /// A float as text (`0.10000000000000001`): its shortest decimal (`0.1`)
    Float,
    /// A time of day (`13:45:30.0000000`), without trailing fractional zeros
    Time,
    /// A PostgreSQL array literal (`{1,2}`, `{"a b",NULL}`), as a JSON array
    PgArray,
}

/// The TinyETL type and the way to read a column, from its ODBC type and the database's own
/// type name (`SQL_DESC_TYPE_NAME`: `json`, `uuid`, `money`, `_int4`, ...)
fn column_reading(
    col_desc: &ColumnDescription,
    type_name: &str,
    unsigned: bool,
    kind: DbKind,
) -> (DataType, ReadAs) {
    use odbc_api::DataType as OdbcDataType;
    let name = type_name.to_ascii_lowercase();
    match (&col_desc.data_type, name.as_str()) {
        // MySQL's booleans (declared types from the catalog)
        (OdbcDataType::Bit, _) if kind == DbKind::MySql => (DataType::Boolean, ReadAs::BitBytes),
        (_, "tinyint(1)") => (DataType::Boolean, ReadAs::Text),
        // Past i64::MAX: an exact decimal
        (OdbcDataType::BigInt, _) if unsigned => (DataType::Decimal, ReadAs::Text),
        (_, name) if name.starts_with("bigint") && name.contains("unsigned") => {
            (DataType::Decimal, ReadAs::Text)
        }
        (
            OdbcDataType::Binary { .. }
            | OdbcDataType::Varbinary { .. }
            | OdbcDataType::LongVarbinary { .. },
            _,
        )
        | (_, "bytea" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "image") => {
            (DataType::String, ReadAs::Binary)
        }
        (_, "json" | "jsonb") => (DataType::Json, ReadAs::Json),
        (_, "uuid" | "uniqueidentifier") => (DataType::String, ReadAs::Lowercase),
        (_, "money" | "smallmoney") => (
            DataType::Decimal,
            ReadAs::Money {
                normalize: kind == DbKind::SqlServer,
            },
        ),
        (_, "datetimeoffset" | "timestamptz") => (DataType::DateTime, ReadAs::Text),
        (_, "bool" | "boolean") => (DataType::Boolean, ReadAs::Text),
        // PostgreSQL arrays: `text[]` from the catalog, `_text` from some drivers
        (_, name)
            if kind == DbKind::Postgres && (name.starts_with('_') || name.ends_with("[]")) =>
        {
            (DataType::Json, ReadAs::PgArray)
        }
        (OdbcDataType::Time { .. }, _) | (_, "time" | "time2" | "timetz") => {
            (DataType::String, ReadAs::Time)
        }
        (OdbcDataType::Real | OdbcDataType::Float { .. } | OdbcDataType::Double, _) => {
            (DataType::Decimal, ReadAs::Float)
        }
        _ => (map_odbc_type_to_datatype(col_desc), ReadAs::Text),
    }
}

/// The declared type of each column of `table` (lowercase, e.g. `jsonb`, `text[]`,
/// `tinyint(1)`, `bigint unsigned`), from the database's catalog. Drivers report some types
/// only vaguely (psqlODBC says "unknown" for jsonb and arrays; MariaDB's driver says "tinyint"
/// for tinyint(1)). Empty for databases without a lookup here, or if it fails.
fn declared_types(
    conn: &Connection<'_>,
    kind: DbKind,
    table: &str,
) -> std::collections::HashMap<String, String> {
    let literal = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let unquote = |s: &str| s.trim_matches(|c| c == '"' || c == '`').to_string();
    let (schema, name) = match table.rsplit_once('.') {
        Some((schema, name)) => (Some(schema), name),
        None => (None, table),
    };
    let query = match kind {
        DbKind::Postgres => format!(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod) FROM pg_attribute a \
             WHERE a.attrelid = to_regclass({}) AND a.attnum > 0 AND NOT a.attisdropped",
            literal(table)
        ),
        DbKind::MySql => format!(
            "SELECT COLUMN_NAME, COLUMN_TYPE FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {}",
            schema.map_or("DATABASE()".to_string(), |s| literal(&unquote(s))),
            literal(&unquote(name))
        ),
        _ => return Default::default(),
    };
    let mut types = std::collections::HashMap::new();
    let Ok(Some(mut cursor)) = conn.execute(&query, (), None) else {
        debug!("ODBC: no declared types for {}", table);
        return types;
    };
    let (mut column, mut declared) = (Vec::new(), Vec::new());
    while let Ok(Some(mut row)) = cursor.next_row() {
        if row.get_wide_text(1, &mut column).unwrap_or(false)
            && row.get_wide_text(2, &mut declared).unwrap_or(false)
        {
            types.insert(
                String::from_utf16_lossy(&column),
                String::from_utf16_lossy(&declared).to_ascii_lowercase(),
            );
        }
    }
    types
}

/// The database's name for a result column's type (`SQL_DESC_TYPE_NAME`), or "" if the driver
/// doesn't say
fn col_type_name(cursor: &mut impl odbc_api::ResultSetMetadata, column: u16) -> String {
    use odbc_api::handles::Statement;
    let statement = cursor.as_stmt_ref();
    let mut buffer = vec![0u8; 256];
    let mut length: i16 = 0;
    // SAFETY: the buffer outlives the call, and its length is passed
    let ret = unsafe {
        odbc_api::sys::SQLColAttribute(
            statement.as_sys(),
            column,
            odbc_api::sys::Desc::TypeName,
            buffer.as_mut_ptr() as odbc_api::sys::Pointer,
            buffer.len() as i16,
            &mut length,
            std::ptr::null_mut(),
        )
    };
    if ret != odbc_api::sys::SqlReturn::SUCCESS
        && ret != odbc_api::sys::SqlReturn::SUCCESS_WITH_INFO
    {
        return String::new();
    }
    let length = (length.max(0) as usize).min(buffer.len());
    String::from_utf8_lossy(&buffer[..length])
        .trim_end_matches('\0')
        .to_string()
}

/// A value read as text, converted the way `read_as` says
fn read_text(text: String, data_type: &DataType, read_as: ReadAs) -> Value {
    match read_as {
        ReadAs::Json => match serde_json::from_str(&text) {
            Ok(json) => Value::Json(json),
            Err(_) => Value::String(text),
        },
        ReadAs::Lowercase => Value::String(text.to_ascii_lowercase()),
        ReadAs::Money { normalize } => {
            let negative = text.contains('-') || (text.starts_with('(') && text.ends_with(')'));
            let digits: String = text
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            match Decimal::from_str(&digits) {
                Ok(d) => {
                    let d = if negative { -d } else { d };
                    Value::Decimal(if normalize { d.normalize() } else { d })
                }
                Err(_) => Value::String(text),
            }
        }
        ReadAs::Float => match text.trim().parse::<f64>() {
            Ok(f) if f.is_finite() => match Decimal::from_str(&f.to_string())
                .or_else(|_| Decimal::from_scientific(&format!("{:e}", f)))
            {
                Ok(d) => Value::Decimal(d),
                Err(_) => Value::String(text),
            },
            _ => Value::String(text),
        },
        ReadAs::Time => Value::String(match text.split_once('.') {
            Some((whole, fraction)) => {
                let fraction = fraction.trim_end_matches('0');
                if fraction.is_empty() {
                    whole.to_string()
                } else {
                    format!("{}.{}", whole, fraction)
                }
            }
            None => text,
        }),
        ReadAs::PgArray => match pg_array_to_json(&text) {
            Some(json) => Value::Json(json),
            None => Value::String(text),
        },
        ReadAs::Text | ReadAs::Binary | ReadAs::BitBytes => text_to_value(text, data_type),
    }
}

/// A PostgreSQL array literal as JSON: `{1,2}` → `[1,2]`, `{{1,2},{3,4}}` → nested,
/// `{"a b",NULL}` → `["a b",null]`. Unquoted elements that are numbers or `t`/`f` become
/// numbers and booleans.
fn pg_array_to_json(text: &str) -> Option<serde_json::Value> {
    fn parse(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<serde_json::Value> {
        if chars.next()? != '{' {
            return None;
        }
        let mut items = Vec::new();
        loop {
            match chars.peek()? {
                '}' => {
                    chars.next();
                    return Some(serde_json::Value::Array(items));
                }
                ',' => {
                    chars.next();
                }
                '{' => items.push(parse(chars)?),
                '"' => {
                    chars.next();
                    let mut item = String::new();
                    loop {
                        match chars.next()? {
                            '\\' => item.push(chars.next()?),
                            '"' => break,
                            c => item.push(c),
                        }
                    }
                    items.push(serde_json::Value::String(item));
                }
                _ => {
                    let mut item = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == ',' || c == '}' {
                            break;
                        }
                        item.push(c);
                        chars.next();
                    }
                    items.push(match item.as_str() {
                        "NULL" => serde_json::Value::Null,
                        "t" => serde_json::Value::Bool(true),
                        "f" => serde_json::Value::Bool(false),
                        _ => serde_json::from_str::<serde_json::Number>(&item)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::String(item)),
                    });
                }
            }
        }
    }
    let mut chars = text.trim().chars().peekable();
    let json = parse(&mut chars)?;
    chars.next().is_none().then_some(json)
}

/// Convert a value read as text into the type of its column. If the text doesn't parse as that
/// type, it is kept as a string so no data is lost.
fn text_to_value(text: String, data_type: &DataType) -> Value {
    let trimmed = text.trim();
    let parsed = match data_type {
        DataType::Integer => trimmed.parse::<i64>().ok().map(Value::Integer),
        DataType::Decimal => Decimal::from_str(trimmed)
            .or_else(|_| Decimal::from_scientific(trimmed))
            .ok()
            .map(Value::Decimal),
        DataType::Boolean => match trimmed.to_ascii_lowercase().as_str() {
            // MariaDB's driver sends bit(1) as the byte itself
            "1" | "true" | "t" | "yes" | "y" | "\u{1}" => Some(Value::Boolean(true)),
            "0" | "false" | "f" | "no" | "n" | "\u{0}" => Some(Value::Boolean(false)),
            _ => None,
        },
        DataType::Date | DataType::DateTime => DateParser::try_parse(trimmed),
        _ => None,
    };
    parsed.unwrap_or(Value::String(text))
}

/// Text sent to the database for a value, or `None` for NULL.
fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Decimal(d) => Some(d.to_string()),
        Value::Boolean(b) => Some(if *b { "1" } else { "0" }.to_string()),
        Value::Date(dt) => Some(dt.format("%Y-%m-%d %H:%M:%S").to_string()),
        Value::Json(j) => Some(serde_json::to_string(j).unwrap_or_else(|_| "{}".to_string())),
    }
}

/// The open result set of a source, with the type of each column.
struct OpenCursor {
    cursor: OwnedCursor<Connection<'static>>,
    columns: Vec<(String, DataType, ReadAs)>,
    /// No row has been fetched yet, so the cursor is still at the start.
    fresh: bool,
}

/// ODBC source: streams every row of one table from a single `SELECT *`.
pub struct OdbcSource {
    connection_string: String,
    table_name: String,
    /// `--source-query`: read this query's result instead of the table
    query: Option<String>,
    /// Behind a `Mutex` because `Source` must be `Sync`; ODBC handles are only `Send`.
    open: Mutex<Option<OpenCursor>>,
    schema: Option<Schema>,
    exhausted: bool,
}

impl OdbcSource {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (connection_string, table_name) = parse_connection_string(connection_string, "source")?;
        Ok(Self {
            connection_string,
            table_name,
            query: None,
            open: Mutex::new(None),
            schema: None,
            exhausted: false,
        })
    }

    /// Read the result of `query`; `connection_string` has no `#table`
    pub fn with_query(connection_string: &str, query: &str) -> Result<Self> {
        Ok(Self {
            connection_string: connection_string.trim_start_matches("odbc://").to_string(),
            table_name: String::new(),
            query: Some(query.to_string()),
            open: Mutex::new(None),
            schema: None,
            exhausted: false,
        })
    }

    /// What is read, for error messages
    fn what(&self) -> String {
        match &self.query {
            Some(_) => "the query".to_string(),
            None => format!("table '{}'", self.table_name),
        }
    }

    /// Connect and run `SELECT * FROM <table>` (or the query), replacing any cursor already open.
    fn open_cursor(&mut self) -> Result<()> {
        let conn = connect(&self.connection_string)?;
        let dialect = Dialect::detect(&conn);
        let declared = match &self.query {
            Some(_) => Default::default(),
            None => declared_types(&conn, dialect.kind, &self.table_name),
        };
        let query = match &self.query {
            Some(query) => query.clone(),
            None => format!("SELECT * FROM {}", dialect.qualified(&self.table_name)),
        };
        debug!("ODBC source query: {}", query);

        let what = self.what();
        let mut cursor = conn
            .into_cursor(&query, (), None)
            .map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to read {}: {}", what, e.error))
            })?
            .ok_or_else(|| {
                TinyEtlError::DataTransfer(format!("Reading {} returned no result set", what))
            })?;

        let num_cols = cursor.num_result_cols().map_err(|e| {
            TinyEtlError::DataTransfer(format!("Failed to get column count: {}", e))
        })?;
        let mut columns = Vec::with_capacity(num_cols as usize);
        let mut schema_columns = Vec::with_capacity(num_cols as usize);
        for i in 1..=num_cols as u16 {
            let mut col_desc = ColumnDescription::default();
            cursor.describe_col(i, &mut col_desc).map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to describe column {}: {}", i, e))
            })?;
            let name = col_desc.name_to_string().map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to get column name: {}", e))
            })?;
            // The catalog's declared type when there is one, else what the driver reports
            let type_name = declared
                .get(&name)
                .cloned()
                .unwrap_or_else(|| col_type_name(&mut cursor, i));
            let unsigned = cursor.column_is_unsigned(i).unwrap_or(false);
            let (data_type, read_as) =
                column_reading(&col_desc, &type_name, unsigned, dialect.kind);
            debug!(
                "ODBC column {}: {:?} ({}) → {:?}",
                name, col_desc.data_type, type_name, data_type
            );
            schema_columns.push(Column {
                name: name.clone(),
                data_type: data_type.clone(),
                // Drivers that can't tell report "unknown"; treat that as nullable
                nullable: col_desc.nullability != odbc_api::Nullability::NoNulls,
            });
            columns.push((name, data_type, read_as));
        }

        self.schema = Some(Schema {
            columns: schema_columns,
            estimated_rows: None,
            primary_key_candidate: None,
        });
        self.exhausted = false;
        *self.open.lock().unwrap() = Some(OpenCursor {
            cursor,
            columns,
            fresh: true,
        });
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open.lock().unwrap().is_some()
    }
}

#[async_trait]
impl Source for OdbcSource {
    async fn connect(&mut self) -> Result<()> {
        self.open_cursor()
    }

    async fn infer_schema(&mut self, _sample_size: usize) -> Result<Schema> {
        if !self.is_open() {
            self.open_cursor()?;
        }
        Ok(self
            .schema
            .clone()
            .expect("schema is set when the cursor opens"))
    }

    async fn read_batch(&mut self, batch_size: usize) -> Result<Vec<Row>> {
        if self.exhausted {
            return Ok(Vec::new());
        }
        if !self.is_open() {
            self.open_cursor()?;
        }

        let mut guard = self.open.lock().unwrap();
        let open = guard.as_mut().expect("cursor is open");
        open.fresh = false;

        let mut rows = Vec::with_capacity(batch_size.min(10_000));
        // Read as UTF-16 (SQL_C_WCHAR) so the driver never converts through a narrow code page;
        // binary columns as bytes
        let mut buf: Vec<u16> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        while rows.len() < batch_size {
            let mut row_cursor =
                match open.cursor.next_row().map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to fetch row: {}", e))
                })? {
                    Some(row_cursor) => row_cursor,
                    None => {
                        self.exhausted = true;
                        break;
                    }
                };

            let mut row = Row::new();
            for (index, (name, data_type, read_as)) in open.columns.iter().enumerate() {
                let column = index as u16 + 1;
                let failed = |e: odbc_api::Error| {
                    TinyEtlError::DataTransfer(format!("Failed to read column {}: {}", name, e))
                };
                let value = if *read_as == ReadAs::Binary {
                    match row_cursor.get_binary(column, &mut bytes).map_err(failed)? {
                        true => Value::String(format!("0x{}", hex::encode(&bytes))),
                        false => Value::Null,
                    }
                } else if *read_as == ReadAs::BitBytes {
                    match row_cursor.get_binary(column, &mut bytes).map_err(failed)? {
                        true => Value::Boolean(bytes.iter().any(|b| *b != 0 && *b != b'0')),
                        false => Value::Null,
                    }
                } else {
                    match row_cursor.get_wide_text(column, &mut buf).map_err(failed)? {
                        true => read_text(String::from_utf16_lossy(&buf), data_type, *read_as),
                        false => Value::Null,
                    }
                };
                row.insert(name.clone(), value);
            }
            rows.push(row);
        }
        Ok(rows)
    }

    async fn estimated_row_count(&self) -> Result<Option<usize>> {
        Ok(None)
    }

    async fn reset(&mut self) -> Result<()> {
        let fresh = matches!(&*self.open.lock().unwrap(), Some(open) if open.fresh);
        if !fresh {
            // ODBC result sets only move forward, so start the query again
            self.open.lock().unwrap().take();
            self.open_cursor()?;
        }
        self.exhausted = false;
        Ok(())
    }

    fn has_more(&self) -> bool {
        !self.exhausted
    }
}

/// ODBC target: creates the table if needed and inserts rows with ODBC parameter arrays.
///
/// Checks and DDL run in autocommit mode. A transaction starts with the first truncate or insert
/// and is committed in `finalize`, so a failed transfer leaves the table as it was (for databases
/// with transactional DML).
pub struct OdbcTarget {
    connection_string: String,
    table_name: String,
    /// Behind a `Mutex` because `Target` must be `Sync`; ODBC handles are only `Send`.
    conn: Option<Mutex<Connection<'static>>>,
    dialect: Option<Dialect>,
    schema: Option<Schema>,
    in_transaction: bool,
}

impl OdbcTarget {
    pub fn new(connection_string: &str) -> Result<Self> {
        let (connection_string, table_name) = parse_connection_string(connection_string, "target")?;
        Ok(Self {
            connection_string,
            table_name,
            conn: None,
            dialect: None,
            schema: None,
            in_transaction: false,
        })
    }

    fn ensure_connected(&mut self) -> Result<()> {
        if self.conn.is_none() {
            let conn = connect(&self.connection_string)?;
            self.dialect = Some(Dialect::detect(&conn));
            self.conn = Some(Mutex::new(conn));
        }
        Ok(())
    }

    fn dialect(&self) -> &Dialect {
        self.dialect.as_ref().expect("connected")
    }

    /// Whether the table can be queried. Must run before the transaction starts: on some
    /// databases (PostgreSQL) a failed statement aborts the surrounding transaction.
    fn table_exists(&self, table_name: &str) -> bool {
        let Some(conn) = &self.conn else {
            return false;
        };
        let query = format!(
            "SELECT * FROM {} WHERE 1=0",
            self.dialect().qualified(table_name)
        );
        let conn = conn.lock().unwrap();
        let exists = conn.execute(&query, (), None).is_ok();
        exists
    }

    /// Turn autocommit off so the following statements run in one transaction.
    fn begin(&mut self) -> Result<()> {
        if !self.in_transaction {
            let conn = self.conn.as_ref().expect("connected").lock().unwrap();
            conn.set_autocommit(false).map_err(|e| {
                TinyEtlError::Connection(format!("Failed to start a transaction: {}", e))
            })?;
            drop(conn);
            self.in_transaction = true;
        }
        Ok(())
    }
}

#[async_trait]
impl Target for OdbcTarget {
    async fn connect(&mut self) -> Result<()> {
        self.ensure_connected()
    }

    async fn create_table(&mut self, table_name: &str, schema: &Schema) -> Result<()> {
        self.ensure_connected()?;

        if !self.table_exists(table_name) {
            let dialect = self.dialect();
            let columns = schema
                .columns
                .iter()
                .map(|col| {
                    format!(
                        "{} {}{}",
                        dialect.quote_ident(&col.name),
                        dialect.column_type(&col.data_type),
                        if col.nullable { "" } else { " NOT NULL" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let create_sql = format!(
                "CREATE TABLE {} ({})",
                dialect.qualified(table_name),
                columns
            );
            debug!("ODBC create table: {}", create_sql);

            let conn = self.conn.as_ref().expect("connected").lock().unwrap();
            conn.execute(&create_sql, (), None).map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to create table: {}", e))
            })?;
        }

        // Always set the schema, whether or not the table was created
        self.schema = Some(schema.clone());
        Ok(())
    }

    async fn write_batch(&mut self, rows: &[Row]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        self.ensure_connected()?;
        self.begin()?;

        let schema = self.schema.as_ref().ok_or_else(|| {
            TinyEtlError::Configuration("Schema not set for ODBC target".to_string())
        })?;
        let dialect = self.dialect();

        let column_names = schema
            .columns
            .iter()
            .map(|c| dialect.quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = vec!["?"; schema.columns.len()].join(", ");
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            dialect.qualified(&self.table_name),
            column_names,
            placeholders
        );

        // Format every cell as UTF-16 first, so the parameter buffers can be sized once per chunk.
        // Wide (SQL_C_WCHAR) parameters reach NVARCHAR/Unicode columns without passing through
        // a narrow code page, which would turn non-Latin text into '?' on SQL Server.
        let cells: Vec<Vec<Option<Vec<u16>>>> = rows
            .iter()
            .map(|row| {
                schema
                    .columns
                    .iter()
                    .map(|col| {
                        value_to_text(row.get(&col.name).unwrap_or(&Value::Null))
                            .map(|text| text.encode_utf16().collect())
                    })
                    .collect()
            })
            .collect();

        let conn = self.conn.as_ref().expect("connected").lock().unwrap();
        for chunk in cells.chunks(INSERT_CHUNK_ROWS) {
            let params = (0..schema.columns.len()).map(|col| {
                let max_len = chunk
                    .iter()
                    .filter_map(|row| row[col].as_ref().map(Vec::len))
                    .max()
                    .unwrap_or(0)
                    .max(1);
                BindParamDesc::wide_text(max_len)
            });
            let prepared = conn.prepare(&insert_sql).map_err(|e| {
                TinyEtlError::DataTransfer(format!("Failed to prepare INSERT: {}", e))
            })?;
            let mut inserter = prepared
                .into_column_inserter(chunk.len(), params)
                .map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to bind INSERT parameters: {}", e))
                })?;
            inserter.set_num_rows(chunk.len());
            for col in 0..schema.columns.len() {
                let mut column = inserter
                    .column_mut(col)
                    .as_wide_text()
                    .expect("parameters are bound as wide text");
                for (row_index, row) in chunk.iter().enumerate() {
                    column.set_cell(row_index, row[col].as_deref());
                }
            }
            inserter
                .execute()
                .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to insert rows: {}", e)))?;
        }

        Ok(rows.len())
    }

    async fn finalize(&mut self) -> Result<()> {
        if self.in_transaction {
            if let Some(conn) = &self.conn {
                let conn = conn.lock().unwrap();
                conn.commit().map_err(|e| {
                    TinyEtlError::DataTransfer(format!("Failed to commit transaction: {}", e))
                })?;
                // Leave the connection in its default mode
                let _ = conn.set_autocommit(true);
            }
            self.in_transaction = false;
        }
        Ok(())
    }

    async fn exists(&self, table_name: &str) -> Result<bool> {
        Ok(self.table_exists(table_name))
    }

    async fn truncate(&mut self, table_name: &str) -> Result<()> {
        self.ensure_connected()?;
        // Part of the transaction, so the old rows come back if the transfer fails
        self.begin()?;
        let sql = format!("DELETE FROM {}", self.dialect().qualified(table_name));
        let conn = self.conn.as_ref().expect("connected").lock().unwrap();
        conn.execute(&sql, (), None)
            .map_err(|e| TinyEtlError::DataTransfer(format!("Failed to truncate table: {}", e)))?;
        Ok(())
    }

    fn supports_append(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminated() {
        assert_eq!(terminated("DSN=x;PWD=secret"), "DSN=x;PWD=secret;");
        assert_eq!(terminated("DSN=x;PWD={a;b} "), "DSN=x;PWD={a;b};");
        assert_eq!(terminated("DSN=x;"), "DSN=x;");
        assert_eq!(terminated(""), "");
    }

    #[test]
    fn test_homebrew_odbc_sysini() {
        let only = |present: &'static [&'static str]| move |p: &str| present.contains(&p);

        // Respect an explicit ODBCSYSINI
        assert_eq!(
            homebrew_odbc_sysini(true, only(&["/opt/homebrew/etc/odbcinst.ini"])),
            None
        );
        // A system-wide /etc/odbcinst.ini takes precedence
        assert_eq!(
            homebrew_odbc_sysini(
                false,
                only(&["/etc/odbcinst.ini", "/opt/homebrew/etc/odbcinst.ini"])
            ),
            None
        );
        // Apple silicon Homebrew, then Intel Homebrew
        assert_eq!(
            homebrew_odbc_sysini(false, only(&["/opt/homebrew/etc/odbcinst.ini"])),
            Some("/opt/homebrew/etc")
        );
        assert_eq!(
            homebrew_odbc_sysini(false, only(&["/usr/local/etc/odbcinst.ini"])),
            Some("/usr/local/etc")
        );
        // Nothing registered anywhere
        assert_eq!(homebrew_odbc_sysini(false, only(&[])), None);
    }

    #[test]
    fn test_rewrite_driver_attribute() {
        let registered = |name: &str| match name {
            "PostgreSQL Unicode" => Some("psqlodbcw.so".to_string()),
            "ODBC Driver 18 for SQL Server" => {
                Some("/opt/microsoft/msodbcsql18/lib64/libmsodbcsql-18.so".to_string())
            }
            _ => None,
        };
        let find = |file: &str| {
            (file == "psqlodbcw.so").then(|| "/usr/lib/x86_64-linux-gnu/odbc/psqlodbcw.so".into())
        };
        let rewrite = |cs: &str| rewrite_driver_attribute(cs, registered, find).map(|(_, s)| s);

        // A registered name whose library is a bare file name
        assert_eq!(
            rewrite("Driver={PostgreSQL Unicode};Server=db;PWD={a;b}").as_deref(),
            Some("Driver={/usr/lib/x86_64-linux-gnu/odbc/psqlodbcw.so};Server=db;PWD={a;b}")
        );
        // A bare file name given directly, with any key casing
        assert_eq!(
            rewrite("server=db;DRIVER=psqlodbcw.so").as_deref(),
            Some("server=db;DRIVER={/usr/lib/x86_64-linux-gnu/odbc/psqlodbcw.so}")
        );
        // Left alone: registered with an absolute path, unknown names, unfindable files, DSNs
        assert_eq!(
            rewrite("Driver={ODBC Driver 18 for SQL Server};Server=x"),
            None
        );
        assert_eq!(rewrite("Driver={Unknown};Server=x"), None);
        assert_eq!(rewrite("Driver=libmissing.so"), None);
        assert_eq!(rewrite("DSN=mydb;UID=u"), None);
    }

    #[test]
    fn test_ini_value() {
        let ini = "; comment\n[PostgreSQL ANSI]\nDriver=psqlodbca.so\n\n[PostgreSQL Unicode]\n\
                   Description = PostgreSQL ODBC driver (Unicode version)\n driver = psqlodbcw.so \n";
        assert_eq!(
            ini_value(ini, "postgresql unicode", "Driver").as_deref(),
            Some("psqlodbcw.so")
        );
        assert_eq!(
            ini_value(ini, "PostgreSQL ANSI", "Driver").as_deref(),
            Some("psqlodbca.so")
        );
        assert_eq!(ini_value(ini, "MySQL", "Driver"), None);
    }

    #[test]
    fn test_linux_odbc_driver_dirs() {
        let only = |present: &'static [&'static str]| move |p: &str| present.contains(&p);
        assert_eq!(
            linux_odbc_driver_dirs(only(&["/usr/lib64/odbc"])),
            vec!["/usr/lib64/odbc"]
        );
        assert!(linux_odbc_driver_dirs(only(&[])).is_empty());
    }

    #[test]
    fn test_dbms_name_detection() {
        assert_eq!(
            DbKind::from_dbms_name("Microsoft SQL Server"),
            DbKind::SqlServer
        );
        assert_eq!(DbKind::from_dbms_name("PostgreSQL"), DbKind::Postgres);
        assert_eq!(DbKind::from_dbms_name("MySQL"), DbKind::MySql);
        assert_eq!(DbKind::from_dbms_name("MariaDB"), DbKind::MySql);
        assert_eq!(DbKind::from_dbms_name("SQLite"), DbKind::Sqlite);
        assert_eq!(DbKind::from_dbms_name("Oracle"), DbKind::Other);
        assert_eq!(DbKind::from_dbms_name(""), DbKind::Other);
    }

    #[test]
    fn test_identifier_quoting() {
        let ansi = Dialect::new("PostgreSQL", Some('"'));
        assert_eq!(ansi.quote_ident("customers"), "\"customers\"");
        assert_eq!(ansi.quote_ident("odd\"name"), "\"odd\"\"name\"");
        assert_eq!(
            ansi.qualified("public.customers"),
            "\"public\".\"customers\""
        );

        let mysql = Dialect::new("MySQL", Some('`'));
        assert_eq!(mysql.quote_ident("order"), "`order`");
        assert_eq!(mysql.quote_ident("a`b"), "`a``b`");
        assert_eq!(mysql.qualified("testdb.orders"), "`testdb`.`orders`");

        let unquoted = Dialect::new("Some DB", None);
        assert_eq!(unquoted.qualified("dbo.orders"), "dbo.orders");
    }

    #[test]
    fn test_column_types_per_dialect() {
        let types = |name| {
            let d = Dialect::new(name, Some('"'));
            [
                DataType::String,
                DataType::Integer,
                DataType::Decimal,
                DataType::Boolean,
                DataType::DateTime,
                DataType::Json,
            ]
            .map(|t| d.column_type(&t))
        };
        assert_eq!(
            types("Microsoft SQL Server"),
            [
                "NVARCHAR(MAX)",
                "BIGINT",
                "DECIMAL(18,6)",
                "BIT",
                "DATETIME2",
                "NVARCHAR(MAX)"
            ]
        );
        assert_eq!(
            types("PostgreSQL"),
            ["TEXT", "BIGINT", "DECIMAL", "BOOLEAN", "TIMESTAMP", "TEXT"]
        );
        assert_eq!(
            types("MySQL"),
            [
                "TEXT",
                "BIGINT",
                "DECIMAL(65,30)",
                "BOOLEAN",
                "DATETIME",
                "JSON"
            ]
        );
        assert_eq!(
            types("SQLite"),
            ["TEXT", "INTEGER", "REAL", "INTEGER", "TEXT", "TEXT"]
        );
        assert_eq!(
            types("Oracle"),
            [
                "VARCHAR(4000)",
                "BIGINT",
                "DECIMAL(38,10)",
                "BOOLEAN",
                "TIMESTAMP",
                "VARCHAR(4000)"
            ]
        );
    }

    #[test]
    fn test_read_text_by_database_type() {
        let read =
            |text: &str, read_as| render(read_text(text.to_string(), &DataType::String, read_as));
        assert_eq!(
            read("$1,234.56", ReadAs::Money { normalize: false }),
            "dec 1234.56"
        );
        assert_eq!(
            read("-$5.00", ReadAs::Money { normalize: false }),
            "dec -5.00"
        );
        assert_eq!(
            read("12.3400", ReadAs::Money { normalize: true }),
            "dec 12.34"
        );
        assert_eq!(read("0.10000000000000001", ReadAs::Float), "dec 0.1");
        assert_eq!(read("13:45:30.0000000", ReadAs::Time), "str 13:45:30");
        assert_eq!(read("13:45:30.1230000", ReadAs::Time), "str 13:45:30.123");
        assert_eq!(read("123E4567-AB", ReadAs::Lowercase), "str 123e4567-ab");
        assert_eq!(read("{\"a\": [1]}", ReadAs::Json), "json {\"a\":[1]}");
    }

    #[test]
    fn test_pg_array_to_json() {
        let json = |text: &str| pg_array_to_json(text).map(|j| j.to_string());
        assert_eq!(json("{1,2,3}").as_deref(), Some("[1,2,3]"));
        assert_eq!(json("{}").as_deref(), Some("[]"));
        assert_eq!(json("{a,b}").as_deref(), Some("[\"a\",\"b\"]"));
        assert_eq!(
            json("{\"a b\",NULL,\"say \\\"hi\\\"\"}").as_deref(),
            Some("[\"a b\",null,\"say \\\"hi\\\"\"]")
        );
        assert_eq!(json("{{1,2},{3,4}}").as_deref(), Some("[[1,2],[3,4]]"));
        assert_eq!(json("{t,f}").as_deref(), Some("[true,false]"));
        assert_eq!(json("not an array"), None);
        assert_eq!(json("{1,2"), None);
    }

    fn render(value: Value) -> String {
        match value {
            Value::Decimal(d) => format!("dec {}", d),
            Value::String(s) => format!("str {}", s),
            Value::Json(j) => format!("json {}", j),
            other => format!("{:?}", other),
        }
    }

    #[test]
    fn test_text_to_value() {
        assert_eq!(
            text_to_value("42".into(), &DataType::Integer),
            Value::Integer(42)
        );
        assert_eq!(
            text_to_value("12.50".into(), &DataType::Decimal),
            Value::Decimal(Decimal::from_str("12.50").unwrap())
        );
        assert_eq!(
            text_to_value("1.5E-3".into(), &DataType::Decimal),
            Value::Decimal(Decimal::from_str("0.0015").unwrap())
        );
        assert_eq!(
            text_to_value("1".into(), &DataType::Boolean),
            Value::Boolean(true)
        );
        assert_eq!(
            text_to_value("f".into(), &DataType::Boolean),
            Value::Boolean(false)
        );
        assert!(matches!(
            text_to_value("2024-01-15 10:30:00".into(), &DataType::DateTime),
            Value::Date(_)
        ));
        // Text that doesn't parse as the column type is kept as a string
        assert_eq!(
            text_to_value("n/a".into(), &DataType::Integer),
            Value::String("n/a".into())
        );
        assert_eq!(
            text_to_value(" padded ".into(), &DataType::String),
            Value::String(" padded ".into())
        );
    }

    #[test]
    fn test_value_to_text_round_trip() {
        assert_eq!(value_to_text(&Value::Null), None);
        assert_eq!(value_to_text(&Value::Boolean(true)), Some("1".into()));
        let parsed = text_to_value("2024-01-15 10:30:00".into(), &DataType::DateTime);
        assert_eq!(value_to_text(&parsed), Some("2024-01-15 10:30:00".into()));
    }

    #[test]
    fn test_parse_connection_string() {
        let (conn, table) = parse_connection_string(
            "odbc://Driver={PostgreSQL Unicode};Server=localhost;Database=db#public.orders",
            "source",
        )
        .unwrap();
        assert_eq!(
            conn,
            "Driver={PostgreSQL Unicode};Server=localhost;Database=db"
        );
        assert_eq!(table, "public.orders");
        assert!(parse_connection_string("odbc://DSN=x", "source").is_err());
        assert!(parse_connection_string("odbc://DSN=x#", "target").is_err());
    }
}
