#[derive(Debug, Clone)]
pub enum CiState {
    Success,
    Failure,
    Running,
    None,
}

impl CiState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CiState::Success => "success",
            CiState::Failure => "failure",
            CiState::Running => "running",
            CiState::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ReviewState {
    Requested,
    Approved,
    None,
}

impl ReviewState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewState::Requested => "requested",
            ReviewState::Approved => "approved",
            ReviewState::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pr {
    pub pr_key: String, // "{owner}/{repo}#{number}"
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,

    pub updated_at_unix: i64,
    pub last_commit_sha: Option<String>,
    pub ci_state: CiState,
    pub review_state: ReviewState,
}



