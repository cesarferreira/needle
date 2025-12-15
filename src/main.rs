mod db;
mod demo;
mod github;
mod model;
mod refresh;
mod timeutil;
mod tui;

use crate::db::{db_path, open_db};
use crate::refresh::{load_cached, refresh, refresh_demo, ScopeFilters};
use crate::tui::{AppState, UiPrefs, run_tui};
use clap::Parser;
use octocrab::Octocrab;
use std::sync::Arc;

#[derive(Parser, Debug, Clone)]
#[command(name = "needle", version, about = "TUI PR triage for GitHub")]
struct CliArgs {
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
    let days = args.days;
    let scope = ScopeFilters {
        orgs: args.org.clone(),
        include_repos: args.include.clone(),
        exclude_repos: args.exclude.clone(),
    };
    let ui = UiPrefs {
        hide_pr_numbers: args.hide_pr_numbers,
        hide_repo: args.hide_repo,
        hide_author: args.hide_author,
    };

    if args.demo {
        let demo_path = std::path::PathBuf::from("target/needle-demo/prs.sqlite");
        let conn = open_db(&demo_path).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });

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

        if let Err(e) = run_tui(&conn, state, refresh_fn, false, args.bell) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    if std::env::var("GITHUB_TOKEN").is_err() {
        eprintln!("Missing GITHUB_TOKEN env var");
        std::process::exit(1);
    }
    let token = std::env::var("GITHUB_TOKEN").unwrap();

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
    let conn = open_db(&path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    // Fast startup: render cached SQLite snapshot immediately, then refresh in background.
    let cached = load_cached(&conn, days, &scope).unwrap_or_else(|_e| Vec::new());
    let state = AppState::new(cached, ui);

    let handle = tokio::runtime::Handle::current();
    let db_path_for_refresh = path.clone();
    let octo_for_refresh = octo.clone();
    let handle_for_refresh = handle.clone();
    let scope_for_refresh = scope.clone();
    let include_team_requests = args.include_team_requests;
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

    if let Err(e) = run_tui(&conn, state, refresh_fn, true, args.bell) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
