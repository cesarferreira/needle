//! Configuration file support for needle.
//!
//! Loads settings from `~/.config/needle/config.toml` (or platform equivalent).
//! CLI arguments take precedence over config file values.

use serde::Deserialize;
use std::path::PathBuf;

/// Configuration loaded from TOML file.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Only include PRs updated in the last N days.
    pub days: Option<i64>,

    /// Only show PRs from these orgs/users.
    pub org: Option<Vec<String>>,

    /// Only show these repos (owner/repo).
    pub include: Option<Vec<String>>,

    /// Exclude these repos (owner/repo).
    pub exclude: Option<Vec<String>>,

    /// Include PRs requested to teams you are in.
    pub include_team_requests: Option<bool>,

    /// Emit a terminal bell on important new events.
    pub bell: Option<bool>,

    /// Disable OS desktop notifications.
    pub no_notifications: Option<bool>,

    /// Hide PR numbers column in list view.
    pub hide_pr_numbers: Option<bool>,

    /// Hide repository column in list view.
    pub hide_repo: Option<bool>,

    /// Hide author column in list view.
    pub hide_author: Option<bool>,

    /// Auto-refresh interval in list view (seconds). Default: 180 (3 minutes).
    pub refresh_interval_list_secs: Option<u64>,

    /// Auto-refresh interval in details view (seconds). Default: 30.
    pub refresh_interval_details_secs: Option<u64>,
}

/// Returns the path to the config file.
/// Platform-specific: `~/.config/needle/config.toml` on Linux/macOS,
/// `%APPDATA%\needle\config.toml` on Windows.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("needle").join("config.toml"))
}

/// Default config file content with all options documented.
const DEFAULT_CONFIG: &str = r#"# Needle configuration file
# All fields are optional - CLI arguments override these values
# Uncomment and modify the options you want to customize

# Only include PRs updated in the last N days (default: 30)
# days = 30

# Only show PRs from these orgs/users
# org = ["my-company", "my-username"]

# Only show these specific repos (owner/repo)
# include = ["my-company/important-repo"]

# Exclude these repos from the list (owner/repo)
# exclude = ["my-company/noisy-repo", "my-company/legacy-repo"]

# Include PRs where review is requested from teams you're in (default: false)
# include_team_requests = false

# Ring terminal bell on important events (default: false)
# bell = false

# Disable desktop notifications (default: false, i.e. notifications enabled)
# no_notifications = false

# Hide columns in list view
# hide_pr_numbers = false
# hide_repo = false
# hide_author = false

# Auto-refresh intervals in seconds
# refresh_interval_list_secs = 180    # 3 minutes for list view
# refresh_interval_details_secs = 30  # 30 seconds for details view
"#;

/// Create the default config file if it doesn't exist.
fn create_default_config(path: &PathBuf) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }
    std::fs::write(path, DEFAULT_CONFIG)
        .map_err(|e| format!("Failed to write config file: {e}"))?;
    eprintln!("Created default config file at {}", path.display());
    Ok(())
}

