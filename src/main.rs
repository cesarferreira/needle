mod config;
mod db;
mod demo;
mod github;
mod model;
mod notify;
mod refresh;
mod timeutil;
mod tui;

use crate::config::load_config;
use crate::db::{db_path, delete_prs_not_in, open_db};
use crate::refresh::{ScopeFilters, load_cached, refresh, refresh_demo};
use crate::tui::{AppState, RefreshIntervals, UiPrefs, run_tui};
use clap::{ArgAction, Parser};
use octocrab::Octocrab;
use std::sync::Arc;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "needle",
    version,
    about = "TUI PR triage for GitHub",
    disable_version_flag = true
)]
struct CliArgs {
    /// Print version information (-v, -V, --version).
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    version: (),

    /// Skip loading cached PRs on startup (start empty, rely on refresh).
    #[arg(long = "no-cache")]
    no_cache: bool,

    /// Delete the cache database before starting (also applies to --demo path).
    #[arg(long = "purge-cache")]
    purge_cache: bool,

    /// Only include PRs updated in the last N days.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(i64).range(0..))]
    days: i64,

    /// Start with diverse fake data (no GitHub token required).
    #[arg(long)]
    demo: bool,

    /// Only show PRs from these orgs/users (repeatable or comma-delimited).
    #[arg(long, value_delimiter = ',', num_args = 0..)]
    org: Vec<String>,

    /// Only show these repos (owner/repo) (repeatable or comma-delimited).
    #[arg(long, value_delimiter = ',', num_args = 0..)]
    include: Vec<String>,

    /// Exclude these repos (owner/repo) (repeatable or comma-delimited).
    #[arg(long, value_delimiter = ',', num_args = 0..)]
    exclude: Vec<String>,

    /// Include PRs requested to teams you are in (default: only explicit user requests).
    #[arg(long)]
    include_team_requests: bool,

    /// Emit a terminal bell on important new events.
    #[arg(long)]
    bell: bool,

    /// Disable OS desktop notifications on important new events.
    #[arg(long)]
    no_notifications: bool,

    /// Hide PR numbers column in list view.
    #[arg(long)]
    hide_pr_numbers: bool,

    /// Hide repository column in list view.
    #[arg(long)]
    hide_repo: bool,

    /// Hide author column in list view.
    #[arg(long)]
    hide_author: bool,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args = CliArgs::parse();
    let config = load_config();

    // Merge config with CLI args (CLI takes precedence).
    // For days, only use config if CLI is at default (30).
    let days = if args.days != 30 {
        args.days
    } else {
        config.days.unwrap_or(30)
    };

    // For Vec fields, use CLI if non-empty, otherwise config.
    let orgs = if !args.org.is_empty() {
        args.org.clone()
    } else {
        config.org.unwrap_or_default()
    };
    let include_repos = if !args.include.is_empty() {
        args.include.clone()
    } else {
        config.include.unwrap_or_default()
    };
    let exclude_repos = if !args.exclude.is_empty() {
        args.exclude.clone()
    } else {
        config.exclude.unwrap_or_default()
    };

    let scope = ScopeFilters {
        orgs,
        include_repos,
        exclude_repos,
    };

    // For boolean flags, CLI true overrides config; otherwise use config value.
    let include_team_requests =
        args.include_team_requests || config.include_team_requests.unwrap_or(false);
    let bell_enabled = args.bell || config.bell.unwrap_or(false);
    let notify_enabled =
        !(args.no_notifications || config.no_notifications.unwrap_or(false));

    let ui = UiPrefs {
        hide_pr_numbers: args.hide_pr_numbers || config.hide_pr_numbers.unwrap_or(false),
        hide_repo: args.hide_repo || config.hide_repo.unwrap_or(false),
        hide_author: args.hide_author || config.hide_author.unwrap_or(false),
    };

    let refresh_intervals = RefreshIntervals {
        list_secs: config.refresh_interval_list_secs.unwrap_or(180),
        details_secs: config.refresh_interval_details_secs.unwrap_or(30),
    };

    if args.demo {
        let demo_path = std::path::PathBuf::from("target/needle-demo/prs.sqlite");
        if args.purge_cache {
            let _ = std::fs::remove_file(&demo_path);
        }
        let conn = open_db(&demo_path).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });

        if args.no_cache {
            let _ = delete_prs_not_in(&conn, &[]);
        }

        // Seed once, then run again so some CI failures look "unchanged" on first render.
        let _ = refresh_demo(&conn, days, &scope);
        let demo_prs = refresh_demo(&conn, days, &scope).unwrap_or_else(|_e| Vec::new());
        let state = AppState::new(demo_prs, ui);

        let demo_path_for_refresh = demo_path.clone();
        let scope_for_refresh = scope.clone();
        let refresh_fn: Arc<dyn Fn() -> Result<Vec<crate::refresh::UiPr>, String> + Send + Sync> =
            Arc::new(move || {
                let c = open_db(&demo_path_for_refresh)?;
                refresh_demo(&c, days, &scope_for_refresh)
            });

        if let Err(e) = run_tui(&conn, state, refresh_fn, false, bell_enabled, notify_enabled, true, refresh_intervals) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    let token = std::env::var("NEEDLE_GITHUB_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .unwrap_or_else(|_| {
            eprintln!("Missing NEEDLE_GITHUB_TOKEN or GITHUB_TOKEN env var");
            std::process::exit(1);
        });

    let octo = Octocrab::builder()
        .personal_token(token)
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Failed to init octocrab: {e}");
            std::process::exit(1);
        });

    let path = db_path().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    if args.purge_cache {
        let _ = std::fs::remove_file(&path);
    }
    let conn = open_db(&path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    if args.no_cache {
        let _ = delete_prs_not_in(&conn, &[]);
    }

    // Fast startup: render cached SQLite snapshot immediately, then refresh in background.
    let cached = if args.no_cache {
        Vec::new()
    } else {
        load_cached(&conn, days, &scope).unwrap_or_else(|_e| Vec::new())
    };
    let state = AppState::new(cached, ui);

    let handle = tokio::runtime::Handle::current();
    let db_path_for_refresh = path.clone();
    let octo_for_refresh = octo.clone();
    let handle_for_refresh = handle.clone();
    let scope_for_refresh = scope.clone();
    let refresh_fn: Arc<dyn Fn() -> Result<Vec<crate::refresh::UiPr>, String> + Send + Sync> =
        Arc::new(move || {
            let c = open_db(&db_path_for_refresh)?;
            // Called from a non-runtime worker thread (for shimmer), so this uses handle.block_on.
            if tokio::runtime::Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| {
                    handle_for_refresh.block_on(refresh(
                        &c,
                        &octo_for_refresh,
                        days,
                        &scope_for_refresh,
                        include_team_requests,
                    ))
                })
            } else {
                handle_for_refresh.block_on(refresh(
                    &c,
                    &octo_for_refresh,
                    days,
                    &scope_for_refresh,
                    include_team_requests,
                ))
            }
        });

    if let Err(e) = run_tui(&conn, state, refresh_fn, true, bell_enabled, notify_enabled, false, refresh_intervals) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
