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
}

