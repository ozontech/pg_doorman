//! Tests for configuration module.

use super::*;
use serial_test::serial;
use std::io::Write;
use tempfile::NamedTempFile;

// Helper function to create a temporary config file for testing
fn create_temp_config() -> NamedTempFile {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432
idle_timeout = 40000

[[pools.example_db.users]]
username = "example_user_1"
password = "password1"
pool_size = 40
pool_mode = "transaction"

[[pools.example_db.users]]
username = "example_user_2"
password = "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
pool_size = 20

[pools.test_db1]
server_host = "localhost"
server_port = 5432

[pools.test_db2]
server_host = "localhost"
server_port = 5432

[pools.test_db3]
server_host = "localhost"
server_port = 5432
"#;
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();
    temp_file
}

#[tokio::test]
#[serial]
async fn test_config() {
    let temp_file = create_temp_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();

    assert_eq!(get_config().pools.len(), 4);
    assert_eq!(get_config().pools["example_db"].idle_timeout, Some(40000));
    assert_eq!(
        get_config().pools["example_db"].users[0].username,
        "example_user_1"
    );
    assert_eq!(
        get_config().pools["example_db"].users[1].password,
        "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
    );
    assert_eq!(get_config().pools["example_db"].users[1].pool_size, 20);
    assert_eq!(
        get_config().pools["example_db"].users[1].username,
        "example_user_2"
    );
    assert_eq!(get_config().pools["example_db"].users[0].pool_size, 40);
    assert_eq!(
        get_config().pools["example_db"].users[0].pool_mode,
        Some(PoolMode::Transaction)
    );
}

#[tokio::test]
#[serial]
async fn test_serialize_configs() {
    let temp_file = create_temp_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();
    print!("{}", toml::to_string(&get_config()).unwrap());
}

// Tests for the validate function

// Test valid configuration
#[tokio::test]
async fn test_validate_valid_config() {
    let mut config = Config::default();

    // Add a pool with a user
    let mut pool = Pool::default();
    let user = User {
        username: "test_user".to_string(),
        password: "test_password".to_string(),
        pool_size: 50, // Greater than virtual_pool_count
        ..User::default()
    };
    pool.users.push(user);
    config.pools.insert("test_pool".to_string(), pool);

    // Set valid TLS rate limit
    config.general.tls_rate_limit_per_second = 100;

    // Set valid prepared statements config
    config.general.prepared_statements = true;
    config.general.prepared_statements_cache_size = 1024;

    // Validate should pass
    let result = config.validate().await;
    assert!(result.is_ok());
}

// Test TLS rate limit less than 100 (but not 0)
#[tokio::test]
async fn test_validate_tls_rate_limit_less_than_100() {
    let mut config = Config::default();

    // Set invalid TLS rate limit
    config.general.tls_rate_limit_per_second = 50;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls rate limit should be > 100"));
    } else {
        panic!("Expected BadConfig error about tls rate limit");
    }
}

// Test TLS rate limit not multiple of 100
#[tokio::test]
async fn test_validate_tls_rate_limit_not_multiple_of_100() {
    let mut config = Config::default();

    // Set invalid TLS rate limit
    config.general.tls_rate_limit_per_second = 150;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls rate limit should be multiple 100"));
    } else {
        panic!("Expected BadConfig error about tls rate limit multiple");
    }
}

// Test HBA and pg_hba both set
#[tokio::test]
async fn test_validate_hba_and_pg_hba_both_set() {
    let mut config = Config::default();

    // Set both HBA settings
    config.general.hba = vec!["192.168.1.0/24".parse().unwrap()];
    config.general.pg_hba = Some(crate::auth::hba::PgHba::default());

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("general.hba and general.pg_hba cannot be specified at the same time"));
    } else {
        panic!("Expected BadConfig error about hba and pg_hba");
    }
}

// Test prepared_statements enabled but cache_size is 0
#[tokio::test]
async fn test_validate_prepared_statements_no_cache() {
    let mut config = Config::default();

    // Set invalid prepared statements config
    config.general.prepared_statements = true;
    config.general.prepared_statements_cache_size = 0;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("prepared_statements_cache"));
    } else {
        panic!("Expected BadConfig error about prepared_statements_cache");
    }
}

// Test tls_certificate set but tls_private_key not set
#[tokio::test]
async fn test_validate_tls_certificate_without_private_key() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_certificate = Some("cert.pem".to_string());
    config.general.tls_private_key = None;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_certificate is set but tls_private_key is not"));
    } else {
        panic!("Expected BadConfig error about tls_certificate without tls_private_key");
    }
}

// Test tls_private_key set but tls_certificate not set
#[tokio::test]
async fn test_validate_tls_private_key_without_certificate() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_certificate = None;
    config.general.tls_private_key = Some("key.pem".to_string());

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_private_key is set but tls_certificate is not"));
    } else {
        panic!("Expected BadConfig error about tls_private_key without tls_certificate");
    }
}

