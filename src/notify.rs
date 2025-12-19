//! Cross-platform OS notifications for important PR events.
//!
//! On macOS: Uses terminal-notifier if available (supports click-to-open),
//! otherwise falls back to notify-rust.
//! On other platforms: Uses notify-rust (D-Bus on Linux, Toast on Windows).

use notify_rust::Notification;
use std::sync::OnceLock;

/// Check if terminal-notifier is available (cached).
#[cfg(target_os = "macos")]
fn has_terminal_notifier() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("which")
            .arg("terminal-notifier")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Send a notification with URL using terminal-notifier (macOS).
/// Clicking the notification opens the URL in the default browser.
#[cfg(target_os = "macos")]
fn send_with_terminal_notifier(title: &str, subtitle: &str, body: &str, url: &str) {
    let _ = std::process::Command::new("terminal-notifier")
        .arg("-title")
        .arg(title)
        .arg("-subtitle")
        .arg(subtitle)
        .arg("-message")
        .arg(body)
        .arg("-open")
        .arg(url)
        .arg("-ignoreDnD")
        .spawn();
}

/// Send a simple notification using terminal-notifier (macOS).
#[cfg(target_os = "macos")]
fn send_simple_terminal_notifier(title: &str, body: &str) {
    let _ = std::process::Command::new("terminal-notifier")
        .arg("-title")
        .arg(title)
        .arg("-message")
        .arg(body)
        .arg("-ignoreDnD")
        .spawn();
}

/// Send a notification using notify-rust (fallback).
fn send_with_notify_rust(title: &str, subtitle: &str, body: &str, url: &str) {
    let _ = Notification::new()
        .summary(title)
        .body(&format!("{}\n{}\n{}", subtitle, body, url))
        .timeout(5000)
        .show();
}

/// Send a simple notification using notify-rust (fallback).
fn send_simple_notify_rust(title: &str, body: &str) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .timeout(5000)
        .show();
}

/// Send a notification with URL in the body.
/// On macOS with terminal-notifier: click to open URL.
/// Otherwise: URL shown in body text.
#[cfg(target_os = "macos")]
fn send_with_url(title: &str, subtitle: &str, body: &str, url: &str) {
    if has_terminal_notifier() {
        send_with_terminal_notifier(title, subtitle, body, url);
    } else {
        send_with_notify_rust(title, subtitle, body, url);
    }
}

#[cfg(not(target_os = "macos"))]
fn send_with_url(title: &str, subtitle: &str, body: &str, url: &str) {
    send_with_notify_rust(title, subtitle, body, url);
}

/// Send a simple notification.
#[cfg(target_os = "macos")]
fn send_simple(title: &str, body: &str) {
    if has_terminal_notifier() {
        send_simple_terminal_notifier(title, body);
    } else {
        send_simple_notify_rust(title, body);
    }
}

#[cfg(not(target_os = "macos"))]
fn send_simple(title: &str, body: &str) {
    send_simple_notify_rust(title, body);
}

/// Send a notification for a new CI failure.
pub fn notify_ci_failure(pr_title: &str, repo: &str, url: &str) {
    send_with_url(
        "❌ CI Failed",
        repo,
        &truncate(pr_title, 50),
        url,
    );
}

/// Send a notification for a new review request.
pub fn notify_review_requested(pr_title: &str, repo: &str, url: &str) {
    send_with_url(
        "👀 Review Requested",
        repo,
        &truncate(pr_title, 50),
        url,
    );
}

/// Send a notification when a new repo appears in the PR list.
pub fn notify_new_repo(repo_name: &str) {
    send_simple(
        "📁 New Repository",
        &format!("PRs from {} now visible", repo_name),
    );
}

/// Send a summary notification when PRs enter the "Needs You" category.
pub fn notify_needs_you(count: usize) {
    let body = if count == 1 {
        "1 PR needs your attention".to_string()
    } else {
        format!("{} PRs need your attention", count)
    };
    send_simple("⚠️ Needle: Action Required", &body);
}

/// Send a notification when a PR becomes ready to merge.
pub fn notify_ready_to_merge(pr_title: &str, repo: &str, url: &str) {
    send_with_url(
        "✅ Ready to Merge",
        repo,
        &truncate(pr_title, 50),
        url,
    );
}

/// Send a notification when a new draft PR appears.
pub fn notify_new_draft(pr_title: &str, repo: &str, url: &str) {
    send_with_url(
        "📝 New Draft PR",
        repo,
        &truncate(pr_title, 50),
        url,
    );
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
    let pr_num = 100 + (counter % 50);
    let url = format!("https://github.com/{}/pull/{}", repo, pr_num);
    let notification_type = counter % 6;

    match notification_type {
        0 => notify_ci_failure(title, repo, &url),
        1 => notify_review_requested(title, repo, &url),
        2 => notify_new_repo(repo),
        3 => notify_ready_to_merge(title, repo, &url),
        4 => notify_new_draft(title, repo, &url),
        _ => notify_needs_you((counter % 3) + 1),
    }
}

