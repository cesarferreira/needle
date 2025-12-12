use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct DbPrRow {
    pub pr_key: String,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,

    pub last_commit_sha: Option<String>,
    pub last_ci_state: Option<String>,
    pub last_review_state: Option<String>,

    pub last_seen_at: Option<i64>,
    pub last_opened_at: Option<i64>,
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn db_path() -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or_else(|| "Failed to resolve data_dir()".to_string())?;
    Ok(base.join("needle").join("prs.sqlite"))
}

pub fn open_db(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create db dir: {e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("Failed to open sqlite db: {e}"))?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS prs (
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
"#,
    )
    .map_err(|e| format!("Failed to init schema: {e}"))?;
    Ok(())
}

pub fn load_all_prs(conn: &Connection) -> Result<HashMap<String, DbPrRow>, String> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  pr_key, owner, repo, number, title, url,
  last_commit_sha, last_ci_state, last_review_state,
  last_seen_at, last_opened_at
FROM prs
"#,
        )
        .map_err(|e| format!("Failed to prepare load query: {e}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| format!("Failed to query prs: {e}"))?;

    let mut out = HashMap::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("Failed to iterate prs: {e}"))?
    {
        let pr = DbPrRow {
            pr_key: row.get(0).map_err(|e| format!("Row decode: {e}"))?,
            owner: row.get(1).map_err(|e| format!("Row decode: {e}"))?,
            repo: row.get(2).map_err(|e| format!("Row decode: {e}"))?,
            number: row.get(3).map_err(|e| format!("Row decode: {e}"))?,
            title: row.get(4).map_err(|e| format!("Row decode: {e}"))?,
            url: row.get(5).map_err(|e| format!("Row decode: {e}"))?,
            last_commit_sha: row.get(6).map_err(|e| format!("Row decode: {e}"))?,
            last_ci_state: row.get(7).map_err(|e| format!("Row decode: {e}"))?,
            last_review_state: row.get(8).map_err(|e| format!("Row decode: {e}"))?,
            last_seen_at: row.get(9).map_err(|e| format!("Row decode: {e}"))?,
            last_opened_at: row.get(10).map_err(|e| format!("Row decode: {e}"))?,
        };
        out.insert(pr.pr_key.clone(), pr);
    }
    Ok(out)
}

pub fn upsert_pr(conn: &Connection, pr: &DbPrRow, last_seen_at: i64) -> Result<(), String> {
    conn.execute(
        r#"
INSERT INTO prs (
  pr_key, owner, repo, number, title, url,
  last_commit_sha, last_ci_state, last_review_state,
  last_seen_at, last_opened_at
) VALUES (
  ?1, ?2, ?3, ?4, ?5, ?6,
  ?7, ?8, ?9,
  ?10, ?11
)
ON CONFLICT(pr_key) DO UPDATE SET
  owner = excluded.owner,
  repo = excluded.repo,
  number = excluded.number,
  title = excluded.title,
  url = excluded.url,
  last_commit_sha = excluded.last_commit_sha,
  last_ci_state = excluded.last_ci_state,
  last_review_state = excluded.last_review_state,
  last_seen_at = excluded.last_seen_at
"#,
        params![
            pr.pr_key,
            pr.owner,
            pr.repo,
            pr.number,
            pr.title,
            pr.url,
            pr.last_commit_sha,
            pr.last_ci_state,
            pr.last_review_state,
            last_seen_at,
            pr.last_opened_at
        ],
    )
    .map_err(|e| format!("Failed to upsert pr: {e}"))?;
    Ok(())
}

pub fn set_last_opened_at(conn: &Connection, pr_key: &str, ts: i64) -> Result<(), String> {
    let updated = conn
        .execute(
            "UPDATE prs SET last_opened_at = ?1 WHERE pr_key = ?2",
            params![ts, pr_key],
        )
        .map_err(|e| format!("Failed to update last_opened_at: {e}"))?;
    if updated == 0 {
        // Not fatal; it just means the PR is no longer in DB.
    }
    Ok(())
}

pub fn delete_prs_not_in(conn: &Connection, keep_pr_keys: &[String]) -> Result<(), String> {
    if keep_pr_keys.is_empty() {
        conn.execute("DELETE FROM prs", [])
            .map_err(|e| format!("Failed to delete prs: {e}"))?;
        return Ok(());
    }

    let placeholders = (0..keep_pr_keys.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("DELETE FROM prs WHERE pr_key NOT IN ({placeholders})");

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed to prepare delete query: {e}"))?;
    let refs: Vec<&str> = keep_pr_keys.iter().map(|s| s.as_str()).collect();
    stmt.execute(rusqlite::params_from_iter(refs))
        .map_err(|e| format!("Failed to delete old prs: {e}"))?;
    Ok(())
}