// Test tls_mode set but tls_certificate or tls_private_key not set
#[tokio::test]
async fn test_validate_tls_mode_without_cert_or_key() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_mode = Some("require".to_string());
    config.general.tls_certificate = None;
    config.general.tls_private_key = None;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_mode is require but tls_certificate or tls_private_key is not"));
    } else {
        panic!("Expected BadConfig error about tls_mode without cert/key");
    }
}

// Test tls_mode is verify-full but tls_ca_cert is not set
#[tokio::test]
async fn test_validate_tls_mode_verify_full_without_ca_cert() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_mode = Some("verify-full".to_string());
    config.general.tls_certificate = Some("cert.pem".to_string());
    config.general.tls_private_key = Some("key.pem".to_string());
    config.general.tls_ca_cert = None;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_mode is verify-full but tls_ca_cert is not set"));
    } else {
        panic!("Expected BadConfig error about tls_mode verify-full without ca_cert");
    }
}

// Test valid TLS configuration with mode "allow"
#[tokio::test]
async fn test_validate_valid_tls_mode_allow() {
    let mut config = Config::default();

    // Set valid TLS config for "allow" mode
    config.general.tls_mode = Some("allow".to_string());

    // For "allow" mode, certificates are optional
    // Test without certificates to avoid certificate validation
    let result = config.validate().await;
    assert!(
        result.is_ok(),
        "Validation should pass for 'allow' mode without certificates"
    );
}

// Test valid TLS configuration with mode "disable"
#[tokio::test]
async fn test_validate_valid_tls_mode_disable() {
    let mut config = Config::default();

    // Set valid TLS config for "disable" mode
    config.general.tls_mode = Some("disable".to_string());

    // For "disable" mode, certificates are optional
    // Test without certificates to avoid certificate validation
    let result = config.validate().await;
    assert!(
        result.is_ok(),
        "Validation should pass for 'disable' mode without certificates"
    );
}

// ============================================================================
// Tests for YAML configuration support
// ============================================================================

#[test]
fn test_config_format_detect_toml() {
    assert_eq!(ConfigFormat::detect("config.toml"), ConfigFormat::Toml);
    assert_eq!(
        ConfigFormat::detect("/path/to/config.toml"),
        ConfigFormat::Toml
    );
    assert_eq!(ConfigFormat::detect("CONFIG.TOML"), ConfigFormat::Toml);
}

#[test]
fn test_config_format_detect_yaml() {
    assert_eq!(ConfigFormat::detect("config.yaml"), ConfigFormat::Yaml);
    assert_eq!(ConfigFormat::detect("config.yml"), ConfigFormat::Yaml);
    assert_eq!(
        ConfigFormat::detect("/path/to/config.yaml"),
        ConfigFormat::Yaml
    );
    assert_eq!(
        ConfigFormat::detect("/path/to/config.yml"),
        ConfigFormat::Yaml
    );
    assert_eq!(ConfigFormat::detect("CONFIG.YAML"), ConfigFormat::Yaml);
    assert_eq!(ConfigFormat::detect("CONFIG.YML"), ConfigFormat::Yaml);
}

#[test]
fn test_config_format_detect_default_to_toml() {
    // Unknown extensions should default to TOML
    assert_eq!(ConfigFormat::detect("config.json"), ConfigFormat::Toml);
    assert_eq!(ConfigFormat::detect("config"), ConfigFormat::Toml);
    assert_eq!(ConfigFormat::detect("config.txt"), ConfigFormat::Toml);
}

// Helper function to create a temporary YAML config file for testing
fn create_temp_yaml_config() -> NamedTempFile {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  example_db:
    server_host: "localhost"
    server_port: 5432
    idle_timeout: 40000
    users:
      - username: "example_user_1"
        password: "password1"
        pool_size: 40
        pool_mode: "transaction"
      - username: "example_user_2"
        password: "password2"
        pool_size: 20
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();
    temp_file
}

#[tokio::test]
#[serial]
async fn test_yaml_config_parsing() {
    let temp_file = create_temp_yaml_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.pools.len(), 1);
    assert_eq!(config.pools["example_db"].idle_timeout, Some(40000));
    assert_eq!(
        config.pools["example_db"].users[0].username,
        "example_user_1"
    );
    assert_eq!(config.pools["example_db"].users[0].pool_size, 40);
    assert_eq!(
        config.pools["example_db"].users[0].pool_mode,
        Some(PoolMode::Transaction)
    );
    assert_eq!(
        config.pools["example_db"].users[1].username,
        "example_user_2"
    );
    assert_eq!(config.pools["example_db"].users[1].pool_size, 20);
}

#[tokio::test]
#[serial]
async fn test_yaml_config_serialize() {
    let temp_file = create_temp_yaml_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();

    let config = get_config();
    // Test that config can be serialized to YAML
    let yaml_output = serde_yaml::to_string(&config).unwrap();
    assert!(yaml_output.contains("example_db"));
    assert!(yaml_output.contains("example_user_1"));

    // Test that config can be serialized to TOML
    let toml_output = toml::to_string_pretty(&config).unwrap();
    assert!(toml_output.contains("example_db"));
    assert!(toml_output.contains("example_user_1"));
}

