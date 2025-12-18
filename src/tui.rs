use crate::db::now_unix;
use crate::refresh::{
    APPROVED_UNMERGED_OLD_SECS, CATEGORY_NEEDS_YOU_MIN, CATEGORY_NO_ACTION_MIN,
    CI_RUNNING_LONG_SECS, Category, SCORE_APPROVED_UNMERGED_OLD, SCORE_CI_FAILED_NEW,
    SCORE_CI_FAILED_UNCHANGED, SCORE_CI_RUNNING_LONG, SCORE_REVIEW_REQUESTED,
    SCORE_WAITING_ON_OTHERS_GREEN, UiPr,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::tty::IsTty;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use rusqlite::Connection;
use std::collections::HashSet;
use std::io::{self, Stdout};
use std::process::Command;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::time::Instant;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use update_informer::{Check, registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Details,
}

#[derive(Debug, Clone, Copy)]
pub struct UiPrefs {
    pub hide_pr_numbers: bool,
    pub hide_repo: bool,
    pub hide_author: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            hide_pr_numbers: false,
            hide_repo: false,
            hide_author: false,
        }
    }
}

pub struct AppState {
    pub prs: Vec<UiPr>,
    pub selected_idx: usize, // index into visible_pr_indices
    pub(crate) mode: ViewMode,
    pub(crate) details_pr_key: Option<String>,
    pub(crate) refreshing: bool,
    pub(crate) shimmer_phase: u8,
    pub(crate) details_ci_selected: usize,
    pub(crate) details_last_auto_refresh: Option<Instant>,
    pub(crate) last_refresh_started: Option<Instant>,
    pub(crate) ui: UiPrefs,
    pub(crate) help_open: bool,

    // List filters/search.
    pub(crate) filter_query: String,
    pub(crate) filter_editing: bool,
    pub(crate) filter_edit: String,
    pub(crate) filter_prev_query: String,
    pub(crate) only_needs_you: bool,
    pub(crate) only_failing_ci: bool,
    pub(crate) only_review_requested: bool,
    pub(crate) update_notice: Option<String>,
}

impl AppState {
    pub fn new(prs: Vec<UiPr>, ui: UiPrefs) -> Self {
        Self {
            prs,
            selected_idx: 0,
            mode: ViewMode::List,
            details_pr_key: None,
            refreshing: false,
            shimmer_phase: 0,
            details_ci_selected: 0,
            details_last_auto_refresh: None,
            last_refresh_started: Some(Instant::now()),
            ui,
            help_open: false,
            filter_query: String::new(),
            filter_editing: false,
            filter_edit: String::new(),
            filter_prev_query: String::new(),
            only_needs_you: false,
            only_failing_ci: false,
            only_review_requested: false,
            update_notice: None,
        }
    }
}

fn spawn_update_check() -> Option<mpsc::Receiver<String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let informer = update_informer::new(
            registry::Crates,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        );
        if let Ok(Some(version)) = informer.check_version() {
            let msg = format!(
                "Update available: v{version} (current v{}) — install with `cargo install {} --locked --force`",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_NAME"),
            );
            let _ = tx.send(msg);
        }
    });
    Some(rx)
}

fn category_title(cat: Category) -> &'static str {
    match cat {
        Category::NeedsYou => "🔥 NEEDS YOU",
        Category::ReadyToMerge => "🚢 READY TO MERGE",
        Category::Waiting => "✅ NO ACTION NEEDED",
        Category::Stale => "⏳ WAITING ON OTHERS",
    }
}

fn category_style(cat: Category) -> Style {
    match cat {
        Category::NeedsYou => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        Category::ReadyToMerge => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        Category::Waiting => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        Category::Stale => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    }
}

fn truncate_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }

    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max_width {
            break;
        }
        out.push(ch);
        w += cw;
    }

    // Ensure we end with an ellipsis if we actually truncated.
    if !out.is_empty() {
        // Make room for ellipsis (width 1) by removing chars until it fits.
        while UnicodeWidthStr::width(out.as_str()) + 1 > max_width {
            out.pop();
        }
        if !out.is_empty() {
            out.push('…');
        }
    } else {
        // No room even for content; show ellipsis if it fits.
        if max_width >= 1 {
            out.push('…');
        }
    }
    out
}

fn pad_right(s: &str, width: usize) -> String {
    let len = UnicodeWidthStr::width(s);
    if len >= width {
        s.to_string()
    } else {
        let mut out = String::with_capacity(width);
        out.push_str(s);
        out.extend(std::iter::repeat(' ').take(width - len));
        out
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

fn human_duration(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    }
}

fn open_in_browser(url: &str) {
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        return;
    }

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };

    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };

    let _ = cmd.spawn();
}

fn matches_filter(pr: &UiPr, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let q = query.trim().to_lowercase();
    let repo = format!("{}/{}", pr.pr.owner, pr.pr.repo).to_lowercase();
    let author = pr.pr.author.to_lowercase();
    let title = pr.pr.title.to_lowercase();
    let num = format!("#{}", pr.pr.number);
    repo.contains(&q) || author.contains(&q) || title.contains(&q) || num.contains(query.trim())
}

fn filtered_indices(
    state_prs: &[UiPr],
    query: &str,
    only_needs_you: bool,
    only_failing_ci: bool,
    only_review_requested: bool,
) -> Vec<usize> {
    let mut out = Vec::new();
    for (idx, pr) in state_prs.iter().enumerate() {
        if only_needs_you && pr.category != Category::NeedsYou {
            continue;
        }
        if only_failing_ci && !matches!(pr.pr.ci_state, crate::model::CiState::Failure) {
            continue;
        }
        if only_review_requested
            && !matches!(pr.pr.review_state, crate::model::ReviewState::Requested)
        {
            continue;
        }
        if !matches_filter(pr, query) {
            continue;
        }
        out.push(idx);
    }
    out
}

