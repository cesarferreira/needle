mod db;
mod github;
mod model;
mod refresh;
mod timeutil;
mod tui;

use crate::db::{db_path, open_db};
use crate::refresh::refresh;
use crate::tui::{AppState, run_tui};
use octocrab::Octocrab;

fn parse_days_arg() -> i64 {
    // Default: last 30 days.
    let mut days: i64 = 30;
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
            "--help" | "-h" => {
                println!("needle\n\nUSAGE:\n  needle [--days <N>]\n\nOPTIONS:\n  --days <N>   Only include PRs updated in the last N days (default: 30)\n  -h, --help   Print help\n");
                std::process::exit(0);
            }
            _ => {
                // Ignore unknown args in V1.
                i += 1;
            }
        }
    }
    days
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if std::env::var("GITHUB_TOKEN").is_err() {
        eprintln!("Missing GITHUB_TOKEN env var");
        std::process::exit(1);
    }
    let days = parse_days_arg();
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

    let initial = refresh(&conn, &octo, days).await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    let state = AppState::new(initial);

    let handle = tokio::runtime::Handle::current();
    if let Err(e) = run_tui(&conn, state, || {
        tokio::task::block_in_place(|| handle.block_on(refresh(&conn, &octo, days)))
    }) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
