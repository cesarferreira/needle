use crate::db::{now_unix, set_last_opened_at};
use crate::refresh::{Category, UiPr};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::tty::IsTty;
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use rusqlite::Connection;
use std::io::{self, Stdout};
use std::process::Command;
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Details,
}

pub struct AppState {
    pub prs: Vec<UiPr>,
    pub selected_idx: usize, // index into visible_pr_indices
    pub(crate) mode: ViewMode,
    pub(crate) details_pr_key: Option<String>,
}

impl AppState {
    pub fn new(prs: Vec<UiPr>) -> Self {
        Self {
            prs,
            selected_idx: 0,
            mode: ViewMode::List,
            details_pr_key: None,
        }
    }
}

fn category_title(cat: Category) -> &'static str {
    match cat {
        Category::NeedsYou => "🔥 NEEDS YOU",
        Category::Waiting => "⏳ WAITING",
        Category::Stale => "⚠️ STALE",
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

fn build_list_lines(
    prs: &[UiPr],
    inner_width: u16,
    inner_height: u16,
    selected_visible_idx: usize,
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

    // Table-ish column sizing (dynamic; only truncates when the terminal width forces it).
    // Columns: prefix(2) repo(var) author(var) #num(var) title(var) status(var)
    let iw = inner_width as usize;
    let prefix_w = 2usize;
    let sep_w = 2usize; // two spaces between columns

    let max_repo_len = prs
        .iter()
        .map(|p| {
            let s = format!("{}/{}", p.pr.owner, p.pr.repo);
            UnicodeWidthStr::width(s.as_str())
        })
        .max()
        .unwrap_or(10);
    let max_author_len = prs
        .iter()
        .map(|p| UnicodeWidthStr::width(p.pr.author.as_str()))
        .max()
        .unwrap_or(6);
    let max_num_len = prs
        .iter()
        .map(|p| {
            let s = format!("#{}", p.pr.number);
            UnicodeWidthStr::width(s.as_str())
        })
        .max()
        .unwrap_or(4);
    let max_status_len = prs
        .iter()
        .map(|p| UnicodeWidthStr::width(p.display_status.as_str()))
        .max()
        .unwrap_or(10);

    // Reasonable upper bounds so title keeps most of the width,
    // but allow longer statuses like "CI running (123m)" without truncation.
    let status_w = max_status_len.clamp(12, 34);
    let num_w = max_num_len.clamp(4, 8);
    let author_w = max_author_len.clamp(6, 16);

    // Ensure title gets at least 16 chars; repo uses remaining but capped.
    let min_title_w = 16usize;
    let max_repo_w = 35usize;
    let mut repo_w = max_repo_len.min(max_repo_w);

    // Compute remaining for title and shrink repo if needed.
    let fixed = prefix_w + repo_w + sep_w + author_w + sep_w + num_w + sep_w + status_w + sep_w;
    let mut title_w = iw.saturating_sub(fixed);
    if title_w < min_title_w {
        let missing = min_title_w - title_w;
        repo_w = repo_w.saturating_sub(missing);
        let fixed2 = prefix_w + repo_w + sep_w + author_w + sep_w + num_w + sep_w + status_w + sep_w;
        title_w = iw.saturating_sub(fixed2);
    }
    if title_w < 8 {
        // Extremely narrow terminal; keep things from going negative.
        title_w = 8;
    }

    let cats = [Category::NeedsYou, Category::Waiting, Category::Stale];
    for cat in cats {
        // Skip empty sections entirely.
        if !prs.iter().any(|p| p.category == cat) {
            continue;
        }

        let start_len = lines.len();

        // Header + divider
        push_line(&mut lines, inner_height, Line::from(Span::styled(
            category_title(cat).to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::raw(std::iter::repeat('─').take(iw).collect::<String>())),
        );
        push_line(
            &mut lines,
            inner_height,
            Line::from(Span::styled(
                format!(
                    "  {}  {}  {}  {}  {}",
                    pad_right("REPO", repo_w),
                    pad_right("AUTHOR", author_w),
                    pad_right("PR", num_w),
                    pad_right("TITLE", title_w),
                    pad_right("STATUS", status_w)
                ),
                Style::default().add_modifier(Modifier::DIM),
            )),
        );

        // Rows in this category
        for (idx, pr) in prs.iter().enumerate() {
            if pr.category != cat {
                continue;
            }
            if (lines.len() as u16) >= inner_height {
                break;
            }
            let visible_idx = visible_pr_indices.len();
            visible_pr_indices.push(idx);

            let is_selected = visible_idx == selected_visible_idx;
            let prefix = if is_selected { "> " } else { "  " };
            let repo = truncate_ellipsis(&format!("{}/{}", pr.pr.owner, pr.pr.repo), repo_w);
            let repo = pad_right(&repo, repo_w);

            let author = truncate_ellipsis(&pr.pr.author, author_w);
            let author = pad_right(&author, author_w);

            let num = truncate_ellipsis(&format!("#{}", pr.pr.number), num_w);
            let num = pad_right(&num, num_w);

            let title = truncate_ellipsis(&pr.pr.title, title_w);
            let title = pad_right(&title, title_w);

            let status = truncate_ellipsis(&pr.display_status, status_w);
            let status = pad_right(&status, status_w);

            let recent_dim = pr
                .last_opened_at
                .map(|t| now_unix().saturating_sub(t) <= 3600)
                .unwrap_or(false);

            let style = if is_selected {
                // Highlight the whole row.
                Style::default().add_modifier(Modifier::REVERSED)
            } else if recent_dim {
                Style::default().add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };

            let line = Line::from(Span::styled(
                format!("{prefix}{repo}  {author}  {num}  {title}  {status}"),
                style,
            ));
            push_line(&mut lines, inner_height, line);
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

    (lines, visible_pr_indices)
}

fn build_footer(inner_width: u16, mode: ViewMode) -> Line<'static> {
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

    let mut segs: Vec<Seg> = Vec::new();
    match mode {
        ViewMode::List => {
            segs.extend([
                keycap("q"), label("quit"), sep(),
                keycap("r"), label("refresh"), sep(),
                keycap("Enter"), label("open"), sep(),
                keycap("Tab"), label("details"), sep(),
                keycap("↑/↓"), label("move"),
            ]);
        }
        ViewMode::Details => {
            segs.extend([
                keycap("Tab"), label("back"), sep(),
                keycap("Enter"), label("open"), sep(),
                keycap("r"), label("refresh"), sep(),
                keycap("q"), label("quit"),
            ]);
        }
    }

    let total_w: usize = segs
        .iter()
        .map(|s| UnicodeWidthStr::width(s.text.as_str()))
        .sum();
    let iw = inner_width as usize;

    // If it doesn't fit, fall back to a plain truncated hint (still right-aligned).
    if total_w > iw {
        let hint = match mode {
            ViewMode::List => "[q] quit  [r] refresh  [Enter] open  [Tab] details  [↑/↓] move",
            ViewMode::Details => "[Tab] back  [Enter] open  [r] refresh  [q] quit",
        };
        let s = truncate_ellipsis(hint, iw);
        let pad = iw.saturating_sub(UnicodeWidthStr::width(s.as_str()));
        return Line::from(vec![
            Span::raw(std::iter::repeat(' ').take(pad).collect::<String>()),
            Span::styled(s, Style::default().fg(Color::Gray)),
        ]);
    }

    let pad = iw.saturating_sub(total_w);
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::raw(std::iter::repeat(' ').take(pad).collect::<String>()));
    for s in segs {
        spans.push(Span::styled(s.text, s.style));
    }
    Line::from(spans)
}

fn build_details_lines(pr: &UiPr, inner_width: u16, inner_height: u16) -> Vec<Line<'static>> {
    let iw = inner_width as usize;
    let mut out: Vec<Line<'static>> = Vec::new();
    let now = now_unix();

    // Title line
    out.push(Line::from(Span::styled(
        truncate_ellipsis("DETAILS", iw),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    out.push(Line::from(Span::raw(std::iter::repeat('─').take(iw).collect::<String>())));

    let rows = [
        ("Repo", format!("{}/{}", pr.pr.owner, pr.pr.repo)),
        ("PR", format!("#{}", pr.pr.number)),
        ("Author", pr.pr.author.clone()),
        ("Title", pr.pr.title.clone()),
        ("Status", pr.display_status.clone()),
        ("Updated", human_age(now, pr.pr.updated_at_unix)),
        ("URL", pr.pr.url.clone()),
        ("Commit", pr.pr.last_commit_sha.clone().unwrap_or_else(|| "none".to_string())),
        (
            "Opened",
            pr.last_opened_at
                .map(|t| human_age(now, t))
                .unwrap_or_else(|| "never".to_string()),
        ),
    ];

    for (k, v) in rows {
        if (out.len() as u16) >= inner_height {
            break;
        }
        let line = format!("{k}: {v}");
        out.push(Line::from(Span::raw(truncate_ellipsis(&line, iw))));
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

pub fn run_tui(
    conn: &Connection,
    mut state: AppState,
    mut on_refresh: impl FnMut() -> Result<Vec<UiPr>, String>,
) -> Result<(), String> {
    if !io::stdin().is_tty() || !io::stdout().is_tty() {
        return Err("Not a TTY: run `needle` in an interactive terminal.".to_string());
    }
    enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {e}"))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| format!("Failed to enter alt screen: {e}"))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(backend).map_err(|e| format!("Failed to init terminal: {e}"))?;

    let mut last_visible: Vec<usize> = Vec::new();

    loop {
        let area = terminal
            .size()
            .map_err(|e| format!("Failed to read terminal size: {e}"))?;
        let inner_height = area.height.saturating_sub(2); // borders
        let inner_width = area.width.saturating_sub(2); // borders
        let content_height = inner_height.saturating_sub(1); // reserve one line for footer

        let (mut lines, visible) = if state.mode == ViewMode::List {
            let (l, v) = build_list_lines(&state.prs, inner_width, content_height, state.selected_idx);
            (l, v)
        } else {
            let key = state.details_pr_key.clone();
            let maybe = key.and_then(|k| state.prs.iter().find(|p| p.pr.pr_key == k).cloned());
            if let Some(pr) = maybe {
                (build_details_lines(&pr, inner_width, content_height), Vec::new())
            } else {
                state.mode = ViewMode::List;
                let (l, v) = build_list_lines(&state.prs, inner_width, content_height, state.selected_idx);
                (l, v)
            }
        };
        // Footer
        if (lines.len() as u16) < inner_height {
            lines.push(build_footer(inner_width, state.mode));
        }

        last_visible = visible;
        if state.mode == ViewMode::List {
            clamp_selection(&mut state.selected_idx, last_visible.len());
        }

        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .constraints([Constraint::Percentage(100)])
                    .split(f.area());

                let block = Block::default().borders(Borders::ALL);

                let text = Text::from(lines.clone());
                let paragraph = Paragraph::new(text).block(block);
                f.render_widget(paragraph, chunks[0]);
            })
            .map_err(|e| format!("Draw failed: {e}"))?;

        // Keep the UI responsive on quit/navigation.
        if event::poll(Duration::from_millis(50)).map_err(|e| format!("Event poll failed: {e}"))? {
            if let Event::Key(k) = event::read().map_err(|e| format!("Event read failed: {e}"))? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match k.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('r') => {
                        let new_prs = on_refresh()?;
                        state.prs = new_prs;
                        // selection clamped next loop after recompute of visible items
                    }
                    KeyCode::Tab => {
                        if state.mode == ViewMode::List {
                            if let Some(pr_idx) = last_visible.get(state.selected_idx).copied() {
                                if let Some(pr) = state.prs.get(pr_idx) {
                                    state.details_pr_key = Some(pr.pr.pr_key.clone());
                                    state.mode = ViewMode::Details;
                                }
                            }
                        } else {
                            state.mode = ViewMode::List;
                        }
                    }
                    KeyCode::Up => {
                        if state.mode == ViewMode::List && state.selected_idx > 0 {
                            state.selected_idx -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if state.mode == ViewMode::List && state.selected_idx + 1 < last_visible.len() {
                            state.selected_idx += 1;
                        }
                    }
                    KeyCode::Enter => {
                        let pr_idx_opt = if state.mode == ViewMode::List {
                            last_visible.get(state.selected_idx).copied()
                        } else {
                            state
                                .details_pr_key
                                .as_ref()
                                .and_then(|k| state.prs.iter().position(|p| &p.pr.pr_key == k))
                        };

                        if let Some(pr_idx) = pr_idx_opt {
                            if let Some(pr) = state.prs.get_mut(pr_idx) {
                                open_in_browser(&pr.pr.url);
                                let ts = now_unix();
                                pr.last_opened_at = Some(ts);
                                let _ = set_last_opened_at(conn, &pr.pr.pr_key, ts);
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
    terminal.show_cursor().map_err(|e| format!("Failed to show cursor: {e}"))?;
    Ok(())
}