fn build_list_lines(
    prs: &[UiPr],
    inner_width: u16,
    inner_height: u16,
    selected_visible_idx: usize,
    filtered: &[usize],
    filter_banner: Option<&str>,
    ui: UiPrefs,
) -> (Vec<Line<'static>>, Vec<usize>) {
    // We build rendered lines (headers/dividers/rows/blanks) up to inner_height.
    // Also track which `prs` indices are visible, in order, so selection works.
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut visible_pr_indices: Vec<usize> = Vec::new();

    fn push_line(lines: &mut Vec<Line<'static>>, inner_height: u16, line: Line<'static>) {
        if (lines.len() as u16) < inner_height {
            lines.push(line);
        }
    }

    // Optional filter banner at the top.
    if let Some(banner) = filter_banner {
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::styled(
                truncate_ellipsis(banner, inner_width as usize),
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            )),
        );
        push_line(&mut lines, inner_height, Line::from(Span::raw("")));
    }

    // Table-ish column sizing (dynamic; only truncates when the terminal width forces it).
    // Columns: prefix(2) [repo] [author] [#num] title status
    let iw = inner_width as usize;
    let prefix_w = 2usize;
    let sep_w = 2usize; // two spaces between columns

    let max_repo_len = if ui.hide_repo {
        0
    } else {
        filtered
            .iter()
            .filter_map(|&i| prs.get(i))
            .map(|p| {
                let s = format!("{}/{}", p.pr.owner, p.pr.repo);
                UnicodeWidthStr::width(s.as_str())
            })
            .max()
            .unwrap_or(10)
    };
    let max_author_len = if ui.hide_author {
        0
    } else {
        filtered
            .iter()
            .filter_map(|&i| prs.get(i))
            .map(|p| UnicodeWidthStr::width(p.pr.author.as_str()))
            .max()
            .unwrap_or(6)
    };
    let max_num_len = if ui.hide_pr_numbers {
        0
    } else {
        filtered
            .iter()
            .filter_map(|&i| prs.get(i))
            .map(|p| {
                let s = format!("#{}", p.pr.number);
                UnicodeWidthStr::width(s.as_str())
            })
            .max()
            .unwrap_or(4)
    };
    let max_status_len = filtered
        .iter()
        .filter_map(|&i| prs.get(i))
        .map(|p| UnicodeWidthStr::width(p.display_status.as_str()))
        .max()
        .unwrap_or(10);

    // Reasonable upper bounds so title keeps most of the width,
    // but allow longer statuses like "CI running (123m)" without truncation.
    let status_w = max_status_len.clamp(12, 34);
    let num_w = if ui.hide_pr_numbers {
        0
    } else {
        max_num_len.clamp(4, 8)
    };
    let author_w = if ui.hide_author {
        0
    } else {
        max_author_len.clamp(6, 16)
    };

    // Ensure title gets at least 16 chars; repo uses remaining but capped.
    let min_title_w = 16usize;
    let max_repo_w = 35usize;
    let mut repo_w = if ui.hide_repo {
        0
    } else {
        max_repo_len.min(max_repo_w)
    };

    let repo_block = if ui.hide_repo { 0 } else { repo_w + sep_w };
    let author_block = if ui.hide_author { 0 } else { author_w + sep_w };
    let num_block = if ui.hide_pr_numbers { 0 } else { num_w + sep_w };

    // Compute remaining for title and shrink repo if needed.
    let fixed = prefix_w + repo_block + author_block + num_block + status_w + sep_w;
    let mut title_w = iw.saturating_sub(fixed);
    if title_w < min_title_w {
        let missing = min_title_w - title_w;
        repo_w = repo_w.saturating_sub(missing);
        let repo_block2 = if ui.hide_repo { 0 } else { repo_w + sep_w };
        let fixed2 = prefix_w + repo_block2 + author_block + num_block + status_w + sep_w;
        title_w = iw.saturating_sub(fixed2);
    }
    if title_w < 8 {
        // Extremely narrow terminal; keep things from going negative.
        title_w = 8;
    }

    fn render_row(
        lines: &mut Vec<Line<'static>>,
        visible_pr_indices: &mut Vec<usize>,
        inner_height: u16,
        selected_visible_idx: usize,
        ui: UiPrefs,
        repo_w: usize,
        author_w: usize,
        num_w: usize,
        title_w: usize,
        status_w: usize,
        idx: usize,
        pr: &UiPr,
    ) {
        if (lines.len() as u16) >= inner_height {
            return;
        }
        let visible_idx = visible_pr_indices.len();
        visible_pr_indices.push(idx);

        let is_selected = visible_idx == selected_visible_idx;
        let prefix = if is_selected { "> " } else { "  " };

        let repo = if ui.hide_repo {
            String::new()
        } else {
            pad_right(
                &truncate_ellipsis(&format!("{}/{}", pr.pr.owner, pr.pr.repo), repo_w),
                repo_w,
            )
        };

        let author = if ui.hide_author {
            String::new()
        } else {
            pad_right(&truncate_ellipsis(&pr.pr.author, author_w), author_w)
        };

        let num = if ui.hide_pr_numbers {
            String::new()
        } else {
            pad_right(
                &truncate_ellipsis(&format!("#{}", pr.pr.number), num_w),
                num_w,
            )
        };

        let title = pad_right(&truncate_ellipsis(&pr.pr.title, title_w), title_w);
        let status = pad_right(&truncate_ellipsis(&pr.display_status, status_w), status_w);

        let base = if is_selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else if pr.pr.is_draft {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };

        let status_color = match pr.pr.ci_state {
            crate::model::CiState::Success => Color::Green,
            crate::model::CiState::Failure => Color::Red,
            crate::model::CiState::Running => Color::Yellow,
            crate::model::CiState::None => Color::Gray,
        };

        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(prefix.to_string(), base.fg(Color::White)));
        if !ui.hide_repo {
            spans.push(Span::styled(repo, base.fg(Color::Cyan)));
            spans.push(Span::raw("  "));
        }
        if !ui.hide_author {
            spans.push(Span::styled(author, base.fg(Color::Magenta)));
            spans.push(Span::raw("  "));
        }
        if !ui.hide_pr_numbers {
            spans.push(Span::styled(
                num,
                base.fg(Color::Blue).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(title, base.fg(Color::White)));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            status,
            base.fg(status_color).add_modifier(Modifier::BOLD),
        ));
        push_line(lines, inner_height, Line::from(spans));
    }

    // DRAFT section is rendered at the bottom (but drafts are excluded from other sections).
    let has_drafts = filtered
        .iter()
        .filter_map(|&i| prs.get(i))
        .any(|p| p.pr.is_draft);

    let cats = [
        Category::ReadyToMerge,
        Category::NeedsYou,
        Category::Waiting,
        Category::Stale,
    ];
    for cat in cats {
        // Skip empty sections entirely.
        if !filtered
            .iter()
            .filter_map(|&i| prs.get(i))
            .any(|p| !p.pr.is_draft && p.category == cat)
        {
            continue;
        }

        let start_len = lines.len();

        // Header + divider
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::styled(
                category_title(cat).to_string(),
                category_style(cat),
            )),
        );
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::raw(
                std::iter::repeat('─').take(iw).collect::<String>(),
            )),
        );
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::styled(
                {
                    let mut s = String::new();
                    s.push_str("  ");
                    if !ui.hide_repo {
                        s.push_str(&pad_right("REPO", repo_w));
                        s.push_str("  ");
                    }
                    if !ui.hide_author {
                        s.push_str(&pad_right("AUTHOR", author_w));
                        s.push_str("  ");
                    }
                    if !ui.hide_pr_numbers {
                        s.push_str(&pad_right("PR", num_w));
                        s.push_str("  ");
                    }
                    s.push_str(&pad_right("TITLE", title_w));
                    s.push_str("  ");
                    s.push_str(&pad_right("STATUS", status_w));
                    s
                },
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            )),
        );

        // Rows in this category
        for &idx in filtered {
            let Some(pr) = prs.get(idx) else { continue };
            if pr.pr.is_draft {
                continue;
            }
            if pr.category != cat {
                continue;
            }
            if (lines.len() as u16) >= inner_height {
                break;
            }
            render_row(
                &mut lines,
                &mut visible_pr_indices,
                inner_height,
                selected_visible_idx,
                ui,
                repo_w,
                author_w,
                num_w,
                title_w,
                status_w,
                idx,
                pr,
            );
        }

        // Blank line after section, but only if we actually rendered something in the section
        // and we still have space.
        if lines.len() != start_len && (lines.len() as u16) < inner_height {
            push_line(&mut lines, inner_height, Line::from(Span::raw("")));
        }

        if (lines.len() as u16) >= inner_height {
            break;
        }
    }

    // DRAFT section (shown separately so drafts don't appear as "waiting on others").
    if has_drafts && (lines.len() as u16) < inner_height {
        let start_len = lines.len();

        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::styled(
                "📝 DRAFT".to_string(),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            )),
        );
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::raw(
                std::iter::repeat('─').take(iw).collect::<String>(),
            )),
        );
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::styled(
                {
                    let mut s = String::new();
                    s.push_str("  ");
                    if !ui.hide_repo {
                        s.push_str(&pad_right("REPO", repo_w));
                        s.push_str("  ");
                    }
                    if !ui.hide_author {
                        s.push_str(&pad_right("AUTHOR", author_w));
                        s.push_str("  ");
                    }
                    if !ui.hide_pr_numbers {
                        s.push_str(&pad_right("PR", num_w));
                        s.push_str("  ");
                    }
                    s.push_str(&pad_right("TITLE", title_w));
                    s.push_str("  ");
                    s.push_str(&pad_right("STATUS", status_w));
                    s
                },
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            )),
        );

        for &idx in filtered {
            let Some(pr) = prs.get(idx) else { continue };
            if !pr.pr.is_draft {
                continue;
            }
            render_row(
                &mut lines,
                &mut visible_pr_indices,
                inner_height,
                selected_visible_idx,
                ui,
                repo_w,
                author_w,
                num_w,
                title_w,
                status_w,
                idx,
                pr,
            );
            if (lines.len() as u16) >= inner_height {
                break;
            }
        }

        if lines.len() != start_len && (lines.len() as u16) < inner_height {
            push_line(&mut lines, inner_height, Line::from(Span::raw("")));
        }
    }

    (lines, visible_pr_indices)
}