#[test]
fn test_content_to_toml_string_toml() {
    let toml_content = r#"
[general]
host = "127.0.0.1"
port = 6432
"#;
    let result = content_to_toml_string(toml_content, ConfigFormat::Toml).unwrap();
    assert_eq!(result, toml_content);
}

#[test]
fn test_content_to_toml_string_yaml() {
    let yaml_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
"#;
    let result = content_to_toml_string(yaml_content, ConfigFormat::Yaml).unwrap();
    // Result should be valid TOML
    assert!(result.contains("[general]"));
    assert!(result.contains("host"));
    assert!(result.contains("port"));
}

#[test]
fn test_parse_config_content_toml() {
    let toml_content = r#"
[include]
files = []
"#;
    let result: GeneralWithInclude =
        parse_config_content(toml_content, ConfigFormat::Toml).unwrap();
    assert!(result.include.files.is_empty());
}

#[test]
fn test_parse_config_content_yaml() {
    let yaml_content = r#"
include:
  files: []
"#;
    let result: GeneralWithInclude =
        parse_config_content(yaml_content, ConfigFormat::Yaml).unwrap();
    assert!(result.include.files.is_empty());
}

// ============================================================================
// TOML Backward Compatibility Tests
// ============================================================================
// These tests verify that the old TOML format [pools.*.users.0] continues to work
// after the migration to the new array format [[pools.*.users]]

/// Test parsing legacy TOML format with [pools.*.users.0] syntax
#[tokio::test]
#[serial]
async fn test_toml_legacy_users_format() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432

[pools.example_db.users.0]
username = "legacy_user_1"
password = "password1"
pool_size = 30

[pools.example_db.users.1]
username = "legacy_user_2"
password = "password2"
pool_size = 20
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.pools.len(), 1);
    assert_eq!(config.pools["example_db"].users.len(), 2);
    assert_eq!(
        config.pools["example_db"].users[0].username,
        "legacy_user_1"
    );
    assert_eq!(config.pools["example_db"].users[0].pool_size, 30);
    assert_eq!(
        config.pools["example_db"].users[1].username,
        "legacy_user_2"
    );
    assert_eq!(config.pools["example_db"].users[1].pool_size, 20);
}

/// Test parsing new TOML format with [[pools.*.users]] syntax
#[tokio::test]
#[serial]
async fn test_toml_new_array_users_format() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432

[[pools.example_db.users]]
username = "new_user_1"
password = "password1"
pool_size = 40

[[pools.example_db.users]]
username = "new_user_2"
password = "password2"
pool_size = 25
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.pools.len(), 1);
    assert_eq!(config.pools["example_db"].users.len(), 2);
    assert_eq!(config.pools["example_db"].users[0].username, "new_user_1");
    assert_eq!(config.pools["example_db"].users[0].pool_size, 40);
    assert_eq!(config.pools["example_db"].users[1].username, "new_user_2");
    assert_eq!(config.pools["example_db"].users[1].pool_size, 25);
}

/// Test parsing mixed TOML formats - different pools using different user formats
#[tokio::test]
#[serial]
async fn test_toml_mixed_users_formats() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.legacy_pool]
server_host = "localhost"
server_port = 5432

[pools.legacy_pool.users.0]
username = "legacy_user"
password = "password1"
pool_size = 30

[pools.new_pool]
server_host = "localhost"
server_port = 5433

[[pools.new_pool.users]]
username = "new_user"
password = "password2"
pool_size = 40
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.pools.len(), 2);

    // Check legacy pool
    assert_eq!(config.pools["legacy_pool"].users.len(), 1);
    assert_eq!(config.pools["legacy_pool"].users[0].username, "legacy_user");
    assert_eq!(config.pools["legacy_pool"].users[0].pool_size, 30);

    // Check new pool
    assert_eq!(config.pools["new_pool"].users.len(), 1);
    assert_eq!(config.pools["new_pool"].users[0].username, "new_user");
    assert_eq!(config.pools["new_pool"].users[0].pool_size, 40);
}

/// Test that legacy TOML format with multiple users preserves all user attributes
#[tokio::test]
#[serial]
async fn test_toml_legacy_format_all_user_attributes() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432

[pools.example_db.users.0]
username = "full_user"
password = "md5abcdef1234567890abcdef12345678"
pool_size = 50
min_pool_size = 5
pool_mode = "session"
server_lifetime = 3600000
server_username = "real_server_user"
server_password = "real_server_password"
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    let user = &config.pools["example_db"].users[0];

    assert_eq!(user.username, "full_user");
    assert_eq!(user.password, "md5abcdef1234567890abcdef12345678");
    assert_eq!(user.pool_size, 50);
    assert_eq!(user.min_pool_size, Some(5));
    assert_eq!(user.pool_mode, Some(PoolMode::Session));
    assert_eq!(user.server_lifetime, Some(3600000));
    assert_eq!(user.server_username, Some("real_server_user".to_string()));
    assert_eq!(
        user.server_password,
        Some("real_server_password".to_string())
    );
}

