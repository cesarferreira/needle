//! Cross-platform OS notifications for important PR events.
//!
//! Uses notify-rust which supports:
//! - macOS: Native Notification Center
//! - Linux: D-Bus (freedesktop.org standard)
//! - Windows: Toast Notifications

use notify_rust::Notification;

/// Send a notification for a new CI failure.
pub fn notify_ci_failure(pr_title: &str, repo: &str) {
    let _ = Notification::new()
        .summary("CI Failed")
        .body(&format!("{}\n{}", repo, truncate(pr_title, 50)))
        .icon("dialog-error")
        .timeout(5000)
        .show();
}

/// Send a notification for a new review request.
pub fn notify_review_requested(pr_title: &str, repo: &str) {
    let _ = Notification::new()
        .summary("Review Requested")
        .body(&format!("{}\n{}", repo, truncate(pr_title, 50)))
        .icon("dialog-information")
        .timeout(5000)
        .show();
}

/// Send a notification when a new repo appears in the PR list.
pub fn notify_new_repo(repo_name: &str) {
    let _ = Notification::new()
        .summary("New Repository")
        .body(&format!("PRs from {} now visible", repo_name))
        .icon("folder-new")
        .timeout(5000)
        .show();
}

/// Send a summary notification when PRs enter the "Needs You" category.
pub fn notify_needs_you(count: usize) {
    let body = if count == 1 {
        "1 PR needs your attention".to_string()
    } else {
        format!("{} PRs need your attention", count)
    };
    let _ = Notification::new()
        .summary("Needle: Action Required")
        .body(&body)
        .icon("dialog-warning")
        .timeout(5000)
        .show();
}

/// Send a notification when a PR becomes ready to merge.
pub fn notify_ready_to_merge(pr_title: &str, repo: &str) {
    let _ = Notification::new()
        .summary("Ready to Merge")
        .body(&format!("{}\n{}", repo, truncate(pr_title, 50)))
        .icon("emblem-default")
        .timeout(5000)
        .show();
}

/// Truncate a string to a maximum length, adding ellipsis if needed.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}

/// Counter for cycling through demo notification types.
static DEMO_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Send a random demo notification (for --demo mode).
pub fn notify_random_demo() {
    use std::sync::atomic::Ordering;

    // Increment counter to cycle through notification types.
    let counter = DEMO_COUNTER.fetch_add(1, Ordering::Relaxed);

    let demo_repos = [
        "acme/backend",
        "acme/frontend",
        "acme/mobile-app",
        "acme/infrastructure",
        "acme/docs",
    ];
    let demo_titles = [
        "Fix authentication bug in login flow",
        "Add dark mode support",
        "Refactor database queries for performance",
        "Update dependencies to latest versions",
        "Implement new feature flag system",
        "Fix memory leak in worker process",
        "Add unit tests for payment module",
        "Migrate to new API version",
    ];

    let repo = demo_repos[counter % demo_repos.len()];
    let title = demo_titles[counter % demo_titles.len()];
    let notification_type = counter % 5;

    match notification_type {
        0 => notify_ci_failure(title, repo),
        1 => notify_review_requested(title, repo),
        2 => notify_new_repo(repo),
        3 => notify_ready_to_merge(title, repo),
        _ => notify_needs_you((counter % 3) + 1),
    }
}

