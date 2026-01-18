//! Tests for configuration module.

use super::*;
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
    assert_eq!(ConfigFormat::detect("/path/to/config.toml"), ConfigFormat::Toml);
    assert_eq!(ConfigFormat::detect("CONFIG.TOML"), ConfigFormat::Toml);
}

#[test]
fn test_config_format_detect_yaml() {
    assert_eq!(ConfigFormat::detect("config.yaml"), ConfigFormat::Yaml);
    assert_eq!(ConfigFormat::detect("config.yml"), ConfigFormat::Yaml);
    assert_eq!(ConfigFormat::detect("/path/to/config.yaml"), ConfigFormat::Yaml);
    assert_eq!(ConfigFormat::detect("/path/to/config.yml"), ConfigFormat::Yaml);
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
async fn test_yaml_config_parsing() {
    let temp_file = create_temp_yaml_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();

    let config = get_config();
    assert_eq!(config.pools.len(), 1);
    assert_eq!(config.pools["example_db"].idle_timeout, Some(40000));
    assert_eq!(config.pools["example_db"].users[0].username, "example_user_1");
    assert_eq!(config.pools["example_db"].users[0].pool_size, 40);
    assert_eq!(
        config.pools["example_db"].users[0].pool_mode,
        Some(PoolMode::Transaction)
    );
    assert_eq!(config.pools["example_db"].users[1].username, "example_user_2");
    assert_eq!(config.pools["example_db"].users[1].pool_size, 20);
}

#[tokio::test]
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
    let result: GeneralWithInclude = parse_config_content(toml_content, ConfigFormat::Toml).unwrap();
    assert!(result.include.files.is_empty());
}

#[test]
fn test_parse_config_content_yaml() {
    let yaml_content = r#"
include:
  files: []
"#;
    let result: GeneralWithInclude = parse_config_content(yaml_content, ConfigFormat::Yaml).unwrap();
    assert!(result.include.files.is_empty());
}

// ============================================================================
// TOML Backward Compatibility Tests
// ============================================================================
// These tests verify that the old TOML format [pools.*.users.0] continues to work
// after the migration to the new array format [[pools.*.users]]

/// Test parsing legacy TOML format with [pools.*.users.0] syntax
#[tokio::test]
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
    assert_eq!(config.pools["example_db"].users[0].username, "legacy_user_1");
    assert_eq!(config.pools["example_db"].users[0].pool_size, 30);
    assert_eq!(config.pools["example_db"].users[1].username, "legacy_user_2");
    assert_eq!(config.pools["example_db"].users[1].pool_size, 20);
}

/// Test parsing new TOML format with [[pools.*.users]] syntax
#[tokio::test]
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
    assert_eq!(user.server_password, Some("real_server_password".to_string()));
}

/// Test that duplicate usernames are rejected in legacy TOML format
#[tokio::test]
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