/// Test that duplicate usernames are rejected in legacy TOML format
#[tokio::test]
#[serial]
async fn test_toml_legacy_format_duplicate_username_rejected() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432

[pools.example_db.users.0]
username = "duplicate_user"
password = "password1"
pool_size = 30

[pools.example_db.users.1]
username = "duplicate_user"
password = "password2"
pool_size = 20
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    let result = parse(file_path).await;

    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("duplicate username"));
    } else {
        panic!("Expected BadConfig error about duplicate username");
    }
}

/// Test that duplicate usernames are rejected in new TOML array format
#[tokio::test]
#[serial]
async fn test_toml_new_format_duplicate_username_rejected() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432

[[pools.example_db.users]]
username = "duplicate_user"
password = "password1"
pool_size = 30

[[pools.example_db.users]]
username = "duplicate_user"
password = "password2"
pool_size = 20
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    let result = parse(file_path).await;

    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("duplicate username"));
    } else {
        panic!("Expected BadConfig error about duplicate username");
    }
}

/// Test YAML format with array users (for comparison with TOML formats)
#[tokio::test]
#[serial]
async fn test_yaml_array_users_format() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  example_db:
    server_host: "localhost"
    server_port: 5432
    users:
      - username: "yaml_user_1"
        password: "password1"
        pool_size: 35
      - username: "yaml_user_2"
        password: "password2"
        pool_size: 15
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.pools.len(), 1);
    assert_eq!(config.pools["example_db"].users.len(), 2);
    assert_eq!(config.pools["example_db"].users[0].username, "yaml_user_1");
    assert_eq!(config.pools["example_db"].users[0].pool_size, 35);
    assert_eq!(config.pools["example_db"].users[1].username, "yaml_user_2");
    assert_eq!(config.pools["example_db"].users[1].pool_size, 15);
}

/// Test that duplicate usernames are rejected in YAML format
#[tokio::test]
#[serial]
async fn test_yaml_duplicate_username_rejected() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  example_db:
    server_host: "localhost"
    server_port: 5432
    users:
      - username: "duplicate_user"
        password: "password1"
        pool_size: 30
      - username: "duplicate_user"
        password: "password2"
        pool_size: 20
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    let result = parse(file_path).await;

    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("duplicate username"));
    } else {
        panic!("Expected BadConfig error about duplicate username");
    }
}

// ============================================================
// auth_query config tests
// ============================================================

/// Parse YAML config with auth_query in dedicated mode (server_user set)
#[tokio::test]
#[serial]
async fn test_auth_query_yaml_dedicated_mode() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  mydb:
    server_host: "localhost"
    server_port: 5432
    auth_query:
      query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
      user: "pg_doorman_auth"
      password: "secret"
      database: "postgres"
      workers: 3
      server_user: "backend_user"
      server_password: "backend_pass"
      pool_size: 50
      cache_ttl: "2h"
      cache_failure_ttl: "1m"
      min_interval: "2s"
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    let pool = &config.pools["mydb"];
    let aq = pool.auth_query.as_ref().unwrap();

    assert_eq!(
        aq.query,
        "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
    );
    assert_eq!(aq.user, "pg_doorman_auth");
    assert_eq!(aq.password, "secret");
    assert_eq!(aq.database, Some("postgres".to_string()));
    assert_eq!(aq.workers, 3);
    assert_eq!(aq.server_user, Some("backend_user".to_string()));
    assert_eq!(aq.server_password, Some("backend_pass".to_string()));
    assert_eq!(aq.pool_size, 50);
    assert_eq!(aq.cache_ttl, Duration::from_hours(2));
    assert_eq!(aq.cache_failure_ttl, Duration::from_mins(1));
    assert_eq!(aq.min_interval, Duration::from_secs(2));
    assert!(aq.is_dedicated_mode());
}

/// Parse YAML config with auth_query in passthrough mode (no server_user)
#[tokio::test]
#[serial]
async fn test_auth_query_yaml_passthrough_mode() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  mydb:
    server_host: "localhost"
    server_port: 5432
    auth_query:
      query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
      user: "pg_doorman_auth"
      password: "secret"
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    let pool = &config.pools["mydb"];
    let aq = pool.auth_query.as_ref().unwrap();

    assert_eq!(aq.user, "pg_doorman_auth");
    assert_eq!(aq.server_user, None);
    assert_eq!(aq.server_password, None);
    assert!(!aq.is_dedicated_mode());
    // Verify defaults
    assert_eq!(aq.workers, 2);
    assert_eq!(aq.pool_size, 40);
    assert_eq!(aq.cache_ttl, Duration::from_hours(1));
    assert_eq!(aq.cache_failure_ttl, Duration::from_secs(30));
    assert_eq!(aq.min_interval, Duration::from_secs(1));
}

/// Parse YAML config without auth_query (backward compatibility)
#[tokio::test]
#[serial]
async fn test_auth_query_yaml_absent() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  mydb:
    server_host: "localhost"
    server_port: 5432
    users:
      - username: "user1"
        password: "pass1"
        pool_size: 10
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert!(config.pools["mydb"].auth_query.is_none());
}

