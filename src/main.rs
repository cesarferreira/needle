mod db;
mod demo;
mod github;
mod model;
mod refresh;
mod timeutil;
mod tui;

use crate::db::{db_path, open_db};
use crate::refresh::{load_cached, refresh, refresh_demo};
use crate::tui::{AppState, run_tui};
use octocrab::Octocrab;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
struct CliArgs {
    days: i64,
    demo: bool,
}

fn parse_args() -> CliArgs {
    // Default: last 30 days.
    let mut days: i64 = 30;
    let mut demo = false;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--days" => {
                if i + 1 >= args.len() {
                    eprintln!("Missing value for --days");
                    std::process::exit(2);
                }
                days = args[i + 1].parse::<i64>().unwrap_or_else(|_| {
                    eprintln!("Invalid --days value: {}", args[i + 1]);
                    std::process::exit(2);
                });
                if days < 0 {
                    eprintln!("--days must be >= 0");
                    std::process::exit(2);
                }
                i += 2;
            }
            "--demo" => {
                demo = true;
                i += 1;
            }
            "--help" | "-h" => {
                println!(
                    "needle\n\nUSAGE:\n  needle [--days <N>] [--demo]\n\nOPTIONS:\n  --days <N>   Only include PRs updated in the last N days (default: 30)\n  --demo       Start with diverse fake data (no GitHub token required)\n  -h, --help   Print help\n"
                );
                std::process::exit(0);
            }
            _ => {
                // Ignore unknown args in V1.
                i += 1;
            }
        }
    }
    CliArgs { days, demo }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args = parse_args();
    let days = args.days;

    if args.demo {
        let demo_path = std::path::PathBuf::from("target/needle-demo/prs.sqlite");
        let conn = open_db(&demo_path).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });

        // Seed once, then run again so some CI failures look "unchanged" on first render.
        let _ = refresh_demo(&conn, days);
        let demo_prs = refresh_demo(&conn, days).unwrap_or_else(|_e| Vec::new());
        let state = AppState::new(demo_prs);

        let demo_path_for_refresh = demo_path.clone();
        let refresh_fn: Arc<dyn Fn() -> Result<Vec<crate::refresh::UiPr>, String> + Send + Sync> =
            Arc::new(move || {
                let c = open_db(&demo_path_for_refresh)?;
                refresh_demo(&c, days)
            });

        if let Err(e) = run_tui(&conn, state, refresh_fn, false) {
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
    let cached = load_cached(&conn, days).unwrap_or_else(|_e| Vec::new());
    let state = AppState::new(cached);

    let handle = tokio::runtime::Handle::current();
    let db_path_for_refresh = path.clone();
    let octo_for_refresh = octo.clone();
    let handle_for_refresh = handle.clone();
    let refresh_fn: Arc<dyn Fn() -> Result<Vec<crate::refresh::UiPr>, String> + Send + Sync> =
        Arc::new(move || {
            let c = open_db(&db_path_for_refresh)?;
            // Called from a non-runtime worker thread (for shimmer), so this uses handle.block_on.
            if tokio::runtime::Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| handle_for_refresh.block_on(refresh(&c, &octo_for_refresh, days)))
            } else {
                handle_for_refresh.block_on(refresh(&c, &octo_for_refresh, days))
            }
        });

    if let Err(e) = run_tui(&conn, state, refresh_fn, true) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
