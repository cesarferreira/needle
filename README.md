# needle — PR Attention TUI (Rust)

Terminal-native TUI that shows **only** GitHub pull requests that may require your attention and lets you open a PR (or a CI check) in your browser.

This is **not** a GitHub client. It’s an **attention filter**.

## Requirements

- Rust (stable)
- A GitHub Personal Access Token in `GITHUB_TOKEN` (required)

## Install / Run

```bash
cd /Users/cesarferreira/code/github/needle
export GITHUB_TOKEN=...
cargo run
```

### Demo mode (no GitHub token)

```bash
cargo run -- --demo
```

### Filter window (days)

By default it only shows PRs updated in the **last 30 days**.

```bash
cargo run -- --days 7
```

## What it shows (V1 scope)

Included PRs:
- PRs **authored by you**
- PRs where **you are explicitly requested as a reviewer (User)**  
  (team review requests are ignored)

For each PR it computes:
- Latest commit SHA
- CI state (success/failure/running/none)
- Review request state (requested/approved/none)
- A hard-coded score → sorted desc → grouped into categories

## Controls

List view:
- `↑ / ↓`: move selection
- `Enter`: open selected PR in default browser (persists `last_opened_at`)
- `Tab`: open details view
- `r`: refresh now (shows shimmer while refreshing)
- `q`: quit

Details view:
- `↑ / ↓`: select CI check
- `Enter`: open selected CI check page (falls back to PR URL)
- `Tab`: back to list
- `r`: refresh now
- `q`: quit

## UI

- Single-screen list, visually grouped by derived category:
  - **NEEDS YOU** (score >= 40)
  - **WAITING** (0..39)
  - **STALE** (< 0)
- Empty sections are hidden.
- Rows are dimmed if `last_opened_at` is recent.
- No scrolling beyond terminal height (truncates to fit).
- Uses cached SQLite data to render immediately, then refreshes in the background.

### Details view CI checks

In details view you get a list of CI steps (check runs / status contexts):
- ✅ success
- ❌ failed
- 🟡 running (shows “running for …” when `startedAt` is available)

## Data storage (SQLite)

Stores a local snapshot for diffing/scoring at:
- `dirs::data_dir()/needle/prs.sqlite`

Schema (fixed in V1):

```sql
CREATE TABLE prs (
  pr_key TEXT PRIMARY KEY,        -- "{owner}/{repo}#{number}"
  owner TEXT NOT NULL,
  repo TEXT NOT NULL,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  url TEXT NOT NULL,

  last_commit_sha TEXT,
  last_ci_state TEXT,              -- success | failure | running | none
  last_review_state TEXT,          -- requested | approved | none

  last_seen_at INTEGER,            -- unix timestamp
  last_opened_at INTEGER           -- unix timestamp
);
```

On refresh, DB rows not in the current “attention set” are removed so cached startup stays consistent.

## Refresh behavior

- Fetches on startup **in the background** (UI shows cached data immediately).
- Manual refresh: `r`
- Auto refresh: every **30s** while on the details view
- No background async tasks beyond the single refresh worker thread.

## Scoring (hard-coded)

Each PR gets a score (higher = more urgent):

```
+50  review requested from user
+40  CI failed AND state changed since last_seen
+20  CI running longer than 10 minutes
+15  approved but unmerged for >24h
-20  waiting on others (no review requested, CI green)
-30  CI failed but unchanged since last_seen
```

Sort:
- Score desc
- Then by updated timestamp desc

## Troubleshooting

- **Missing token**: set `GITHUB_TOKEN`.
- **Not a TTY**: run in an interactive terminal (not a non-tty runner).