/// Parse TOML config with auth_query section
#[tokio::test]
#[serial]
async fn test_auth_query_toml() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.mydb]
server_host = "localhost"
server_port = 5432

[pools.mydb.auth_query]
query = "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
user = "pg_doorman_auth"
password = "secret"
workers = 2
pool_size = 40
cache_ttl = 3600000
cache_failure_ttl = 30000
min_interval = 1000

[[pools.mydb.users]]
username = "static_user"
password = "static_pass"
pool_size = 10
"#;
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    let pool = &config.pools["mydb"];
    let aq = pool.auth_query.as_ref().unwrap();

    assert_eq!(
        aq.query,
        "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
    );
    assert_eq!(aq.user, "pg_doorman_auth");
    assert_eq!(aq.cache_ttl, Duration::from_millis(3600000));
    // Static users still work alongside auth_query
    assert_eq!(pool.users.len(), 1);
    assert_eq!(pool.users[0].username, "static_user");
}

/// Validation: empty query produces error
#[tokio::test]
async fn test_auth_query_validate_empty_query() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "".to_string(),
        user: "pg_doorman_auth".to_string(),
        password: "secret".to_string(),
        database: None,
        workers: 2,
        server_user: None,
        server_password: None,
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("auth_query.query cannot be empty"));
    }
}

/// Validation: empty user produces error
#[tokio::test]
async fn test_auth_query_validate_empty_user() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1".to_string(),
        user: "".to_string(),
        password: "secret".to_string(),
        database: None,
        workers: 2,
        server_user: None,
        server_password: None,
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("auth_query.user cannot be empty"));
    }
}

/// Validation: server_password without server_user produces error
#[tokio::test]
async fn test_auth_query_validate_server_password_without_server_user() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1".to_string(),
        user: "pg_doorman_auth".to_string(),
        password: "secret".to_string(),
        database: None,
        workers: 2,
        server_user: None,
        server_password: Some("orphan_password".to_string()),
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("server_password requires server_user"));
    }
}

/// Validation: server_user without server_password is valid (trust auth)
#[tokio::test]
async fn test_auth_query_validate_server_user_without_password_ok() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1".to_string(),
        user: "pg_doorman_auth".to_string(),
        password: String::new(),
        database: None,
        workers: 2,
        server_user: Some("backend_user".to_string()),
        server_password: None,
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_ok());
}

/// Validation: pool_size 0 produces error
#[tokio::test]
async fn test_auth_query_validate_pool_size_zero() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1".to_string(),
        user: "pg_doorman_auth".to_string(),
        password: "secret".to_string(),
        database: None,
        workers: 0,
        server_user: None,
        server_password: None,
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("auth_query.workers must be > 0"));
    }
}

/// Validation: empty password is valid (PostgreSQL trust auth for executor)
#[tokio::test]
async fn test_auth_query_validate_empty_password_ok() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1".to_string(),
        user: "pg_doorman_auth".to_string(),
        password: String::new(),
        database: None,
        workers: 2,
        server_user: None,
        server_password: None,
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_ok());
}

/// Validation: min_pool_size > pool_size must be rejected
#[tokio::test]
async fn test_auth_query_validate_min_pool_size_exceeds_pool_size() {
    let mut pool = Pool::default();
    pool.auth_query = Some(pool::AuthQueryConfig {
        query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1".to_string(),
        user: "pg_doorman_auth".to_string(),
        password: "secret".to_string(),
        database: None,
        workers: 2,
        server_user: None,
        server_password: None,
        pool_size: 5,
        min_pool_size: 10,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    });

    let result = pool.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(
            msg.contains("min_pool_size must be <= pool_size"),
            "unexpected error: {}",
            msg
        );
    }
}

// ============================================================
// Scaling config tests
// ============================================================

/// Test 1: Parsing YAML with general scaling fields
#[tokio::test]
#[serial]
async fn test_scaling_config_general_yaml() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"
  scaling_warm_pool_ratio: 30
  scaling_fast_retries: 20
  scaling_max_parallel_creates: 4

pools:
  mydb:
    server_host: "localhost"
    server_port: 5432
    users:
      - username: "user1"
        password: "pass1"
        pool_size: 10
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.general.scaling_warm_pool_ratio, 30);
    assert_eq!(config.general.scaling_fast_retries, 20);
    assert_eq!(config.general.scaling_max_parallel_creates, 4);
}

/// Test 2: Parsing defaults when scaling fields omitted
#[tokio::test]
#[serial]
async fn test_scaling_config_defaults() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  mydb:
    server_host: "localhost"
    server_port: 5432
    users:
      - username: "user1"
        password: "pass1"
        pool_size: 10
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.general.scaling_warm_pool_ratio, 20);
    assert_eq!(config.general.scaling_fast_retries, 10);
    assert_eq!(config.general.scaling_max_parallel_creates, 2);
    // Pool-level should be None
    let pool = &config.pools["mydb"];
    assert_eq!(pool.scaling_warm_pool_ratio, None);
    assert_eq!(pool.scaling_fast_retries, None);
}

