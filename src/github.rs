use crate::model::{CiState, Pr, ReviewState};
use crate::timeutil::{parse_github_datetime_to_unix, unix_to_ymd};
use octocrab::Octocrab;
use std::collections::{HashMap, HashSet};

#[derive(Debug, serde::Serialize)]
struct PaginationVars {
    page_size: i32,
    cursor: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct GraphQlPayload<V> {
    query: &'static str,
    variables: V,
}

#[derive(Debug, serde::Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RepoOwner {
    login: String,
}

#[derive(Debug, serde::Deserialize)]
struct Repository {
    name: String,
    owner: RepoOwner,
}

#[derive(Debug, serde::Deserialize)]
struct StatusCheckRollup {
    state: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CommitInner {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<StatusCheckRollup>,
}

#[derive(Debug, serde::Deserialize)]
struct CommitNode {
    commit: Option<CommitInner>,
}

#[derive(Debug, serde::Deserialize)]
struct Commits {
    nodes: Option<Vec<CommitNode>>,
}

#[derive(Debug, serde::Deserialize)]
struct PullRequestNode {
    number: i64,
    title: String,
    url: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    repository: Repository,
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    commits: Option<Commits>,
}

#[derive(Debug, serde::Deserialize)]
struct ViewerPullRequests {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Option<Vec<PullRequestNode>>,
}

#[derive(Debug, serde::Deserialize)]
struct Viewer {
    #[serde(rename = "pullRequests")]
    pull_requests: ViewerPullRequests,
}

#[derive(Debug, serde::Deserialize)]
struct AuthoredData {
    viewer: Viewer,
}

#[derive(Debug, serde::Deserialize)]
struct GraphQlResponse<T> {
    data: T,
}

#[derive(Debug, serde::Deserialize)]
struct SearchResult {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Option<Vec<SearchNode>>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchNode {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    number: Option<i64>,
    title: Option<String>,
    url: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    repository: Option<Repository>,
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    commits: Option<Commits>,
}

impl SearchNode {
    fn into_pull_request(self) -> Option<PullRequestNode> {
        if self.typename.as_deref()? != "PullRequest" {
            return None;
        }
        Some(PullRequestNode {
            number: self.number?,
            title: self.title?,
            url: self.url?,
            updated_at: self.updated_at?,
            repository: self.repository?,
            head_ref_oid: self.head_ref_oid,
            review_decision: self.review_decision,
            commits: self.commits,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearchData {
    search: SearchResult,
}

const AUTHORED_QUERY: &str = r#"
query($page_size: Int!, $cursor: String) {
  viewer {
    pullRequests(first: $page_size, after: $cursor, states: OPEN, orderBy: {field: UPDATED_AT, direction: DESC}) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        title
        url
        updatedAt
        headRefOid
        reviewDecision
        repository { name owner { login } }
        commits(last: 1) {
          nodes {
            commit {
              statusCheckRollup { state }
            }
          }
        }
      }
    }
  }
}
"#;

const REVIEW_REQUESTED_QUERY: &str = r#"
query($page_size: Int!, $cursor: String, $search_query: String!) {
  search(query: $search_query, type: ISSUE, first: $page_size, after: $cursor) {
    pageInfo { hasNextPage endCursor }
    nodes {
      __typename
      ... on PullRequest {
        number
        title
        url
        updatedAt
        headRefOid
        reviewDecision
        repository { name owner { login } }
        commits(last: 1) {
          nodes {
            commit {
              statusCheckRollup { state }
            }
          }
        }
      }
    }
  }
}
"#;

fn map_ci_state(node: &PullRequestNode) -> CiState {
    let Some(commits) = &node.commits else {
        return CiState::None;
    };
    let Some(nodes) = &commits.nodes else {
        return CiState::None;
    };
    let Some(first) = nodes.first() else {
        return CiState::None;
    };
    let Some(commit) = &first.commit else {
        return CiState::None;
    };
    let Some(rollup) = &commit.status_check_rollup else {
        return CiState::None;
    };
    let state = rollup.state.as_deref().unwrap_or("");
    match state {
        "SUCCESS" => CiState::Success,
        "FAILURE" | "ERROR" => CiState::Failure,
        "PENDING" | "EXPECTED" => CiState::Running,
        _ => CiState::None,
    }
}

fn map_review_state(node: &PullRequestNode, is_requested: bool) -> ReviewState {
    if is_requested {
        return ReviewState::Requested;
    }
    match node.review_decision.as_deref() {
        Some("APPROVED") => ReviewState::Approved,
        _ => ReviewState::None,
    }
}

fn to_pr(node: PullRequestNode, is_requested: bool) -> Option<Pr> {
    let ci_state = map_ci_state(&node);
    let last_commit_sha = node.head_ref_oid.clone();
    let review_state = map_review_state(&node, is_requested);
    let owner = node.repository.owner.login;
    let repo = node.repository.name;
    let updated_at_unix = parse_github_datetime_to_unix(&node.updated_at)?;
    let pr_key = format!("{owner}/{repo}#{}", node.number);

    Some(Pr {
        pr_key,
        owner,
        repo,
        number: node.number,
        title: node.title,
        url: node.url,
        updated_at_unix,
        last_commit_sha,
        ci_state,
        review_state,
    })
}

pub async fn fetch_attention_prs(octo: &Octocrab, cutoff_ts: i64) -> Result<Vec<Pr>, String> {
    // Fetch authored PRs
    let mut authored: Vec<PullRequestNode> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let vars = PaginationVars {
            page_size: 50,
            cursor: cursor.clone(),
        };
        let payload = GraphQlPayload {
            query: AUTHORED_QUERY,
            variables: vars,
        };
        let resp: GraphQlResponse<AuthoredData> = octo
            .graphql(&payload)
            .await
            .map_err(|e| format!("GitHub GraphQL authored query failed: {e}"))?;

        if let Some(nodes) = resp.data.viewer.pull_requests.nodes {
            // Order is updatedAt DESC, so we can stop paginating once this page crosses cutoff.
            let mut keep = Vec::new();
            let mut min_updated: Option<i64> = None;
            for n in nodes {
                if let Some(u) = parse_github_datetime_to_unix(&n.updated_at) {
                    min_updated = Some(min_updated.map(|m| m.min(u)).unwrap_or(u));
                    if u >= cutoff_ts {
                        keep.push(n);
                    }
                }
            }
            authored.extend(keep);
            if min_updated.is_some_and(|m| m < cutoff_ts) {
                break;
            }
        }
        let pi = resp.data.viewer.pull_requests.page_info;
        if !pi.has_next_page {
            break;
        }
        cursor = pi.end_cursor;
        if cursor.is_none() {
            break;
        }
    }

    // Fetch review-requested PRs
    let cutoff_date = unix_to_ymd(cutoff_ts)
        .map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
        .unwrap_or_else(|| "1970-01-01".to_string());
    let search_query = format!(
        "is:pr is:open review-requested:@me sort:updated-desc updated:>={}",
        cutoff_date
    );

    let mut requested_keys: HashSet<String> = HashSet::new();
    let mut requested_nodes: Vec<PullRequestNode> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        #[derive(Debug, serde::Serialize)]
        struct SearchVars {
            page_size: i32,
            cursor: Option<String>,
            search_query: String,
        }

        let vars = SearchVars {
            page_size: 50,
            cursor: cursor.clone(),
            search_query: search_query.clone(),
        };
        let payload = GraphQlPayload {
            query: REVIEW_REQUESTED_QUERY,
            variables: vars,
        };
        let resp: GraphQlResponse<SearchData> = octo
            .graphql(&payload)
            .await
            .map_err(|e| format!("GitHub GraphQL review-requested query failed: {e}"))?;

        if let Some(nodes) = resp.data.search.nodes {
            let mut min_updated: Option<i64> = None;
            for n in nodes {
                if let Some(pr) = n.into_pull_request() {
                    if let Some(u) = parse_github_datetime_to_unix(&pr.updated_at) {
                        min_updated = Some(min_updated.map(|m| m.min(u)).unwrap_or(u));
                        if u < cutoff_ts {
                            continue;
                        }
                    }
                    let key = format!(
                        "{}/{}#{}",
                        pr.repository.owner.login, pr.repository.name, pr.number
                    );
                    requested_keys.insert(key);
                    requested_nodes.push(pr);
                }
            }
            if min_updated.is_some_and(|m| m < cutoff_ts) {
                break;
            }
        }
        let pi = resp.data.search.page_info;
        if !pi.has_next_page {
            break;
        }
        cursor = pi.end_cursor;
        if cursor.is_none() {
            break;
        }
    }

    // Merge & dedupe into Pr list, applying requested-review state when applicable.
    let mut by_key: HashMap<String, Pr> = HashMap::new();

    for node in authored {
        let key = format!(
            "{}/{}#{}",
            node.repository.owner.login, node.repository.name, node.number
        );
        if let Some(pr) = to_pr(node, requested_keys.contains(&key)) {
            by_key.insert(pr.pr_key.clone(), pr);
        }
    }

    for node in requested_nodes {
        let key = format!(
            "{}/{}#{}",
            node.repository.owner.login, node.repository.name, node.number
        );
        if let Some(pr) = to_pr(node, true) {
            by_key.insert(key, pr);
        }
    }

    Ok(by_key.into_values().collect())
}