fn build_footer(
    inner_width: u16,
    mode: ViewMode,
    refreshing: bool,
    shimmer_phase: u8,
    filter_mode: bool,
) -> Line<'static> {
    #[derive(Clone)]
    struct Seg {
        text: String,
        style: Style,
    }
    fn keycap(k: &str) -> Seg {
        Seg {
            text: format!("[{k}]"),
            style: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        }
    }
    fn label(s: &str) -> Seg {
        Seg {
            text: s.to_string(),
            style: Style::default().fg(Color::Gray),
        }
    }
    fn sep() -> Seg {
        Seg {
            text: "  ".to_string(),
            style: Style::default(),
        }
    }

    fn shimmer(phase: u8) -> String {
        // 10-column shimmer bar with a moving bright block.
        let w = 10usize;
        let pos = (phase as usize) % w;
        let mut s = String::with_capacity(w);
        for i in 0..w {
            s.push(if i == pos { '▓' } else { '░' });
        }
        s
    }

    let mut segs: Vec<Seg> = Vec::new();
    match mode {
        ViewMode::List => {
            if filter_mode {
                segs.extend([
                    keycap("Esc"),
                    label("back"),
                    sep(),
                    keycap("Enter"),
                    label("open"),
                    sep(),
                    keycap("Backspace"),
                    label("delete"),
                    sep(),
                    keycap("Ctrl+n"),
                    label("needs"),
                    sep(),
                    keycap("Ctrl+c"),
                    label("failing"),
                    sep(),
                    keycap("Ctrl+v"),
                    label("review"),
                    sep(),
                    keycap("Ctrl+x"),
                    label("clear"),
                    sep(),
                    keycap("?"),
                    label("help"),
                ]);
            } else {
                segs.extend([
                    keycap("q"),
                    label("quit"),
                    sep(),
                    keycap("r"),
                    if refreshing {
                        Seg {
                            text: format!("refreshing {}", shimmer(shimmer_phase)),
                            style: Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        }
                    } else {
                        label("refresh")
                    },
                    sep(),
                    keycap("/"),
                    label("filter"),
                    sep(),
                    keycap("?"),
                    label("help"),
                    sep(),
                    keycap("Enter"),
                    label("open"),
                    sep(),
                    keycap("Tab"),
                    label("details"),
                    sep(),
                    keycap("↑/↓"),
                    label("move"),
                ]);
            }
        }
        ViewMode::Details => {
            segs.extend([
                keycap("Tab"),
                label("back"),
                sep(),
                keycap("Enter"),
                label("open check"),
                sep(),
                keycap("f"),
                label("open failing"),
                sep(),
                keycap("r"),
                if refreshing {
                    Seg {
                        text: format!("refreshing {}", shimmer(shimmer_phase)),
                        style: Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    }
                } else {
                    label("refresh")
                },
                sep(),
                keycap("?"),
                label("help"),
                sep(),
                keycap("q"),
                label("quit"),
                sep(),
                keycap("↑/↓"),
                label("select"),
            ]);
        }
    }

    let iw = inner_width as usize;

    // Keep keycap colors even in narrow terminals by dropping low-priority segments
    // instead of falling back to a plain hint line.
    let total_w: usize = segs
        .iter()
        .map(|s| UnicodeWidthStr::width(s.text.as_str()))
        .sum();
    if total_w > iw {
        let mut essential: Vec<Seg> = match mode {
            ViewMode::List => vec![
                keycap("q"),
                label("quit"),
                sep(),
                keycap("r"),
                if refreshing {
                    Seg {
                        text: format!("refreshing {}", shimmer(shimmer_phase)),
                        style: Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    }
                } else {
                    label("refresh")
                },
                sep(),
                keycap("/"),
                label("filter"),
                sep(),
                keycap("?"),
                label("help"),
                sep(),
                keycap("Enter"),
                label("open"),
                sep(),
                keycap("Tab"),
                label("details"),
                sep(),
                keycap("↑/↓"),
                label("move"),
            ],
            ViewMode::Details => vec![
                keycap("Tab"),
                label("back"),
                sep(),
                keycap("Enter"),
                label("open"),
                sep(),
                keycap("r"),
                if refreshing {
                    Seg {
                        text: format!("refreshing {}", shimmer(shimmer_phase)),
                        style: Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    }
                } else {
                    label("refresh")
                },
                sep(),
                keycap("?"),
                label("help"),
                sep(),
                keycap("q"),
                label("quit"),
            ],
        };

        // Add optional segments only if they fit.
        let mut optional: Vec<Seg> = match mode {
            ViewMode::List => Vec::new(),
            ViewMode::Details => vec![
                sep(),
                keycap("f"),
                label("failing"),
                sep(),
                keycap("↑/↓"),
                label("select"),
            ],
        };

        let mut cur_w: usize = essential
            .iter()
            .map(|s| UnicodeWidthStr::width(s.text.as_str()))
            .sum();
        while !optional.is_empty() {
            let next = optional.remove(0);
            let next_w = UnicodeWidthStr::width(next.text.as_str());
            if cur_w + next_w > iw {
                break;
            }
            cur_w += next_w;
            essential.push(next);
        }

        segs = essential;
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    for s in segs {
        spans.push(Span::styled(s.text, s.style));
    }
    Line::from(spans)
}

