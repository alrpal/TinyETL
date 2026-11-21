//! [`SnowflakeClient`] backed by the `firn` crate. This is the only module that uses `firn`.

use async_trait::async_trait;
use firn::{
    Bind, ConnectionConfig, SecretString, SnowflakeApi, SnowflakeApiBuilder, SnowflakeType,
};
use futures_util::stream::StreamExt;
use std::path::Path;

use super::client::{ColumnInfo, QueryStream, SnowflakeClient};
use super::SnowflakeConfig;
use crate::{schema::DataType, Result, TinyEtlError};

pub(super) struct FirnClient {
    api: SnowflakeApi,
}

/// Log in to Snowflake. Fails here, rather than on the first query, if the account,
/// credentials or warehouse are wrong.
pub(super) async fn connect(cfg: &SnowflakeConfig) -> Result<FirnClient> {
    let mut conn = ConnectionConfig {
        account: Some(cfg.account.clone()),
        user: Some(cfg.user.clone()),
        warehouse: cfg.warehouse.clone(),
        database: Some(cfg.database.clone()),
        schema: Some(cfg.schema.clone()),
        role: cfg.role.clone(),
        application: Some("TinyETL".to_string()),
        ..ConnectionConfig::default()
    };
    if let Some(path) = &cfg.private_key_path {
        // Key-pair auth wins over a password; firn would pick the password if both were set.
        conn.authenticator = Some("snowflake_jwt".to_string());
        conn.private_key_file = Some(path.into());
        conn.private_key_file_pwd = cfg.private_key_passphrase.clone().map(SecretString::from);
    } else if let Some(password) = &cfg.password {
        conn.password = Some(SecretString::from(password.clone()));
    } else {
        return Err(TinyEtlError::Configuration(
            "Snowflake needs credentials: set the private_key_path option (or \
             SNOWFLAKE_PRIVATE_KEY_PATH), or put a password or programmatic access token in the URL"
                .to_string(),
        ));
    }

    let api = SnowflakeApiBuilder::from_connection_config(conn)
        .and_then(SnowflakeApiBuilder::build)
        .map_err(|e| TinyEtlError::Connection(format!("Snowflake configuration error: {}", e)))?;
    // firn logs in lazily on the first request; do it now
    api.exec("SELECT 1").await.map_err(|e| {
        // An answer that isn't a Snowflake login response (e.g. an error page) almost always
        // means there's no account at that address
        use firn::{AuthError, SnowflakeApiError as E};
        let no_login_response = matches!(
            e,
            E::RequestError(_)
                | E::UnexpectedResponse
                | E::EmptyResponse
                | E::AuthError(AuthError::RequestError(_) | AuthError::UnexpectedResponse)
        );
        let hint = match no_login_response {
            false => String::new(),
            true => format!(
                ". No Snowflake account answered at {}: check the account identifier \
                 (ORGNAME-ACCOUNTNAME, see SELECT CURRENT_ORGANIZATION_NAME() || '-' || \
                 CURRENT_ACCOUNT_NAME())",
                api.base_url()
            ),
        };
        TinyEtlError::Connection(format!(
            "Failed to connect to Snowflake account '{}' as '{}': {}{}",
            cfg.account, cfg.user, e, hint
        ))
    })?;
    Ok(FirnClient { api })
}

fn query_error(e: impl std::fmt::Display) -> TinyEtlError {
    TinyEtlError::DataTransfer(format!("Snowflake: {}", e))
}

fn column_info(field: &firn::FieldSchema) -> ColumnInfo {
    let data_type = match field.type_ {
        SnowflakeType::Fixed if field.scale.unwrap_or(0) == 0 => DataType::Integer,
        SnowflakeType::Fixed | SnowflakeType::Real => DataType::Decimal,
        SnowflakeType::Boolean => DataType::Boolean,
        SnowflakeType::Date => DataType::Date,
        SnowflakeType::TimestampNtz | SnowflakeType::TimestampLtz | SnowflakeType::TimestampTz => {
            DataType::DateTime
        }
        // GEOGRAPHY / GEOMETRY ride on OBJECT; keep them as text
        SnowflakeType::Object if field.ext_type_name.is_some() => DataType::String,
        SnowflakeType::Variant | SnowflakeType::Object | SnowflakeType::Array => DataType::Json,
        _ => DataType::String,
    };
    ColumnInfo {
        name: field.name.clone(),
        data_type,
        nullable: field.nullable,
    }
}

/// A single-quoted SQL string literal.
fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[async_trait]
impl SnowflakeClient for FirnClient {
    async fn execute(&self, sql: &str, binds: Vec<Option<String>>) -> Result<()> {
        let binds = binds.into_iter().map(|value| match value {
            Some(text) => Bind::text(text),
            None => Bind::null("TEXT"),
        });
        self.api
            .query(sql)
            .binds(binds)
            .execute()
            .await
            .map(|_| ())
            .map_err(query_error)
    }

    async fn query(&self, sql: &str) -> Result<QueryStream> {
        let (metadata, batches) = self
            .api
            .query(sql)
            .execute_stream()
            .await
            .map_err(query_error)?;
        Ok(QueryStream {
            columns: metadata.column_schema.iter().map(column_info).collect(),
            total_rows: metadata.total_rows.and_then(|n| u64::try_from(n).ok()),
            batches: batches.map(|batch| batch.map_err(query_error)).boxed(),
        })
    }

    async fn put(&self, local_file: &Path, stage_location: &str) -> Result<()> {
        let path = local_file.to_string_lossy().replace('\\', "/");
        let uri = if path.starts_with('/') {
            format!("file://{}", path)
        } else {
            format!("file:///{}", path) // Windows: C:/...
        };
        let sql = format!(
            "PUT {} {} AUTO_COMPRESS = FALSE SOURCE_COMPRESSION = GZIP OVERWRITE = TRUE",
            sql_string(&uri),
            sql_string(stage_location)
        );
        self.api.exec(&sql).await.map(|_| ()).map_err(|e| {
            TinyEtlError::DataTransfer(format!(
                "Snowflake: failed to upload to stage {}: {} (set the option load_method = insert \
                 to load without a stage)",
                stage_location, e
            ))
        })
    }

    async fn close(&self) -> Result<()> {
        self.api.close_session().await.map_err(query_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_string_escaping() {
        assert_eq!(sql_string("file:///tmp/a.csv.gz"), "'file:///tmp/a.csv.gz'");
        assert_eq!(sql_string("it's"), "'it\\'s'");
        assert_eq!(sql_string("a\\b"), "'a\\\\b'");
    }
}
