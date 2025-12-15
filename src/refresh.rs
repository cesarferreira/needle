use crate::db::{DbPrRow, delete_prs_not_in, load_all_prs, now_unix, upsert_pr};
use crate::demo::{generate_demo_prs, next_demo_tick, seeded_last_opened_at};
use crate::github::fetch_attention_prs;
use crate::model::{CiCheck, CiState, Pr, ReviewState};
use octocrab::Octocrab;
use rusqlite::Connection;
use std::collections::HashMap;

// Scoring constants (single source of truth, also used by TUI help).
pub const SCORE_REVIEW_REQUESTED: i32 = 50;
pub const SCORE_CI_FAILED_NEW: i32 = 40;
pub const SCORE_CI_RUNNING_LONG: i32 = 20;
pub const SCORE_APPROVED_UNMERGED_OLD: i32 = 15;
pub const SCORE_WAITING_ON_OTHERS_GREEN: i32 = -20;
pub const SCORE_CI_FAILED_UNCHANGED: i32 = -30;

pub const CATEGORY_NEEDS_YOU_MIN: i32 = 40;
pub const CATEGORY_NO_ACTION_MIN: i32 = 0;

pub const CI_RUNNING_LONG_SECS: i64 = 10 * 60;
pub const APPROVED_UNMERGED_OLD_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, Default)]
pub struct ScopeFilters {
    pub orgs: Vec<String>,
    pub include_repos: Vec<String>, // owner/repo
    pub exclude_repos: Vec<String>, // owner/repo
}

impl ScopeFilters {
    fn matches(&self, pr: &Pr) -> bool {
        if !self.orgs.is_empty() && !self.orgs.iter().any(|o| o == &pr.owner) {
            return false;
        }
        let full = format!("{}/{}", pr.owner, pr.repo);
        if !self.include_repos.is_empty() && !self.include_repos.iter().any(|r| r == &full) {
            return false;
        }
        if self.exclude_repos.iter().any(|r| r == &full) {
            return false;
        }
        true
    }
}

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
    pub is_new_review_request: bool,
    pub is_new_ci_failure: bool,
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

fn parse_ci_checks_json(s: Option<&str>) -> Vec<CiCheck> {
    let Some(s) = s else { return Vec::new() };
    serde_json::from_str::<Vec<CiCheck>>(s).unwrap_or_else(|_| Vec::new())
}

