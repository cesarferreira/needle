use crate::db::{DbPrRow, delete_prs_not_in, load_all_prs, now_unix, upsert_pr};
use crate::github::fetch_attention_prs;
use crate::model::{CiState, Pr, ReviewState};
use octocrab::Octocrab;
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    NeedsYou,
    Waiting,
    Stale,
}

#[derive(Debug, Clone)]
pub struct UiPr {
    pub pr: Pr,
    pub score: i32,
    pub category: Category,
    pub display_status: String,
    pub last_opened_at: Option<i64>,
}

fn parse_ci_state(s: Option<&str>) -> CiState {
    match s {
        Some("success") => CiState::Success,
        Some("failure") => CiState::Failure,
        Some("running") => CiState::Running,
        Some("none") | None => CiState::None,
        _ => CiState::None,
    }
}

fn parse_review_state(s: Option<&str>) -> ReviewState {
    match s {
        Some("requested") => ReviewState::Requested,
        Some("approved") => ReviewState::Approved,
        Some("none") | None => ReviewState::None,
        _ => ReviewState::None,
    }
}

/// Load cached PRs from SQLite for a fast startup render (no network).
/// Note: DB does not store GitHub updatedAt; we approximate with last_seen_at for ordering/age.
pub fn load_cached(conn: &Connection, cutoff_days: i64) -> Result<Vec<UiPr>, String> {
    let existing: HashMap<String, DbPrRow> = load_all_prs(conn)?;
    let now = now_unix();
    let cutoff_ts = now.saturating_sub(cutoff_days.saturating_mul(86_400));

    let mut out: Vec<UiPr> = Vec::new();
    for (_k, row) in existing {
        let updated_at_unix = row.last_seen_at.unwrap_or(now);
        if updated_at_unix < cutoff_ts {
            continue;
        }
        let pr = Pr {
            pr_key: row.pr_key.clone(),
            owner: row.owner.clone(),
            repo: row.repo.clone(),
            number: row.number,
            author: "unknown".to_string(),
            title: row.title.clone(),
            url: row.url.clone(),
            updated_at_unix,
            last_commit_sha: row.last_commit_sha.clone(),
            ci_state: parse_ci_state(row.last_ci_state.as_deref()),
            ci_checks: Vec::new(),
            review_state: parse_review_state(row.last_review_state.as_deref()),
        };

        let is_new_review = false;
        let is_new_ci_failure = false;
        let score = score_pr(&pr, None, now, is_new_ci_failure);
        let category = category_for(score);
        let display_status = status_text(&pr, now, is_new_ci_failure, is_new_review);

        out.push(UiPr {
            pr,
            score,
            category,
            display_status,
            last_opened_at: row.last_opened_at,
        });
    }

    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.pr.updated_at_unix.cmp(&a.pr.updated_at_unix))
    });
    Ok(out)
}

fn ci_to_db(ci: &CiState) -> &'static str {
    ci.as_str()
}

fn review_to_db(r: &ReviewState) -> &'static str {
    r.as_str()
}

fn score_pr(pr: &Pr, old: Option<&DbPrRow>, now: i64, is_new_ci_failure: bool) -> i32 {
    let mut score = 0;

    // +50  review requested from user
    if matches!(pr.review_state, ReviewState::Requested) {
        score += 50;
    }

    // CI failure scoring
    if matches!(pr.ci_state, CiState::Failure) {
        if is_new_ci_failure {
            // +40  CI failed AND state changed since last_seen (or commit changed)
            score += 40;
        } else {
            // -30  CI failed but unchanged since last_seen
            score -= 30;
        }
    }

    // +20  CI running longer than 10 minutes (using updatedAt proxy)
    if matches!(pr.ci_state, CiState::Running) {
        if now.saturating_sub(pr.updated_at_unix) > 10 * 60 {
            score += 20;
        }
    }

    // +15 approved but unmerged for >24h
    if matches!(pr.review_state, ReviewState::Approved) {
        if now.saturating_sub(pr.updated_at_unix) > 24 * 3600 {
            score += 15;
        }
    }

    // -20 waiting on others (no review requested, CI green)
    if !matches!(pr.review_state, ReviewState::Requested) && matches!(pr.ci_state, CiState::Success) {
        score -= 20;
    }

    // Note: `old` currently unused beyond is_new_ci_failure; keep signature stable for V1.
    let _ = old;
    score
}