/// Test 3: Pool-level override parsing
#[tokio::test]
#[serial]
async fn test_scaling_config_pool_override_yaml() {
    let config_content = r#"
general:
  host: "127.0.0.1"
  port: 6432
  admin_username: "admin"
  admin_password: "admin_password"

pools:
  overridden_db:
    server_host: "localhost"
    server_port: 5432
    scaling_warm_pool_ratio: 50
    scaling_fast_retries: 5
    users:
      - username: "user1"
        password: "pass1"
        pool_size: 10
  default_db:
    server_host: "localhost"
    server_port: 5432
    users:
      - username: "user2"
        password: "pass2"
        pool_size: 10
"#;
    let mut temp_file = NamedTempFile::with_suffix(".yaml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    let overridden = &config.pools["overridden_db"];
    assert_eq!(overridden.scaling_warm_pool_ratio, Some(50));
    assert_eq!(overridden.scaling_fast_retries, Some(5));

    let default = &config.pools["default_db"];
    assert_eq!(default.scaling_warm_pool_ratio, None);
    assert_eq!(default.scaling_fast_retries, None);
}

/// Test 4: resolve_scaling_config() — pool override wins
#[tokio::test]
async fn test_resolve_scaling_config_pool_override() {
    let mut general = General::default();
    general.scaling_warm_pool_ratio = 20;
    general.scaling_fast_retries = 10;
    general.scaling_max_parallel_creates = 2;

    let pool = Pool {
        scaling_warm_pool_ratio: Some(50),
        ..Pool::default()
    };

    let scaling = pool.resolve_scaling_config(&general);
    assert!((scaling.warm_pool_ratio - 0.5).abs() < f32::EPSILON);
    assert_eq!(scaling.fast_retries, 10); // general default
    assert_eq!(scaling.max_parallel_creates, 2); // global only
}

/// Test 5: resolve_scaling_config() — general fallback
#[tokio::test]
async fn test_resolve_scaling_config_general_fallback() {
    let mut general = General::default();
    general.scaling_warm_pool_ratio = 30;
    general.scaling_fast_retries = 15;
    general.scaling_max_parallel_creates = 3;

    let pool = Pool::default(); // all scaling fields are None

    let scaling = pool.resolve_scaling_config(&general);
    assert!((scaling.warm_pool_ratio - 0.3).abs() < f32::EPSILON);
    assert_eq!(scaling.fast_retries, 15);
    assert_eq!(scaling.max_parallel_creates, 3);
}

/// Test 5b: Validation rejects max_parallel_creates = 0 (would deadlock create path)
#[tokio::test]
async fn test_validate_scaling_max_parallel_creates_zero_rejected() {
    let mut config = Config::default();
    config.general.scaling_max_parallel_creates = 0;
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("scaling_max_parallel_creates"));
    } else {
        panic!("Expected BadConfig error about scaling_max_parallel_creates");
    }
}

/// Test 6: Validation — general warm_pool_ratio > 100
#[tokio::test]
async fn test_validate_scaling_warm_pool_ratio_general_out_of_range() {
    let mut config = Config::default();
    config.general.scaling_warm_pool_ratio = 150;
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("scaling_warm_pool_ratio"));
    } else {
        panic!("Expected BadConfig error about scaling_warm_pool_ratio");
    }
}

/// Test 7: Validation — pool warm_pool_ratio > 100
#[tokio::test]
async fn test_validate_scaling_warm_pool_ratio_pool_out_of_range() {
    let mut config = Config::default();
    let pool = Pool {
        scaling_warm_pool_ratio: Some(101),
        users: vec![User {
            username: "user1".to_string(),
            password: "pass1".to_string(),
            ..User::default()
        }],
        ..Pool::default()
    };
    config.pools.insert("testdb".to_string(), pool);

    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("scaling_warm_pool_ratio"));
    } else {
        panic!("Expected BadConfig error about scaling_warm_pool_ratio");
    }
}

/// Test 8: Hash changes when scaling config changes
#[test]
fn test_scaling_config_changes_pool_hash() {
    let pool_a = Pool {
        scaling_warm_pool_ratio: None,
        ..Pool::default()
    };
    let pool_b = Pool {
        scaling_warm_pool_ratio: Some(50),
        ..Pool::default()
    };
    assert_ne!(pool_a.hash_value(), pool_b.hash_value());
}

/// Test 9: TOML backward compatibility
#[tokio::test]
#[serial]
async fn test_scaling_config_toml_parsing() {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"
scaling_warm_pool_ratio = 40
scaling_fast_retries = 15
scaling_max_parallel_creates = 3

[pools.mydb]
server_host = "localhost"
server_port = 5432
scaling_warm_pool_ratio = 60

[[pools.mydb.users]]
username = "user1"
password = "pass1"
pool_size = 10
"#;
    let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let file_path = temp_file.path().to_str().unwrap();
    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.general.scaling_warm_pool_ratio, 40);
    assert_eq!(config.general.scaling_fast_retries, 15);
    assert_eq!(config.general.scaling_max_parallel_creates, 3);

    let pool = &config.pools["mydb"];
    assert_eq!(pool.scaling_warm_pool_ratio, Some(60));
    assert_eq!(pool.scaling_fast_retries, None);
}

