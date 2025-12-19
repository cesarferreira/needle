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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CiCheckState {
    Success,
    Failure,
    Running,
    Neutral,
    None,
}

impl CiCheckState {
    pub fn is_failure(&self) -> bool {
        matches!(self, CiCheckState::Failure)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CiCheck {
    pub name: String,
    pub state: CiCheckState,
    pub url: Option<String>,
    pub started_at_unix: Option<i64>,
}

/// Detailed information about why a PR cannot be merged.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MergeBlockers {
    /// PR has merge conflicts with the base branch.
    pub has_conflicts: bool,
    /// Number of approving reviews required by branch protection.
    pub required_approvals: Option<u32>,
    /// Current number of approving reviews.
    pub current_approvals: u32,
    /// Required status check contexts from branch protection.
    pub required_checks: Vec<String>,
    /// Subset of required checks that are failing or missing.
    pub failing_required_checks: Vec<String>,
    /// Base branch is ahead of the PR branch.
    pub is_behind_base: bool,
}

impl MergeBlockers {
    /// Returns true if there are no merge blockers.
    pub fn is_clear(&self) -> bool {
        !self.has_conflicts
            && !self.is_behind_base
            && self.failing_required_checks.is_empty()
            && self
                .required_approvals
                .map(|r| self.current_approvals >= r)
                .unwrap_or(true)
    }

    /// Returns a list of human-readable blocker descriptions.
    pub fn to_descriptions(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.has_conflicts {
            out.push("Merge conflicts".to_string());
        }
        if self.is_behind_base {
            out.push("Branch behind base".to_string());
        }
        if let Some(required) = self.required_approvals {
            if self.current_approvals < required {
                out.push(format!(
                    "Approvals: {}/{} required",
                    self.current_approvals, required
                ));
            }
        }
        if !self.failing_required_checks.is_empty() {
            let checks = self.failing_required_checks.join(", ");
            out.push(format!("Required checks failing: {}", checks));
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct Pr {
    pub pr_key: String, // "{owner}/{repo}#{number}"
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub author: String,
    pub title: String,
    pub url: String,

    pub updated_at_unix: i64,
    pub last_commit_sha: Option<String>,
    pub ci_state: CiState,
    pub ci_checks: Vec<CiCheck>,
    pub review_state: ReviewState,

    // Extra metadata for triage.
    pub is_draft: bool,
    pub mergeable: Option<String>, // e.g. "MERGEABLE" | "CONFLICTING" | "UNKNOWN"
    pub merge_state_status: Option<String>, // e.g. "CLEAN" | "BLOCKED" | ...
    pub is_viewer_author: bool,    // true when this PR is authored by the signed-in user
    pub merge_blockers: Option<MergeBlockers>,
}