fn category_for(score: i32) -> Category {
    if score >= 40 {
        Category::NeedsYou
    } else if score >= 0 {
        Category::Waiting
    } else {
        Category::Stale
    }
}

fn human_age(now: i64, then: i64) -> String {
    let d = now.saturating_sub(then);
    if d < 60 {
        "now".to_string()
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}

fn status_text(pr: &Pr, now: i64, is_new_ci_failure: bool, is_new_review_request: bool) -> String {
    if is_new_review_request && matches!(pr.review_state, ReviewState::Requested) {
        return "👀 review requested".to_string();
    }

    match pr.ci_state {
        CiState::Failure => {
            if is_new_ci_failure {
                "❌ CI failed (new)".to_string()
            } else {
                "❌ CI failed".to_string()
            }
        }
        CiState::Running => {
            let mins = now.saturating_sub(pr.updated_at_unix) / 60;
            format!("🟡 CI running ({}m)", mins)
        }
        CiState::Success => format!("✅ green {}", human_age(now, pr.updated_at_unix)),
        CiState::None => format!("⏺ none {}", human_age(now, pr.updated_at_unix)),
    }
}

fn is_new_review_request(pr: &Pr, old: Option<&DbPrRow>) -> bool {
    if !matches!(pr.review_state, ReviewState::Requested) {
        return false;
    }
    let Some(old) = old else { return true };
    old.last_review_state.as_deref() != Some("requested")
}

fn is_new_ci_failure(pr: &Pr, old: Option<&DbPrRow>) -> bool {
    if !matches!(pr.ci_state, CiState::Failure) {
        return false;
    }
    let Some(old) = old else { return true };
    let old_ci = old.last_ci_state.as_deref();
    let old_sha = old.last_commit_sha.as_deref();
    let new_sha = pr.last_commit_sha.as_deref();
    let commit_changed = old_sha != new_sha;
    // "CI failed AND state changed since last_seen" OR "New commit pushed (resets CI relevance)"
    old_ci != Some("failure") || commit_changed
}

pub async fn refresh(conn: &Connection, octo: &Octocrab, cutoff_days: i64) -> Result<Vec<UiPr>, String> {
    let existing: HashMap<String, DbPrRow> = load_all_prs(conn)?;
    let now = now_unix();

    let mut out: Vec<UiPr> = Vec::new();

    let cutoff_ts = now.saturating_sub(cutoff_days.saturating_mul(86_400));
    let prs = fetch_attention_prs(octo, cutoff_ts).await?;

    let prs: Vec<Pr> = prs.into_iter().filter(|p| p.updated_at_unix >= cutoff_ts).collect();
    let keep_keys: Vec<String> = prs.iter().map(|p| p.pr_key.clone()).collect();

    for pr in prs {
        let old = existing.get(&pr.pr_key);
        let new_review = is_new_review_request(&pr, old);
        let new_ci_failure = is_new_ci_failure(&pr, old);

        let last_opened_at = old.and_then(|r| r.last_opened_at);

        let db_row = DbPrRow {
            pr_key: pr.pr_key.clone(),
            owner: pr.owner.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            last_commit_sha: pr.last_commit_sha.clone(),
            last_ci_state: Some(ci_to_db(&pr.ci_state).to_string()),
            last_review_state: Some(review_to_db(&pr.review_state).to_string()),
            last_seen_at: Some(now),
            last_opened_at,
        };
        upsert_pr(conn, &db_row, now)?;

        let score = score_pr(&pr, old, now, new_ci_failure);
        let category = category_for(score);
        let display_status = status_text(&pr, now, new_ci_failure, new_review);

        out.push(UiPr {
            pr,
            score,
            category,
            display_status,
            last_opened_at,
        });
    }

    // Keep cache consistent with the current attention set so cached startup doesn't show stale/irrelevant PRs.
    delete_prs_not_in(conn, &keep_keys)?;

    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.pr.updated_at_unix.cmp(&a.pr.updated_at_unix))
    });

    Ok(out)
}