/// Test 10: Edge case — warm_pool_ratio = 0 and 100
#[tokio::test]
async fn test_scaling_config_boundary_values() {
    let general = General::default();

    // warm_pool_ratio = 0 → valid, all connections go through cooldown
    let pool_zero = Pool {
        scaling_warm_pool_ratio: Some(0),
        ..Pool::default()
    };
    let mut pool_zero_for_validate = pool_zero.clone();
    assert!(pool_zero_for_validate.validate().await.is_ok());
    let scaling = pool_zero.resolve_scaling_config(&general);
    assert!((scaling.warm_pool_ratio - 0.0).abs() < f32::EPSILON);

    // warm_pool_ratio = 100 → valid, all connections created immediately
    let pool_hundred = Pool {
        scaling_warm_pool_ratio: Some(100),
        ..Pool::default()
    };
    let mut pool_hundred_for_validate = pool_hundred.clone();
    assert!(pool_hundred_for_validate.validate().await.is_ok());
    let scaling = pool_hundred.resolve_scaling_config(&general);
    assert!((scaling.warm_pool_ratio - 1.0).abs() < f32::EPSILON);
}

// --- Pool coordinator validation tests ---
// These validations produce warnings (log::warn), not errors.
// We verify that the config is accepted (Ok) despite the suboptimal settings.

#[tokio::test]
async fn test_validate_coordinator_sum_min_pool_size_exceeds_max() {
    let mut pool = Pool {
        max_db_connections: Some(10),
        users: vec![
            User {
                username: "u1".to_string(),
                password: "p1".to_string(),
                min_pool_size: Some(6),
                pool_size: 10,
                ..Default::default()
            },
            User {
                username: "u2".to_string(),
                password: "p2".to_string(),
                min_pool_size: Some(6),
                pool_size: 10,
                ..Default::default()
            },
        ],
        ..Pool::default()
    };
    // sum(min_pool_size) = 12 > max_db_connections = 10 → rejected
    let err = pool.validate().await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("sum of min_pool_size"),
        "error should mention min_pool_size sum: {msg}"
    );
}

#[tokio::test]
async fn test_validate_coordinator_user_pool_size_exceeds_max() {
    let mut pool = Pool {
        max_db_connections: Some(5),
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 20,
            ..Default::default()
        }],
        ..Pool::default()
    };
    // user.pool_size = 20 > max_db_connections = 5 → accepted with warning
    assert!(pool.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_coordinator_min_lifetime_exceeds_idle_timeout() {
    let mut pool = Pool {
        max_db_connections: Some(10),
        min_connection_lifetime: Some(30000),
        idle_timeout: Some(5000),
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 10,
            ..Default::default()
        }],
        ..Pool::default()
    };
    // min_connection_lifetime(30s) > idle_timeout(5s) → accepted with warning
    assert!(pool.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_coordinator_guaranteed_exceeds_pool_size() {
    let mut pool = Pool {
        max_db_connections: Some(10),
        min_guaranteed_pool_size: Some(8),
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 5,
            ..Default::default()
        }],
        ..Pool::default()
    };
    // min_guaranteed_pool_size(8) > pool_size(5) → accepted with warning
    assert!(pool.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_coordinator_disabled_skips_all_checks() {
    let mut pool = Pool {
        max_db_connections: Some(0), // disabled
        min_guaranteed_pool_size: Some(100),
        min_connection_lifetime: Some(999999),
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 50,
            min_pool_size: Some(50),
            ..Default::default()
        }],
        ..Pool::default()
    };
    // max_db_connections=0 → coordinator disabled, no warnings checked
    assert!(pool.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_coordinator_reserve_exceeds_max_db_connections_accepted_with_warning() {
    let mut pool = Pool {
        max_db_connections: Some(5),
        reserve_pool_size: Some(10), // 10 > 5 → warn but OK
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 5,
            ..Default::default()
        }],
        ..Pool::default()
    };
    // reserve_pool_size > max_db_connections → warning only, not error
    assert!(pool.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_reserve_pool_timeout_exceeds_query_wait_timeout_accepted_with_warning() {
    let mut config = Config::default();
    config.general.query_wait_timeout = Duration::from_millis(2000);

    let mut pool = Pool {
        max_db_connections: Some(10),
        reserve_pool_timeout: Some(5000), // 5000 > 2000 → warn but OK
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 10,
            ..Default::default()
        }],
        ..Pool::default()
    };
    pool.validate().await.unwrap();
    config.pools.insert("test_db".to_string(), pool);

    // Cross-config validation: reserve_pool_timeout > query_wait_timeout → accepted with warning
    assert!(config.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_reserve_pool_timeout_within_query_wait_timeout_no_warning() {
    let mut config = Config::default();
    config.general.query_wait_timeout = Duration::from_millis(5000);

    let mut pool = Pool {
        max_db_connections: Some(10),
        reserve_pool_timeout: Some(3000), // 3000 < 5000 → no warning
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 10,
            ..Default::default()
        }],
        ..Pool::default()
    };
    pool.validate().await.unwrap();
    config.pools.insert("test_db".to_string(), pool);

    assert!(config.validate().await.is_ok());
}