/// Load configuration from the config file.
/// Creates a default config file if it doesn't exist.
/// Returns default config if the file can't be parsed.
pub fn load_config() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };

    if !path.exists() {
        // Create default config file
        if let Err(e) = create_default_config(&path) {
            eprintln!("Warning: {e}");
        }
        return Config::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(config) => config,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to parse config file at {}: {}",
                    path.display(),
                    e
                );
                Config::default()
            }
        },
        Err(e) => {
            eprintln!(
                "Warning: Failed to read config file at {}: {}",
                path.display(),
                e
            );
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.days.is_none());
        assert!(config.org.is_none());
    }

    #[test]
    fn test_parse_full_config() {
        let toml_str = r#"
days = 14
org = ["my-company", "other-org"]
include = ["my-company/important-repo"]
exclude = ["my-company/legacy-repo"]
include_team_requests = true
bell = true
no_notifications = false
hide_pr_numbers = false
hide_repo = false
hide_author = true
refresh_interval_list_secs = 120
refresh_interval_details_secs = 15
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.days, Some(14));
        assert_eq!(
            config.org,
            Some(vec!["my-company".to_string(), "other-org".to_string()])
        );
        assert_eq!(
            config.include,
            Some(vec!["my-company/important-repo".to_string()])
        );
        assert_eq!(
            config.exclude,
            Some(vec!["my-company/legacy-repo".to_string()])
        );
        assert_eq!(config.include_team_requests, Some(true));
        assert_eq!(config.bell, Some(true));
        assert_eq!(config.no_notifications, Some(false));
        assert_eq!(config.hide_author, Some(true));
        assert_eq!(config.refresh_interval_list_secs, Some(120));
        assert_eq!(config.refresh_interval_details_secs, Some(15));
    }

    #[test]
    fn test_parse_partial_config() {
        let toml_str = r#"
days = 7
bell = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.days, Some(7));
        assert_eq!(config.bell, Some(true));
        assert!(config.org.is_none());
        assert!(config.include.is_none());
    }

    #[test]
    fn test_default_config_is_valid_toml() {
        // The DEFAULT_CONFIG constant should be valid TOML
        let result: Result<Config, _> = toml::from_str(DEFAULT_CONFIG);
        assert!(
            result.is_ok(),
            "DEFAULT_CONFIG should be valid TOML: {:?}",
            result.err()
        );

        // Since all options are commented out, the parsed config should have all None values
        let config = result.unwrap();
        assert!(config.days.is_none());
        assert!(config.org.is_none());
        assert!(config.include.is_none());
        assert!(config.exclude.is_none());
        assert!(config.include_team_requests.is_none());
        assert!(config.bell.is_none());
        assert!(config.no_notifications.is_none());
        assert!(config.hide_pr_numbers.is_none());
        assert!(config.hide_repo.is_none());
        assert!(config.hide_author.is_none());
        assert!(config.refresh_interval_list_secs.is_none());
        assert!(config.refresh_interval_details_secs.is_none());
    }

    #[test]
    fn test_default_config_contains_all_options() {
        // Verify that DEFAULT_CONFIG documents all available options
        assert!(
            DEFAULT_CONFIG.contains("days"),
            "DEFAULT_CONFIG should document 'days' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("org"),
            "DEFAULT_CONFIG should document 'org' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("include"),
            "DEFAULT_CONFIG should document 'include' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("exclude"),
            "DEFAULT_CONFIG should document 'exclude' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("include_team_requests"),
            "DEFAULT_CONFIG should document 'include_team_requests' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("bell"),
            "DEFAULT_CONFIG should document 'bell' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("no_notifications"),
            "DEFAULT_CONFIG should document 'no_notifications' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("hide_pr_numbers"),
            "DEFAULT_CONFIG should document 'hide_pr_numbers' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("hide_repo"),
            "DEFAULT_CONFIG should document 'hide_repo' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("hide_author"),
            "DEFAULT_CONFIG should document 'hide_author' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("refresh_interval_list_secs"),
            "DEFAULT_CONFIG should document 'refresh_interval_list_secs' option"
        );
        assert!(
            DEFAULT_CONFIG.contains("refresh_interval_details_secs"),
            "DEFAULT_CONFIG should document 'refresh_interval_details_secs' option"
        );
    }

    #[test]
    fn test_create_default_config_creates_file() {
        // Create a temporary directory for the test
        let temp_dir = std::env::temp_dir().join(format!(
            "needle-test-create-config-{}",
            std::process::id()
        ));
        let config_path = temp_dir.join("config.toml");

        // Clean up any previous test run
        let _ = fs::remove_dir_all(&temp_dir);

        // Create the config file
        let result = create_default_config(&config_path);
        assert!(result.is_ok(), "create_default_config should succeed");

        // Verify the file exists
        assert!(config_path.exists(), "Config file should be created");

        // Verify the contents match DEFAULT_CONFIG
        let contents = fs::read_to_string(&config_path).unwrap();
        assert_eq!(contents, DEFAULT_CONFIG);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_create_default_config_creates_parent_directories() {
        // Create a deeply nested path
        let temp_dir = std::env::temp_dir().join(format!(
            "needle-test-nested-{}/deep/nested/path",
            std::process::id()
        ));
        let config_path = temp_dir.join("config.toml");

        // Clean up any previous test run
        let base_dir = std::env::temp_dir().join(format!("needle-test-nested-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base_dir);

        // Create the config file (should create all parent directories)
        let result = create_default_config(&config_path);
        assert!(
            result.is_ok(),
            "create_default_config should create parent directories"
        );

        // Verify the file exists
        assert!(config_path.exists(), "Config file should be created");

        // Clean up
        let _ = fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn test_load_config_from_existing_file() {
        // Create a temporary config file with custom values
        let temp_dir = std::env::temp_dir().join(format!(
            "needle-test-load-existing-{}",
            std::process::id()
        ));
        let config_path = temp_dir.join("config.toml");

        // Clean up any previous test run
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Write a custom config
        let custom_config = r#"
days = 14
bell = true
org = ["test-org"]
"#;
        fs::write(&config_path, custom_config).unwrap();

        // Load and parse the config
        let contents = fs::read_to_string(&config_path).unwrap();
        let config: Config = toml::from_str(&contents).unwrap();

        assert_eq!(config.days, Some(14));
        assert_eq!(config.bell, Some(true));
        assert_eq!(config.org, Some(vec!["test-org".to_string()]));

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_config_with_uncommented_default_values() {
        // Test parsing a config where the user has uncommented some default values
        let toml_str = r#"
# User uncommented these lines from the default config
days = 30
refresh_interval_list_secs = 180
refresh_interval_details_secs = 30
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.days, Some(30));
        assert_eq!(config.refresh_interval_list_secs, Some(180));
        assert_eq!(config.refresh_interval_details_secs, Some(30));
    }

    #[test]
    fn test_config_with_empty_arrays() {
        // Test that empty arrays are handled correctly
        let toml_str = r#"
org = []
include = []
exclude = []
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.org, Some(vec![]));
        assert_eq!(config.include, Some(vec![]));
        assert_eq!(config.exclude, Some(vec![]));
    }

    #[test]
    fn test_config_ignores_unknown_fields() {
        // Test that unknown fields in the config file are ignored (forward compatibility)
        let toml_str = r#"
days = 7
unknown_future_option = true
another_unknown = ["value"]
"#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        // With #[serde(default)], unknown fields should be ignored
        assert!(result.is_ok(), "Config should ignore unknown fields");
        let config = result.unwrap();
        assert_eq!(config.days, Some(7));
    }
}