fn build_details_lines(
    pr: &UiPr,
    inner_width: u16,
    inner_height: u16,
    ci_selected: usize,
) -> Vec<Line<'static>> {
    let iw = inner_width as usize;
    let mut out: Vec<Line<'static>> = Vec::new();
    let now = now_unix();

    // Title line
    out.push(Line::from(Span::styled(
        truncate_ellipsis("DETAILS", iw),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    out.push(Line::from(Span::styled(
        std::iter::repeat('─').take(iw).collect::<String>(),
        Style::default().fg(Color::Gray),
    )));

    let rows = [
        ("Repo", format!("{}/{}", pr.pr.owner, pr.pr.repo)),
        ("PR", format!("#{}", pr.pr.number)),
        ("Author", pr.pr.author.clone()),
        ("Title", pr.pr.title.clone()),
        ("Status", pr.display_status.clone()),
        ("Updated", human_age(now, pr.pr.updated_at_unix)),
        ("URL", pr.pr.url.clone()),
        (
            "Commit",
            pr.pr
                .last_commit_sha
                .clone()
                .unwrap_or_else(|| "none".to_string()),
        ),
        (
            "Draft",
            if pr.pr.is_draft {
                "yes".to_string()
            } else {
                "no".to_string()
            },
        ),
        (
            "Mergeable",
            pr.pr
                .mergeable
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        ),
        (
            "MergeState",
            pr.pr
                .merge_state_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        ),
    ];

    for (k, v) in rows {
        if (out.len() as u16) >= inner_height {
            break;
        }
        let key = format!("{k}: ");
        let val = v;
        let key_w = UnicodeWidthStr::width(key.as_str());
        let val_w = iw.saturating_sub(key_w);
        out.push(Line::from(vec![
            Span::styled(
                key,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate_ellipsis(&val, val_w),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    // CI checks list
    if (out.len() as u16) < inner_height {
        out.push(Line::from(Span::raw("")));
    }
    if (out.len() as u16) < inner_height {
        out.push(Line::from(Span::styled(
            "CI CHECKS".to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if (out.len() as u16) < inner_height {
        out.push(Line::from(Span::styled(
            std::iter::repeat('─').take(iw).collect::<String>(),
            Style::default().fg(Color::Gray),
        )));
    }

    if pr.pr.ci_checks.is_empty() {
        if (out.len() as u16) < inner_height {
            out.push(Line::from(Span::styled(
                "No check runs".to_string(),
                Style::default().fg(Color::Gray),
            )));
        }
    } else {
        let mut n_fail = 0usize;
        let mut n_run = 0usize;
        let mut n_ok = 0usize;
        let mut n_other = 0usize;
        for c in &pr.pr.ci_checks {
            match c.state {
                crate::model::CiCheckState::Failure => n_fail += 1,
                crate::model::CiCheckState::Running => n_run += 1,
                crate::model::CiCheckState::Success => n_ok += 1,
                _ => n_other += 1,
            }
        }
        if (out.len() as u16) < inner_height {
            out.push(Line::from(Span::styled(
                format!("Summary: {n_fail} failed, {n_run} running, {n_ok} ok, {n_other} other"),
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            )));
        }

        let mut failed: Vec<String> = pr
            .pr
            .ci_checks
            .iter()
            .filter(|c| c.state.is_failure())
            .map(|c| c.name.clone())
            .collect();
        failed.truncate(3);
        if !failed.is_empty() && (out.len() as u16) < inner_height {
            out.push(Line::from(Span::styled(
                format!("Failed: {}", failed.join(", ")),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }

        for (idx, c) in pr.pr.ci_checks.iter().enumerate() {
            if (out.len() as u16) >= inner_height {
                break;
            }
            let is_sel = idx == ci_selected;
            let prefix = if is_sel { "> " } else { "  " };
            let (icon, col) = match c.state {
                crate::model::CiCheckState::Success => ("✅", Color::Green),
                crate::model::CiCheckState::Failure => ("❌", Color::Red),
                crate::model::CiCheckState::Running => ("🟡", Color::Yellow),
                crate::model::CiCheckState::Neutral => ("➖", Color::Gray),
                crate::model::CiCheckState::None => ("⏺", Color::Gray),
            };
            let mut suffix = String::new();
            if matches!(c.state, crate::model::CiCheckState::Running) {
                if let Some(start) = c.started_at_unix {
                    suffix = format!(" ({})", human_duration(now.saturating_sub(start)));
                }
            }
            let name = truncate_ellipsis(&format!("{}{}", c.name, suffix), iw.saturating_sub(6));
            let base = if is_sel {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            out.push(Line::from(vec![
                Span::styled(prefix.to_string(), base.fg(Color::White)),
                Span::styled(
                    format!("{icon} "),
                    base.fg(col).add_modifier(Modifier::BOLD),
                ),
                Span::styled(name, base.fg(Color::White)),
            ]));
        }
        if (out.len() as u16) < inner_height {
            out.push(Line::from(Span::styled(
                "Enter: open selected check   f: open first failing check".to_string(),
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            )));
        }
    }

    out
}

fn clamp_selection(selected: &mut usize, visible_len: usize) {
    if visible_len == 0 {
        *selected = 0;
    } else if *selected >= visible_len {
        *selected = visible_len - 1;
    }
}

fn help_lines() -> Vec<Line<'static>> {
    let mins = |s: i64| s / 60;
    let hours = |s: i64| s / 3600;
    vec![
        Line::from(vec![
            Span::styled(
                "needle",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  help", Style::default().fg(Color::Gray)),
        ]),
        Line::from(Span::styled(
            "─".repeat(60),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Sections",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("  🔥 NEEDS YOU: score >= {CATEGORY_NEEDS_YOU_MIN}")),
        Line::from("  🚢 READY TO MERGE: your PR, CI green, no blockers"),
        Line::from("  ✅ NO ACTION NEEDED: score 0..39 (not ready-to-merge)"),
        Line::from(format!(
            "  ⏳ WAITING ON OTHERS: score < {CATEGORY_NO_ACTION_MIN} (currently: green + no review)"
        )),
        Line::from(
            "  📝 DRAFT: drafts are dimmed and shown in their own section (regardless of score)",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Scoring",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "  +{SCORE_REVIEW_REQUESTED:<2}  review requested from you"
        )),
        Line::from(format!("  +{SCORE_CI_FAILED_NEW:<2}  CI failed (new)")),
        Line::from("       (state changed since last_seen or new commit)"),
        Line::from(format!(
            "  +{SCORE_CI_RUNNING_LONG:<2}  CI running longer than {}m",
            mins(CI_RUNNING_LONG_SECS)
        )),
        Line::from(format!(
            "  +{SCORE_APPROVED_UNMERGED_OLD:<2}  approved but unmerged for >{}h",
            hours(APPROVED_UNMERGED_OLD_SECS)
        )),
        Line::from(format!(
            "  {SCORE_WAITING_ON_OTHERS_GREEN:<3} waiting on others (green + no review, not approved)"
        )),
        Line::from(format!(
            "  {SCORE_CI_FAILED_UNCHANGED:<3} CI failed (unchanged)"
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Keys",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "  List    : ↑/↓ move  Enter open  Tab details  r refresh  / filter  ? help  q quit",
        ),
        Line::from("            Esc clears active filter (when not typing)"),
        Line::from("  Filter  : type to filter  ↑/↓ move  Enter open  Esc clear+exit"),
        Line::from("            Ctrl+n needs  Ctrl+c failing  Ctrl+v review  Ctrl+x clear"),
        Line::from("  Details : ↑/↓ select  Enter open check  f open failing  Tab back"),
        Line::from(""),
        Line::from(Span::styled(
            "Press ? or Esc to close.",
            Style::default().fg(Color::Gray),
        )),
    ]
}

pub fn run_tui(
    _conn: &Connection,
    mut state: AppState,
    refresh_fn: Arc<dyn Fn() -> Result<Vec<UiPr>, String> + Send + Sync>,
    start_refresh_immediately: bool,
    bell_enabled: bool,
    notify_enabled: bool,
    demo_mode: bool,
) -> Result<(), String> {
    if !io::stdin().is_tty() || !io::stdout().is_tty() {
        return Err("Not a TTY: run `needle` in an interactive terminal.".to_string());
    }
    enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {e}"))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| format!("Failed to enter alt screen: {e}"))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(backend).map_err(|e| format!("Failed to init terminal: {e}"))?;

    let mut refresh_rx: Option<mpsc::Receiver<Result<Vec<UiPr>, String>>> = None;
    let mut update_rx = spawn_update_check();

    // Track seen repos for new-repo notifications.
    let mut seen_repos: HashSet<String> = state
        .prs
        .iter()
        .map(|p| format!("{}/{}", p.pr.owner, p.pr.repo))
        .collect();

    // Timer for demo mode random notifications (every 3 seconds).
    let mut last_demo_notification: Option<Instant> = if demo_mode && notify_enabled {
        Some(Instant::now())
    } else {
        None
    };

    if start_refresh_immediately && !state.refreshing {
        state.refreshing = true;
        state.shimmer_phase = 0;
        state.last_refresh_started = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        refresh_rx = Some(rx);
        let rf = Arc::clone(&refresh_fn);
        std::thread::spawn(move || {
            let res = rf();
            let _ = tx.send(res);
        });
    }

    loop {
        // Non-blocking update check result.
        if let Some(rx) = &update_rx {
            match rx.try_recv() {
                Ok(msg) => {
                    state.update_notice = Some(msg);
                    update_rx = None;
                }
                Err(TryRecvError::Disconnected) => {
                    update_rx = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        // Demo mode: send random notifications every 3 seconds.
        if let Some(last) = last_demo_notification {
            if last.elapsed() >= Duration::from_secs(3) {
                crate::notify::notify_random_demo();
                last_demo_notification = Some(Instant::now());
            }
        }

        // Auto refresh in list view every 3 minutes (non-blocking).
        if state.mode == ViewMode::List && !state.refreshing {
            let should = state
                .last_refresh_started
                .map(|t| t.elapsed() >= Duration::from_secs(60 * 3))
                .unwrap_or(true);
            if should {
                state.last_refresh_started = Some(Instant::now());
                state.refreshing = true;
                state.shimmer_phase = 0;
                let (tx, rx) = mpsc::channel();
                refresh_rx = Some(rx);
                let rf = Arc::clone(&refresh_fn);
                std::thread::spawn(move || {
                    let res = rf();
                    let _ = tx.send(res);
                });
            }
        }

        // Auto refresh in details view every 30s (non-blocking).
        if state.mode == ViewMode::Details && !state.refreshing {
            let should = state
                .details_last_auto_refresh
                .map(|t| t.elapsed() >= Duration::from_secs(30))
                .unwrap_or(true);
            if should {
                state.details_last_auto_refresh = Some(Instant::now());
                state.last_refresh_started = Some(Instant::now());
                state.refreshing = true;
                state.shimmer_phase = 0;
                let (tx, rx) = mpsc::channel();
                refresh_rx = Some(rx);
                let rf = Arc::clone(&refresh_fn);
                std::thread::spawn(move || {
                    let res = rf();
                    let _ = tx.send(res);
                });
            }
        }

        // If a refresh is in-flight, animate shimmer and apply results when ready.
        if state.refreshing {
            state.shimmer_phase = state.shimmer_phase.wrapping_add(1);
            if let Some(rx) = &refresh_rx {
                match rx.try_recv() {
                    Ok(Ok(new_prs)) => {
                        // Compute changes for bell/notification alerts.
                        let old_needs: HashSet<String> = state
                            .prs
                            .iter()
                            .filter(|p| p.category == Category::NeedsYou)
                            .map(|p| p.pr.pr_key.clone())
                            .collect();
                        let old_ready_to_merge: HashSet<String> = state
                            .prs
                            .iter()
                            .filter(|p| p.category == Category::ReadyToMerge)
                            .map(|p| p.pr.pr_key.clone())
                            .collect();
                        let prs_entered_needs_you: Vec<&UiPr> = new_prs
                            .iter()
                            .filter(|p| {
                                p.category == Category::NeedsYou
                                    && !old_needs.contains(&p.pr.pr_key)
                            })
                            .collect();
                        let prs_became_ready_to_merge: Vec<&UiPr> = new_prs
                            .iter()
                            .filter(|p| {
                                p.category == Category::ReadyToMerge
                                    && !old_ready_to_merge.contains(&p.pr.pr_key)
                            })
                            .collect();
                        let prs_new_ci_failure: Vec<&UiPr> = new_prs
                            .iter()
                            .filter(|p| p.is_new_ci_failure)
                            .collect();
                        let prs_new_review_request: Vec<&UiPr> = new_prs
                            .iter()
                            .filter(|p| p.is_new_review_request)
                            .collect();

                        // Detect new repos.
                        let current_repos: HashSet<String> = new_prs
                            .iter()
                            .map(|p| format!("{}/{}", p.pr.owner, p.pr.repo))
                            .collect();
                        let new_repos: Vec<String> = current_repos
                            .iter()
                            .filter(|r| !seen_repos.contains(*r))
                            .cloned()
                            .collect();

                        // Bell alert (terminal bell).
                        if bell_enabled
                            && (!prs_entered_needs_you.is_empty()
                                || !prs_new_ci_failure.is_empty())
                        {
                            let _ = execute!(terminal.backend_mut(), Print("\x07"));
                        }

                        // OS notifications.
                        if notify_enabled {
                            // Notify for new repos.
                            for repo in &new_repos {
                                crate::notify::notify_new_repo(repo);
                            }
                            // Notify for new CI failures (up to 3).
                            for pr in prs_new_ci_failure.iter().take(3) {
                                let repo = format!("{}/{}", pr.pr.owner, pr.pr.repo);
                                crate::notify::notify_ci_failure(&pr.pr.title, &repo);
                            }
                            // Notify for new review requests (up to 3).
                            for pr in prs_new_review_request.iter().take(3) {
                                let repo = format!("{}/{}", pr.pr.owner, pr.pr.repo);
                                crate::notify::notify_review_requested(&pr.pr.title, &repo);
                            }
                            // Summary notification if PRs entered "Needs You".
                            if !prs_entered_needs_you.is_empty() {
                                crate::notify::notify_needs_you(prs_entered_needs_you.len());
                            }
                            // Notify for PRs that became ready to merge (up to 3).
                            for pr in prs_became_ready_to_merge.iter().take(3) {
                                let repo = format!("{}/{}", pr.pr.owner, pr.pr.repo);
                                crate::notify::notify_ready_to_merge(&pr.pr.title, &repo);
                            }
                        }

                        // Update seen repos set.
                        seen_repos = current_repos;

                        state.prs = new_prs;
                        state.refreshing = false;
                        refresh_rx = None;
                    }
                    Ok(Err(_e)) => {
                        // Keep V1 minimal: stop refreshing; errors surface on next startup/log.
                        state.refreshing = false;
                        refresh_rx = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        state.refreshing = false;
                        refresh_rx = None;
                    }
                }
            }
        }

        let area = terminal
            .size()
            .map_err(|e| format!("Failed to read terminal size: {e}"))?;
        let inner_height = area.height.saturating_sub(2); // borders
        let inner_width = area.width.saturating_sub(2); // borders
        let content_height = inner_height.saturating_sub(1); // footer rendered separately at bottom

        let (lines, visible) = if state.mode == ViewMode::List {
            let filtered = filtered_indices(
                &state.prs,
                &state.filter_query,
                state.only_needs_you,
                state.only_failing_ci,
                state.only_review_requested,
            );
            let mut banner = String::new();
            if state.filter_editing {
                banner = format!("Filter: {} (Esc back)", state.filter_edit);
            } else if !state.filter_query.is_empty()
                || state.only_needs_you
                || state.only_failing_ci
                || state.only_review_requested
            {
                let mut parts: Vec<String> = Vec::new();
                if !state.filter_query.is_empty() {
                    parts.push(format!("q=\"{}\"", state.filter_query));
                }
                if state.only_needs_you {
                    parts.push("needs".to_string());
                }
                if state.only_failing_ci {
                    parts.push("failing".to_string());
                }
                if state.only_review_requested {
                    parts.push("review".to_string());
                }
                banner = format!("Filter: {}", parts.join("  "));
            }
            let banner_opt = if banner.is_empty() {
                None
            } else {
                Some(banner.as_str())
            };
            let (l, v) = build_list_lines(
                &state.prs,
                inner_width,
                content_height,
                state.selected_idx,
                &filtered,
                banner_opt,
                state.ui,
            );
            (l, v)
        } else {
            let key = state.details_pr_key.clone();
            let maybe = key.and_then(|k| state.prs.iter().find(|p| p.pr.pr_key == k).cloned());
            if let Some(pr) = maybe {
                (
                    build_details_lines(
                        &pr,
                        inner_width,
                        content_height,
                        state.details_ci_selected,
                    ),
                    Vec::new(),
                )
            } else {
                state.mode = ViewMode::List;
                let filtered = filtered_indices(
                    &state.prs,
                    &state.filter_query,
                    state.only_needs_you,
                    state.only_failing_ci,
                    state.only_review_requested,
                );
                let (l, v) = build_list_lines(
                    &state.prs,
                    inner_width,
                    content_height,
                    state.selected_idx,
                    &filtered,
                    None,
                    state.ui,
                );
                (l, v)
            }
        };
        let footer_line = build_footer(
            inner_width,
            state.mode,
            state.refreshing,
            state.shimmer_phase,
            state.mode == ViewMode::List && state.filter_editing,
        );
        let visible_for_events = visible;
        if state.mode == ViewMode::List {
            clamp_selection(&mut state.selected_idx, visible_for_events.len());
        }

        terminal
            .draw(|f| {
                let area = f.area();
                let block = Block::default().borders(Borders::ALL);
                let inner = block.inner(area);
                f.render_widget(block, area);
                let parts = Layout::default()
                    .constraints([Constraint::Min(0), Constraint::Length(1)])
                    .split(inner);

                // Content (top)
                let text = Text::from(lines.clone());
                let content = Paragraph::new(text);
                f.render_widget(content, parts[0]);

                // Footer (bottom): update notice on the left, controls on the right.
                let footer_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(parts[1]);

                let left_line = if let Some(msg) = state.update_notice.as_deref() {
                    let w = footer_chunks[0].width.max(1) as usize;
                    let txt = truncate_ellipsis(msg, w.saturating_sub(1));
                    Line::from(Span::styled(
                        txt,
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ))
                } else {
                    Line::from("")
                };
                let footer_left = Paragraph::new(left_line).alignment(Alignment::Left);
                f.render_widget(footer_left, footer_chunks[0]);

                let footer_right = Paragraph::new(footer_line.clone()).alignment(Alignment::Right);
                f.render_widget(footer_right, footer_chunks[1]);

                if state.help_open {
                    // Centered modal overlay.
                    let w = (inner.width as f32 * 0.85) as u16;
                    let h = (inner.height as f32 * 0.70) as u16;
                    let popup = ratatui::layout::Rect {
                        x: inner.x + (inner.width.saturating_sub(w)) / 2,
                        y: inner.y + (inner.height.saturating_sub(h)) / 2,
                        width: w.max(20),
                        height: h.max(8),
                    };
                    f.render_widget(Clear, popup);
                    let b = Block::default().title("Help").borders(Borders::ALL);
                    let t = Text::from(help_lines());
                    let p = Paragraph::new(t).block(b).wrap(Wrap { trim: false });
                    f.render_widget(p, popup);
                }
            })
            .map_err(|e| format!("Draw failed: {e}"))?;

        // Keep the UI responsive on quit/navigation.
        if event::poll(Duration::from_millis(50)).map_err(|e| format!("Event poll failed: {e}"))? {
            if let Event::Key(k) = event::read().map_err(|e| format!("Event read failed: {e}"))? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                if state.help_open {
                    match k.code {
                        KeyCode::Char('?') | KeyCode::Esc => state.help_open = false,
                        KeyCode::Char('q') => state.help_open = false,
                        _ => {}
                    }
                    continue;
                }

                if state.filter_editing {
                    match (k.code, k.modifiers) {
                        (KeyCode::Up, _) => {
                            if state.mode == ViewMode::List {
                                let n = visible_for_events.len();
                                if n == 0 {
                                    // nothing to select
                                } else if state.selected_idx == 0 {
                                    state.selected_idx = n - 1;
                                } else {
                                    state.selected_idx -= 1;
                                }
                            }
                        }
                        (KeyCode::Down, _) => {
                            if state.mode == ViewMode::List {
                                let n = visible_for_events.len();
                                if n == 0 {
                                    // nothing to select
                                } else if state.selected_idx + 1 >= n {
                                    state.selected_idx = 0;
                                } else {
                                    state.selected_idx += 1;
                                }
                            }
                        }
                        (KeyCode::Esc, _) => {
                            // Exit filter mode and clear the filter text (back to unfiltered list).
                            state.filter_prev_query.clear();
                            state.filter_edit.clear();
                            state.filter_query.clear();
                            state.filter_editing = false;
                            state.selected_idx = 0;
                        }
                        (KeyCode::Backspace, _) => {
                            state.filter_edit.pop();
                            state.filter_query = state.filter_edit.clone();
                        }
                        (KeyCode::Enter, _) => {
                            // Live filtering already applied; Enter opens the selected PR (same as list mode).
                            if state.mode == ViewMode::List {
                                if let Some(pr_idx) =
                                    visible_for_events.get(state.selected_idx).copied()
                                {
                                    if let Some(pr) = state.prs.get_mut(pr_idx) {
                                        open_in_browser(&pr.pr.url);
                                    }
                                }
                            }
                            state.filter_editing = false;
                            state.filter_edit.clear();
                        }
                        (KeyCode::Char('x'), m) if m.contains(KeyModifiers::CONTROL) => {
                            state.filter_prev_query.clear();
                            state.filter_edit.clear();
                            state.filter_query.clear();
                            state.only_needs_you = false;
                            state.only_failing_ci = false;
                            state.only_review_requested = false;
                            state.selected_idx = 0;
                        }
                        (KeyCode::Char('n'), m) if m.contains(KeyModifiers::CONTROL) => {
                            state.only_needs_you = !state.only_needs_you;
                            state.selected_idx = 0;
                        }
                        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                            state.only_failing_ci = !state.only_failing_ci;
                            state.selected_idx = 0;
                        }
                        (KeyCode::Char('v'), m) if m.contains(KeyModifiers::CONTROL) => {
                            state.only_review_requested = !state.only_review_requested;
                            state.selected_idx = 0;
                        }
                        (KeyCode::Char(ch), _) => {
                            if !ch.is_control() {
                                state.filter_edit.push(ch);
                                state.filter_query = state.filter_edit.clone();
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                match k.code {
                    KeyCode::Char('?') => {
                        state.help_open = !state.help_open;
                    }
                    KeyCode::Esc => {
                        // In list mode, Esc clears any active filter/toggles even when not currently typing.
                        if state.mode == ViewMode::List {
                            if !state.filter_query.is_empty()
                                || state.only_needs_you
                                || state.only_failing_ci
                                || state.only_review_requested
                            {
                                state.filter_query.clear();
                                state.only_needs_you = false;
                                state.only_failing_ci = false;
                                state.only_review_requested = false;
                                state.selected_idx = 0;
                            }
                        }
                    }
                    KeyCode::Char('q') => break,
                    KeyCode::Char('r') => {
                        if !state.refreshing {
                            state.refreshing = true;
                            state.shimmer_phase = 0;
                            state.last_refresh_started = Some(Instant::now());
                            let (tx, rx) = mpsc::channel();
                            refresh_rx = Some(rx);
                            // Run the refresh off-thread so we can animate shimmer + keep quit responsive.
                            // Note: closure may block on network.
                            let rf = Arc::clone(&refresh_fn);
                            std::thread::spawn(move || {
                                let res = rf();
                                let _ = tx.send(res);
                            });
                        }
                    }
                    KeyCode::Char('f') => {
                        if state.mode == ViewMode::Details {
                            let pr_opt = state
                                .details_pr_key
                                .as_ref()
                                .and_then(|k| state.prs.iter_mut().find(|p| &p.pr.pr_key == k));
                            if let Some(pr) = pr_opt {
                                let url = pr
                                    .pr
                                    .ci_checks
                                    .iter()
                                    .find(|c| c.state.is_failure())
                                    .and_then(|c| c.url.as_deref())
                                    .unwrap_or(pr.pr.url.as_str());
                                open_in_browser(url);
                            }
                        }
                    }
                    KeyCode::Char('/') => {
                        if state.mode == ViewMode::List && !state.filter_editing {
                            state.filter_editing = true;
                            state.filter_prev_query = state.filter_query.clone();
                            state.filter_edit = state.filter_query.clone();
                            state.selected_idx = 0;
                        }
                    }
                    KeyCode::Char('x') => {
                        if state.mode == ViewMode::List && !state.filter_editing {
                            state.filter_query.clear();
                            state.only_needs_you = false;
                            state.only_failing_ci = false;
                            state.only_review_requested = false;
                            state.selected_idx = 0;
                        }
                    }
                    KeyCode::Char('n') => {
                        if state.mode == ViewMode::List && !state.filter_editing {
                            state.only_needs_you = !state.only_needs_you;
                            state.selected_idx = 0;
                        }
                    }
                    KeyCode::Char('c') => {
                        if state.mode == ViewMode::List && !state.filter_editing {
                            state.only_failing_ci = !state.only_failing_ci;
                            state.selected_idx = 0;
                        }
                    }
                    KeyCode::Char('v') => {
                        if state.mode == ViewMode::List && !state.filter_editing {
                            state.only_review_requested = !state.only_review_requested;
                            state.selected_idx = 0;
                        }
                    }
                    KeyCode::Tab => {
                        if state.mode == ViewMode::List {
                            if let Some(pr_idx) =
                                visible_for_events.get(state.selected_idx).copied()
                            {
                                if let Some(pr) = state.prs.get(pr_idx) {
                                    state.details_pr_key = Some(pr.pr.pr_key.clone());
                                    state.mode = ViewMode::Details;
                                    state.details_ci_selected = 0;
                                    state.details_last_auto_refresh = Some(Instant::now());
                                }
                            }
                        } else {
                            state.mode = ViewMode::List;
                            state.details_last_auto_refresh = None;
                        }
                    }
                    KeyCode::Up => {
                        if state.mode == ViewMode::List {
                            let n = visible_for_events.len();
                            if n == 0 {
                                // nothing to select
                            } else if state.selected_idx == 0 {
                                state.selected_idx = n - 1;
                            } else {
                                state.selected_idx -= 1;
                            }
                        } else {
                            if state.details_ci_selected > 0 {
                                state.details_ci_selected -= 1;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if state.mode == ViewMode::List {
                            let n = visible_for_events.len();
                            if n == 0 {
                                // nothing to select
                            } else if state.selected_idx + 1 >= n {
                                state.selected_idx = 0;
                            } else {
                                state.selected_idx += 1;
                            }
                        } else {
                            // Clamp based on selected PR's available CI checks.
                            let ci_len = state
                                .details_pr_key
                                .as_ref()
                                .and_then(|k| state.prs.iter().find(|p| &p.pr.pr_key == k))
                                .map(|p| p.pr.ci_checks.len())
                                .unwrap_or(0);
                            if ci_len > 0 && state.details_ci_selected + 1 < ci_len {
                                state.details_ci_selected += 1;
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if state.mode == ViewMode::List {
                            if let Some(pr_idx) =
                                visible_for_events.get(state.selected_idx).copied()
                            {
                                if let Some(pr) = state.prs.get_mut(pr_idx) {
                                    open_in_browser(&pr.pr.url);
                                }
                            }
                        } else {
                            // In details view, Enter opens the selected CI check URL if present, else PR URL.
                            let pr_opt = state
                                .details_pr_key
                                .as_ref()
                                .and_then(|k| state.prs.iter_mut().find(|p| &p.pr.pr_key == k));
                            if let Some(pr) = pr_opt {
                                let url = pr
                                    .pr
                                    .ci_checks
                                    .get(state.details_ci_selected)
                                    .and_then(|c| c.url.as_deref())
                                    .unwrap_or(pr.pr.url.as_str());
                                open_in_browser(url);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode().map_err(|e| format!("Failed to disable raw mode: {e}"))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| format!("Failed to leave alt screen: {e}"))?;
    terminal
        .show_cursor()
        .map_err(|e| format!("Failed to show cursor: {e}"))?;
    Ok(())
}