#[tokio::test]
async fn test_validate_reserve_pool_timeout_skipped_when_coordinator_disabled() {
    let mut config = Config::default();
    config.general.query_wait_timeout = Duration::from_millis(1000);

    let mut pool = Pool {
        max_db_connections: Some(0), // coordinator disabled
        reserve_pool_timeout: Some(9999),
        users: vec![User {
            username: "u1".to_string(),
            password: "p1".to_string(),
            pool_size: 10,
            ..Default::default()
        }],
        ..Pool::default()
    };
    pool.validate().await.unwrap();
    config.pools.insert("test_db".to_string(), pool);

    // Coordinator disabled → no cross-config check
    assert!(config.validate().await.is_ok());
}

// ---- check_hba_with_general: legacy general.hba + is_unix semantics ----

#[test]
fn check_hba_legacy_empty_allows_unix() {
    // Legacy branch, nothing configured: any transport is allowed. Kept as a
    // baseline so the next two tests document what changes once `is_unix`
    // enters the picture.
    let general = General::default();
    let ip: IpAddr = "10.0.0.5".parse().unwrap();
    assert_eq!(
        check_hba_with_general(&general, ip, false, true, "md5", "alice", "app"),
        CheckResult::Allow
    );
    assert_eq!(
        check_hba_with_general(&general, ip, false, false, "md5", "alice", "app"),
        CheckResult::Allow
    );
}

#[test]
fn check_hba_legacy_list_bypassed_for_unix() {
    // Reproduces the "silent privilege expansion" case from review: the
    // operator restricts TCP access with a CIDR whitelist, but Unix clients
    // must still be allowed because the legacy list has no transport concept.
    let mut general = General::default();
    general.hba = vec!["10.0.0.0/8".parse().unwrap()];
    let ip: IpAddr = "192.168.1.10".parse().unwrap();

    // Unix: Allow regardless of source IP
    assert_eq!(
        check_hba_with_general(&general, ip, false, true, "md5", "alice", "app"),
        CheckResult::Allow
    );
    // TCP from an IP outside the whitelist: NotMatched
    assert_eq!(
        check_hba_with_general(&general, ip, false, false, "md5", "alice", "app"),
        CheckResult::NotMatched
    );
    // TCP from an IP inside the whitelist: Allow
    let ip_inside: IpAddr = "10.1.2.3".parse().unwrap();
    assert_eq!(
        check_hba_with_general(&general, ip_inside, false, false, "md5", "alice", "app"),
        CheckResult::Allow
    );
}

#[test]
fn check_hba_pg_hba_takes_precedence_over_legacy_for_unix() {
    // When pg_hba is configured the legacy list must be ignored entirely;
    // `local` rules drive the decision for Unix clients.
    use crate::auth::hba::PgHba;
    let mut general = General::default();
    general.hba = vec!["10.0.0.0/8".parse().unwrap()];
    general.pg_hba = Some(PgHba::from_content("local all all reject"));
    let ip: IpAddr = "127.0.0.1".parse().unwrap();

    assert_eq!(
        check_hba_with_general(&general, ip, false, true, "md5", "alice", "app"),
        CheckResult::Deny
    );
}

// ---- legacy_hba_bypassed_by_unix_socket: silent privilege expansion detector ----

#[test]
fn legacy_hba_bypass_detected_when_unix_dir_set_and_legacy_hba_present() {
    let mut general = General::default();
    general.unix_socket_dir = Some("/tmp".to_string());
    general.hba = vec!["10.0.0.0/8".parse().unwrap()];
    assert!(legacy_hba_bypassed_by_unix_socket(&general));
}

#[test]
fn legacy_hba_bypass_quiet_without_unix_socket_dir() {
    let mut general = General::default();
    general.hba = vec!["10.0.0.0/8".parse().unwrap()];
    // No unix listener → operator's CIDR whitelist applies to every client.
    assert!(!legacy_hba_bypassed_by_unix_socket(&general));
}

#[test]
fn legacy_hba_bypass_quiet_without_legacy_entries() {
    let mut general = General::default();
    general.unix_socket_dir = Some("/tmp".to_string());
    // Empty legacy hba means there is no rule to bypass in the first place.
    assert!(!legacy_hba_bypassed_by_unix_socket(&general));
}

#[test]
fn legacy_hba_bypass_quiet_when_pg_hba_present() {
    use crate::auth::hba::PgHba;
    let mut general = General::default();
    general.unix_socket_dir = Some("/tmp".to_string());
    general.hba = vec!["10.0.0.0/8".parse().unwrap()];
    general.pg_hba = Some(PgHba::from_content("local all all trust"));
    // pg_hba takes precedence and has explicit local rules — no silent bypass.
    assert!(!legacy_hba_bypassed_by_unix_socket(&general));
}