/// Load cached PRs from SQLite for a fast startup render (no network).
pub fn load_cached(conn: &Connection, cutoff_days: i64, scope: &ScopeFilters) -> Result<Vec<UiPr>, String> {
    let existing: HashMap<String, DbPrRow> = load_all_prs(conn)?;
    let now = now_unix();
    let cutoff_ts = now.saturating_sub(cutoff_days.saturating_mul(86_400));

    let mut out: Vec<UiPr> = Vec::new();
    for (_k, row) in existing {
        let updated_at_unix = row
            .updated_at_unix
            .or(row.last_seen_at)
            .unwrap_or(now);
        if updated_at_unix < cutoff_ts {
            continue;
        }
        let pr = Pr {
            pr_key: row.pr_key.clone(),
            owner: row.owner.clone(),
            repo: row.repo.clone(),
            number: row.number,
            author: row.author.clone().unwrap_or_else(|| "unknown".to_string()),
            title: row.title.clone(),
            url: row.url.clone(),
            updated_at_unix,
            last_commit_sha: row.last_commit_sha.clone(),
            ci_state: parse_ci_state(row.last_ci_state.as_deref()),
            ci_checks: parse_ci_checks_json(row.ci_checks_json.as_deref()),
            review_state: parse_review_state(row.last_review_state.as_deref()),
            is_draft: row.is_draft.unwrap_or(0) != 0,
            mergeable: row.mergeable.clone(),
            merge_state_status: row.merge_state_status.clone(),
        };
        if !scope.matches(&pr) {
            continue;
        }

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
            is_new_review_request: is_new_review,
            is_new_ci_failure,
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

fn draft_to_db(v: bool) -> i64 {
    if v { 1 } else { 0 }
}

fn ci_checks_to_db_json(checks: &[CiCheck]) -> Option<String> {
    if checks.is_empty() {
        return None;
    }
    serde_json::to_string(checks).ok()
}

fn score_pr(pr: &Pr, old: Option<&DbPrRow>, now: i64, is_new_ci_failure: bool) -> i32 {
    let mut score = 0;

    // +50  review requested from user
    if matches!(pr.review_state, ReviewState::Requested) {
        score += SCORE_REVIEW_REQUESTED;
    }

    // CI failure scoring
    if matches!(pr.ci_state, CiState::Failure) {
        if is_new_ci_failure {
            // +40  CI failed AND state changed since last_seen (or commit changed)
            score += SCORE_CI_FAILED_NEW;
        } else {
            // -30  CI failed but unchanged since last_seen
            score += SCORE_CI_FAILED_UNCHANGED;
        }
    }

    // +20  CI running longer than 10 minutes (using updatedAt proxy)
    if matches!(pr.ci_state, CiState::Running) {
        if running_for_secs(pr, now) > CI_RUNNING_LONG_SECS {
            score += SCORE_CI_RUNNING_LONG;
        }
    }

    // +15 approved but unmerged for >24h
    if matches!(pr.review_state, ReviewState::Approved) {
        if now.saturating_sub(pr.updated_at_unix) > APPROVED_UNMERGED_OLD_SECS {
            score += SCORE_APPROVED_UNMERGED_OLD;
        }
    }

    // -20 waiting on others (no review requested, CI green)
    // Note: don't penalize "approved" PRs; those are often actionable (merge/queue) even though no review is requested.
    if matches!(pr.review_state, ReviewState::None) && matches!(pr.ci_state, CiState::Success) {
        score += SCORE_WAITING_ON_OTHERS_GREEN;
    }

    // Note: `old` currently unused beyond is_new_ci_failure; keep signature stable for V1.
    let _ = old;
    score
}

fn category_for(score: i32) -> Category {
    if score >= CATEGORY_NEEDS_YOU_MIN {
        Category::NeedsYou
    } else if score >= CATEGORY_NO_ACTION_MIN {
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

fn running_for_secs(pr: &Pr, now: i64) -> i64 {
    // Prefer the oldest running check's start time (more accurate than PR updatedAt).
    let oldest_start = pr
        .ci_checks
        .iter()
        .filter(|c| matches!(c.state, crate::model::CiCheckState::Running))
        .filter_map(|c| c.started_at_unix)
        .min();
    if let Some(start) = oldest_start {
        return now.saturating_sub(start);
    }
    now.saturating_sub(pr.updated_at_unix)
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
            let mins = running_for_secs(pr, now) / 60;
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

pub async fn refresh(conn: &Connection, octo: &Octocrab, cutoff_days: i64, scope: &ScopeFilters, include_team_requests: bool) -> Result<Vec<UiPr>, String> {
    let existing: HashMap<String, DbPrRow> = load_all_prs(conn)?;
    let now = now_unix();

    let mut out: Vec<UiPr> = Vec::new();

    let cutoff_ts = now.saturating_sub(cutoff_days.saturating_mul(86_400));
    let prs = fetch_attention_prs(octo, cutoff_ts, include_team_requests).await?;

    let prs: Vec<Pr> = prs
        .into_iter()
        .filter(|p| p.updated_at_unix >= cutoff_ts)
        .filter(|p| scope.matches(p))
        .collect();
    let keep_keys: Vec<String> = prs.iter().map(|p| p.pr_key.clone()).collect();

    for pr in prs {
        let old = existing.get(&pr.pr_key);
        let new_review = is_new_review_request(&pr, old);
        let new_ci_failure = is_new_ci_failure(&pr, old);

        let last_opened_at = old
            .and_then(|r| r.last_opened_at)
            .or_else(|| seeded_last_opened_at(&pr.pr_key, now));

        let db_row = DbPrRow {
            pr_key: pr.pr_key.clone(),
            owner: pr.owner.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            author: Some(pr.author.clone()),
            updated_at_unix: Some(pr.updated_at_unix),
            last_commit_sha: pr.last_commit_sha.clone(),
            last_ci_state: Some(ci_to_db(&pr.ci_state).to_string()),
            last_review_state: Some(review_to_db(&pr.review_state).to_string()),
            ci_checks_json: ci_checks_to_db_json(&pr.ci_checks),
            is_draft: Some(draft_to_db(pr.is_draft)),
            mergeable: pr.mergeable.clone(),
            merge_state_status: pr.merge_state_status.clone(),
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
            is_new_review_request: new_review,
            is_new_ci_failure: new_ci_failure,
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

pub fn refresh_demo(conn: &Connection, cutoff_days: i64, scope: &ScopeFilters) -> Result<Vec<UiPr>, String> {
    let existing: HashMap<String, DbPrRow> = load_all_prs(conn)?;
    let now = now_unix();
    let cutoff_ts = now.saturating_sub(cutoff_days.saturating_mul(86_400));

    let tick = next_demo_tick();
    let prs = generate_demo_prs(now, tick);
    let prs: Vec<Pr> = prs
        .into_iter()
        .filter(|p| p.updated_at_unix >= cutoff_ts)
        .filter(|p| scope.matches(p))
        .collect();
    let keep_keys: Vec<String> = prs.iter().map(|p| p.pr_key.clone()).collect();

    let mut out: Vec<UiPr> = Vec::new();

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
            author: Some(pr.author.clone()),
            updated_at_unix: Some(pr.updated_at_unix),
            last_commit_sha: pr.last_commit_sha.clone(),
            last_ci_state: Some(ci_to_db(&pr.ci_state).to_string()),
            last_review_state: Some(review_to_db(&pr.review_state).to_string()),
            ci_checks_json: ci_checks_to_db_json(&pr.ci_checks),
            is_draft: Some(draft_to_db(pr.is_draft)),
            mergeable: pr.mergeable.clone(),
            merge_state_status: pr.merge_state_status.clone(),
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
            is_new_review_request: new_review,
            is_new_ci_failure: new_ci_failure,
        });
    }

    delete_prs_not_in(conn, &keep_keys)?;

    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.pr.updated_at_unix.cmp(&a.pr.updated_at_unix))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CiCheckState, ReviewState};

    fn mk_pr(now: i64, ci_state: CiState, review_state: ReviewState, updated_age_secs: i64, checks: Vec<CiCheck>) -> Pr {
        Pr {
            pr_key: "acme/repo#1".to_string(),
            owner: "acme".to_string(),
            repo: "repo".to_string(),
            number: 1,
            author: "alice".to_string(),
            title: "Test".to_string(),
            url: "https://example.com".to_string(),
            updated_at_unix: now.saturating_sub(updated_age_secs),
            last_commit_sha: Some("deadbeef".to_string()),
            ci_state,
            ci_checks: checks,
            review_state,
            is_draft: false,
            mergeable: None,
            merge_state_status: None,
        }
    }

    #[test]
    fn scoring_review_requested_is_high() {
        let now = 1_700_000_000i64;
        let pr = mk_pr(now, CiState::None, ReviewState::Requested, 60, Vec::new());
        let score = score_pr(&pr, None, now, false);
        assert!(score >= 50);
    }

    #[test]
    fn scoring_ci_failure_new_vs_unchanged() {
        let now = 1_700_000_000i64;
        let pr = mk_pr(now, CiState::Failure, ReviewState::None, 60, Vec::new());
        let s_new = score_pr(&pr, None, now, true);
        let s_old = score_pr(&pr, None, now, false);
        assert_eq!(s_new, 40);
        assert_eq!(s_old, -30);
    }

    #[test]
    fn scoring_running_duration_uses_check_started_at() {
        let now = 1_700_000_000i64;
        let checks = vec![CiCheck {
            name: "integration".to_string(),
            state: CiCheckState::Running,
            url: None,
            started_at_unix: Some(now - 11 * 60),
        }];
        // updated_at_unix is recent, but startedAt is old enough to count as long-running.
        let pr = mk_pr(now, CiState::Running, ReviewState::None, 60, checks);
        let score = score_pr(&pr, None, now, false);
        assert_eq!(score, 20);
    }

    #[test]
    fn scoring_approved_but_unmerged_after_24h() {
        let now = 1_700_000_000i64;
        let pr = mk_pr(now, CiState::Success, ReviewState::Approved, 25 * 3600, Vec::new());
        let score = score_pr(&pr, None, now, false);
        assert!(score >= 15);
    }

    #[test]
    fn scoring_waiting_on_others_is_negative() {
        let now = 1_700_000_000i64;
        let pr = mk_pr(now, CiState::Success, ReviewState::None, 300, Vec::new());
        let score = score_pr(&pr, None, now, false);
        assert_eq!(score, -20);
    }

    #[test]
    fn new_ci_failure_when_commit_sha_changes() {
        let now = 1_700_000_000i64;
        let mut pr = mk_pr(now, CiState::Failure, ReviewState::None, 60, Vec::new());
        pr.last_commit_sha = Some("bbbbbbb".to_string());

        let old = DbPrRow {
            pr_key: pr.pr_key.clone(),
            owner: pr.owner.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            author: None,
            updated_at_unix: None,
            last_commit_sha: Some("aaaaaaa".to_string()),
            last_ci_state: Some("failure".to_string()),
            last_review_state: Some("none".to_string()),
            ci_checks_json: None,
            is_draft: None,
            mergeable: None,
            merge_state_status: None,
            last_seen_at: Some(now - 10),
            last_opened_at: None,
        };

        assert!(is_new_ci_failure(&pr, Some(&old)));
    }
}

