use crate::model::{CiCheck, CiCheckState, CiState, Pr, ReviewState};
use crate::timeutil::{parse_github_datetime_to_unix, unix_to_ymd};
use octocrab::Octocrab;
use std::collections::HashMap;

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
struct Author {
    login: String,
}

#[derive(Debug, serde::Deserialize)]
struct ReviewRequestConnection {
    nodes: Option<Vec<ReviewRequestNode>>,
}

#[derive(Debug, serde::Deserialize)]
struct ReviewRequestNode {
    #[serde(rename = "requestedReviewer")]
    requested_reviewer: Option<RequestedReviewer>,
}

#[derive(Debug, serde::Deserialize)]
struct RequestedReviewer {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    login: Option<String>, // User
}

#[derive(Debug, serde::Deserialize)]
struct StatusCheckRollup {
    state: Option<String>,
    contexts: Option<StatusContexts>,
}

#[derive(Debug, serde::Deserialize)]
struct StatusContexts {
    nodes: Option<Vec<StatusContextNode>>,
}

#[derive(Debug, serde::Deserialize)]
struct StatusContextNode {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    // CheckRun
    name: Option<String>,
    conclusion: Option<String>,
    #[serde(rename = "detailsUrl")]
    details_url: Option<String>,
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    // StatusContext
    context: Option<String>,
    state: Option<String>,
    #[serde(rename = "targetUrl")]
    target_url: Option<String>,
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
    author: Option<Author>,
    #[serde(rename = "reviewRequests")]
    review_requests: Option<ReviewRequestConnection>,
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
    login: String,
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
    author: Option<Author>,
    #[serde(rename = "reviewRequests")]
    review_requests: Option<ReviewRequestConnection>,
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
            author: self.author,
            review_requests: self.review_requests,
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
    login
    pullRequests(first: $page_size, after: $cursor, states: OPEN, orderBy: {field: UPDATED_AT, direction: DESC}) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        author { login }
        title
        url
        updatedAt
        headRefOid
        reviewDecision
        repository { name owner { login } }
        reviewRequests(first: 50) {
          nodes {
            requestedReviewer {
              __typename
              ... on User { login }
              ... on Team { slug }
            }
          }
        }
        commits(last: 1) {
          nodes {
            commit {
              statusCheckRollup {
                state
                contexts(first: 50) {
                  nodes {
                    __typename
                    ... on CheckRun {
                      name
                      conclusion
                      detailsUrl
                      startedAt
                    }
                    ... on StatusContext {
                      context
                      state
                      targetUrl
                    }
                  }
                }
              }
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
        author { login }
        title
        url
        updatedAt
        headRefOid
        reviewDecision
        repository { name owner { login } }
        reviewRequests(first: 50) {
          nodes {
            requestedReviewer {
              __typename
              ... on User { login }
              ... on Team { slug }
            }
          }
        }
        commits(last: 1) {
          nodes {
            commit {
              statusCheckRollup {
                state
                contexts(first: 50) {
                  nodes {
                    __typename
                    ... on CheckRun {
                      name
                      conclusion
                      detailsUrl
                      startedAt
                    }
                    ... on StatusContext {
                      context
                      state
                      targetUrl
                    }
                  }
                }
              }
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

fn map_ci_checks(node: &PullRequestNode) -> Vec<CiCheck> {
    let Some(commits) = &node.commits else { return Vec::new() };
    let Some(nodes) = &commits.nodes else { return Vec::new() };
    let Some(first) = nodes.first() else { return Vec::new() };
    let Some(commit) = &first.commit else { return Vec::new() };
    let Some(rollup) = &commit.status_check_rollup else { return Vec::new() };
    let Some(ctxs) = &rollup.contexts else { return Vec::new() };
    let Some(nodes) = &ctxs.nodes else { return Vec::new() };

    let mut out = Vec::new();
    for n in nodes {
        match n.typename.as_deref() {
            Some("CheckRun") => {
                let name = n.name.clone().unwrap_or_else(|| "check".to_string());
                let state = match n.conclusion.as_deref() {
                    Some("SUCCESS") => CiCheckState::Success,
                    Some("FAILURE") | Some("ERROR") | Some("TIMED_OUT") | Some("STARTUP_FAILURE") => CiCheckState::Failure,
                    Some("NEUTRAL") | Some("SKIPPED") | Some("STALE") | Some("CANCELLED") | Some("ACTION_REQUIRED") => CiCheckState::Neutral,
                    None => CiCheckState::Running,
                    _ => CiCheckState::None,
                };
                let started_at_unix = n
                    .started_at
                    .as_deref()
                    .and_then(parse_github_datetime_to_unix);
                out.push(CiCheck {
                    name,
                    state,
                    url: n.details_url.clone(),
                    started_at_unix,
                });
            }
            Some("StatusContext") => {
                let name = n.context.clone().unwrap_or_else(|| "status".to_string());
                let state = match n.state.as_deref() {
                    Some("SUCCESS") => CiCheckState::Success,
                    Some("FAILURE") | Some("ERROR") => CiCheckState::Failure,
                    Some("PENDING") | Some("EXPECTED") => CiCheckState::Running,
                    _ => CiCheckState::None,
                };
                out.push(CiCheck {
                    name,
                    state,
                    url: n.target_url.clone(),
                    started_at_unix: None,
                });
            }
            _ => {}
        }
    }

    // Stable ordering: failed first, then running, then success, then name.
    out.sort_by(|a, b| {
        let rank = |s: &CiCheckState| match s {
            CiCheckState::Failure => 0,
            CiCheckState::Running => 1,
            CiCheckState::Success => 2,
            CiCheckState::Neutral => 3,
            CiCheckState::None => 4,
        };
        rank(&a.state)
            .cmp(&rank(&b.state))
            .then_with(|| a.name.cmp(&b.name))
    });

    out
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

fn is_review_requested_by_user(node: &PullRequestNode, viewer_login: &str) -> bool {
    let Some(rr) = &node.review_requests else { return false };
    let Some(nodes) = &rr.nodes else { return false };
    for n in nodes {
        let Some(r) = &n.requested_reviewer else { continue };
        if r.typename.as_deref() == Some("User") && r.login.as_deref() == Some(viewer_login) {
            return true;
        }
    }
    false
}

fn to_pr(node: PullRequestNode, is_requested: bool) -> Option<Pr> {
    let ci_state = map_ci_state(&node);
    let ci_checks = map_ci_checks(&node);
    let last_commit_sha = node.head_ref_oid.clone();
    let review_state = map_review_state(&node, is_requested);
    let owner = node.repository.owner.login;
    let repo = node.repository.name;
    let author = node
        .author
        .as_ref()
        .map(|a| a.login.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let updated_at_unix = parse_github_datetime_to_unix(&node.updated_at)?;
    let pr_key = format!("{owner}/{repo}#{}", node.number);

    Some(Pr {
        pr_key,
        owner,
        repo,
        number: node.number,
        author,
        title: node.title,
        url: node.url,
        updated_at_unix,
        last_commit_sha,
        ci_state,
        ci_checks,
        review_state,
    })
}

pub async fn fetch_attention_prs(octo: &Octocrab, cutoff_ts: i64) -> Result<Vec<Pr>, String> {
    // Fetch authored PRs
    let mut authored: Vec<PullRequestNode> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut viewer_login: Option<String> = None;
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

        if viewer_login.is_none() {
            viewer_login = Some(resp.data.viewer.login.clone());
        }

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

    let viewer_login = viewer_login.unwrap_or_else(|| "unknown".to_string());

    // Fetch review-requested PRs
    let cutoff_date = unix_to_ymd(cutoff_ts)
        .map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
        .unwrap_or_else(|| "1970-01-01".to_string());
    let search_query = format!(
        "is:pr is:open review-requested:@me sort:updated-desc updated:>={}",
        cutoff_date
    );

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
                    // Only keep PRs where the viewer is explicitly requested as a User reviewer
                    // (ignore team review requests).
                    if is_review_requested_by_user(&pr, &viewer_login) {
                        requested_nodes.push(pr);
                    }
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
        let requested_user = is_review_requested_by_user(&node, &viewer_login);
        if let Some(pr) = to_pr(node, requested_user) {
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


