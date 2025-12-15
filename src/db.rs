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
    pub author: Option<String>,
    pub updated_at_unix: Option<i64>,

    pub last_commit_sha: Option<String>,
    pub last_ci_state: Option<String>,
    pub last_review_state: Option<String>,
    pub ci_checks_json: Option<String>,
    pub is_draft: Option<i64>,
    pub mergeable: Option<String>,
    pub merge_state_status: Option<String>,
    pub author_is_viewer: Option<i64>,

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
    migrate_schema(&conn)?;
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
  author TEXT,
  updated_at_unix INTEGER,

  last_commit_sha TEXT,
  last_ci_state TEXT,              -- success | failure | running | none
  last_review_state TEXT,          -- requested | approved | none
  ci_checks_json TEXT,             -- JSON array of CI checks (optional)
  is_draft INTEGER,                -- 0/1
  mergeable TEXT,                  -- GitHub enum as string
  merge_state_status TEXT,         -- GitHub enum as string
  author_is_viewer INTEGER,        -- 0/1

  last_seen_at INTEGER,            -- unix timestamp
  last_opened_at INTEGER           -- unix timestamp
);
"#,
    )
    .map_err(|e| format!("Failed to init schema: {e}"))?;
    Ok(())
}

fn migrate_schema(conn: &Connection) -> Result<(), String> {
    // Minimal forward-only migrations: add columns if missing.
    let mut stmt = conn
        .prepare("PRAGMA table_info(prs)")
        .map_err(|e| format!("Failed to read schema info: {e}"))?;
    let cols_iter = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("Failed to query schema info: {e}"))?;
    let mut existing = std::collections::HashSet::new();
    for c in cols_iter {
        existing.insert(c.map_err(|e| format!("Failed to decode schema info: {e}"))?);
    }

    fn add_if_missing(
        conn: &Connection,
        existing: &std::collections::HashSet<String>,
        name: &str,
        sql_type: &str,
    ) -> Result<(), String> {
        if existing.contains(name) {
            return Ok(());
        }
        conn.execute(&format!("ALTER TABLE prs ADD COLUMN {name} {sql_type}"), [])
            .map_err(|e| format!("Failed to migrate schema (add {name}): {e}"))?;
        Ok(())
    }

    add_if_missing(conn, &existing, "author", "TEXT")?;
    add_if_missing(conn, &existing, "updated_at_unix", "INTEGER")?;
    add_if_missing(conn, &existing, "ci_checks_json", "TEXT")?;
    add_if_missing(conn, &existing, "is_draft", "INTEGER")?;
    add_if_missing(conn, &existing, "mergeable", "TEXT")?;
    add_if_missing(conn, &existing, "merge_state_status", "TEXT")?;
    add_if_missing(conn, &existing, "author_is_viewer", "INTEGER")?;

    Ok(())
}

pub fn load_all_prs(conn: &Connection) -> Result<HashMap<String, DbPrRow>, String> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  pr_key, owner, repo, number, title, url, author, updated_at_unix,
  last_commit_sha, last_ci_state, last_review_state,
  ci_checks_json, is_draft, mergeable, merge_state_status, author_is_viewer,
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
            author: row.get(6).map_err(|e| format!("Row decode: {e}"))?,
            updated_at_unix: row.get(7).map_err(|e| format!("Row decode: {e}"))?,
            last_commit_sha: row.get(8).map_err(|e| format!("Row decode: {e}"))?,
            last_ci_state: row.get(9).map_err(|e| format!("Row decode: {e}"))?,
            last_review_state: row.get(10).map_err(|e| format!("Row decode: {e}"))?,
            ci_checks_json: row.get(11).map_err(|e| format!("Row decode: {e}"))?,
            is_draft: row.get(12).map_err(|e| format!("Row decode: {e}"))?,
            mergeable: row.get(13).map_err(|e| format!("Row decode: {e}"))?,
            merge_state_status: row.get(14).map_err(|e| format!("Row decode: {e}"))?,
            author_is_viewer: row.get(15).map_err(|e| format!("Row decode: {e}"))?,
            last_seen_at: row.get(16).map_err(|e| format!("Row decode: {e}"))?,
            last_opened_at: row.get(17).map_err(|e| format!("Row decode: {e}"))?,
        };
        out.insert(pr.pr_key.clone(), pr);
    }
    Ok(out)
}

pub fn upsert_pr(conn: &Connection, pr: &DbPrRow, last_seen_at: i64) -> Result<(), String> {
    conn.execute(
        r#"
INSERT INTO prs (
  pr_key, owner, repo, number, title, url, author, updated_at_unix,
  last_commit_sha, last_ci_state, last_review_state,
  ci_checks_json, is_draft, mergeable, merge_state_status,
  author_is_viewer, last_seen_at, last_opened_at
) VALUES (
  ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
  ?9, ?10, ?11,
  ?12, ?13, ?14, ?15,
  ?16, ?17, ?18
)
ON CONFLICT(pr_key) DO UPDATE SET
  owner = excluded.owner,
  repo = excluded.repo,
  number = excluded.number,
  title = excluded.title,
  url = excluded.url,
  author = excluded.author,
  updated_at_unix = excluded.updated_at_unix,
  last_commit_sha = excluded.last_commit_sha,
  last_ci_state = excluded.last_ci_state,
  last_review_state = excluded.last_review_state,
  ci_checks_json = excluded.ci_checks_json,
  is_draft = excluded.is_draft,
  mergeable = excluded.mergeable,
  merge_state_status = excluded.merge_state_status,
  author_is_viewer = excluded.author_is_viewer,
  last_seen_at = excluded.last_seen_at
"#,
        params![
            pr.pr_key,
            pr.owner,
            pr.repo,
            pr.number,
            pr.title,
            pr.url,
            pr.author,
            pr.updated_at_unix,
            pr.last_commit_sha,
            pr.last_ci_state,
            pr.last_review_state,
            pr.ci_checks_json,
            pr.is_draft,
            pr.mergeable,
            pr.merge_state_status,
            pr.author_is_viewer,
            last_seen_at,
            pr.last_opened_at
        ],
    )
    .map_err(|e| format!("Failed to upsert pr: {e}"))?;
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
