use std::env;
use tracing::{info, warn};
use url::Url;

/// Checks if a URL contains a password and logs a warning if it does
pub fn check_and_warn_about_password_in_url(url: &str, source_name: &str) {
    if let Ok(parsed_url) = Url::parse(url) {
        if !parsed_url.password().unwrap_or("").is_empty() {
            warn!("Warning: Using passwords in CLI parameters is insecure. Consider using --source-secret-id / --dest-secret-id.");
        }
    }
    // Also check for common password patterns in connection strings
    else if url.contains("password=")
        || url.contains("pwd=")
        || url.contains(":") && url.contains("@")
    {
        warn!("Warning: Using passwords in CLI parameters is insecure. Consider using --source-secret-id / --dest-secret-id.");
    }
}

/// Resolves a secret from environment variables
/// Returns the secret value if found, otherwise returns an error
pub fn resolve_secret(secret_id: &str) -> Result<String, String> {
    let env_var = format!("TINYETL_SECRET_{}", secret_id);
    match env::var(&env_var) {
        Ok(value) => {
            info!("Successfully resolved secret for ID: {}", secret_id);
            Ok(value)
        }
        Err(_) => Err(format!(
            "Secret not found: Environment variable {} is not set",
            env_var
        )),
    }
}

/// Processes a URL/connection string by replacing placeholders with resolved secrets
/// If secret_id is provided, it will resolve the secret and inject it into the URL
/// If the URL already contains credentials and a secret_id is provided, the secret takes precedence
pub fn process_connection_string(
    original_url: &str,
    secret_id: Option<&String>,
    connection_type: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // First, check for password in URL and warn
    check_and_warn_about_password_in_url(original_url, connection_type);

    // If no secret_id is provided, return the original URL
    let secret_id = match secret_id {
        Some(id) => id,
        None => return Ok(original_url.to_string()),
    };

    // Resolve the secret
    let secret_value = resolve_secret(secret_id)
        .map_err(|e| format!("Failed to resolve {} secret: {}", connection_type, e))?;

    // Try to parse as URL for standard database connection strings
    if let Ok(mut parsed_url) = Url::parse(original_url) {
        // If URL already has a password and we have a secret, warn about override
        if !parsed_url.password().unwrap_or("").is_empty() {
            warn!(
                "Overriding password in {} URL with secret from environment variable",
                connection_type
            );
        }

        // Set the password from the secret
        let _ = parsed_url.set_password(Some(&secret_value));
        Ok(parsed_url.to_string())
    } else {
        // Handle non-URL connection strings (like some database formats)
        // For now, we'll assume these need custom handling per connector
        // This is where future enterprise vault integrations could go
        Err(format!(
            "Cannot inject secret into non-URL connection string for {}. URL format required when using secret IDs.", 
            connection_type
        ).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_resolve_secret_success() {
        env::set_var("TINYETL_SECRET_test", "mysecret");
        let result = resolve_secret("test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "mysecret");
        env::remove_var("TINYETL_SECRET_test");
    }

    #[test]
    fn test_resolve_secret_not_found() {
        env::remove_var("TINYETL_SECRET_nonexistent");
        let result = resolve_secret("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("TINYETL_SECRET_nonexistent"));
    }

    #[test]
    fn test_process_connection_string_with_secret() {
        env::set_var("TINYETL_SECRET_mysql_test", "testpass");

        let result = process_connection_string(
            "mysql://user@localhost:3306/db",
            Some(&"mysql_test".to_string()),
            "source",
        );

        assert!(result.is_ok());
        let processed = result.unwrap();
        assert!(processed.contains("testpass"));
        assert!(processed.contains("user:testpass@localhost"));

        env::remove_var("TINYETL_SECRET_mysql_test");
    }

    #[test]
    fn test_process_connection_string_no_secret() {
        let original = "mysql://user:pass@localhost:3306/db";
        let result = process_connection_string(original, None, "source");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), original);
    }

    #[test]
    fn test_process_connection_string_invalid_secret() {
        env::remove_var("TINYETL_SECRET_invalid");

        let result = process_connection_string(
            "mysql://user@localhost:3306/db",
            Some(&"invalid".to_string()),
            "source",
        );

        assert!(result.is_err());
    }
}
