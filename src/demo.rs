use crate::model::{CiCheck, CiCheckState, CiState, Pr, ReviewState};
use std::sync::atomic::{AtomicU64, Ordering};

static DEMO_TICK: AtomicU64 = AtomicU64::new(0);

pub fn next_demo_tick() -> u64 {
    DEMO_TICK.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

#[derive(Clone, Copy)]
enum CiProfile {
    Green,
    RedNew,
    RedStuck,
    RunningLong,
    RunningShort,
    NoCi,
}

#[derive(Clone)]
struct DemoPrSpec {
    owner: &'static str,
    repo: &'static str,
    number: i64,
    author: &'static str,
    title: &'static str,
    updated_age_secs: i64,
    review: ReviewState,
    ci: CiProfile,
}

fn fnv1a_64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn seeded_last_opened_at(pr_key: &str, now: i64) -> Option<i64> {
    let h = fnv1a_64(pr_key);
    match h % 11 {
        0 => Some(now.saturating_sub(23 * 60)),    // opened recently (dim)
        1 => Some(now.saturating_sub(3 * 3600)),   // opened earlier today
        2 => Some(now.saturating_sub(2 * 86400)),  // opened a couple days ago
        _ => None,
    }
}

fn short_sha(mut x: u64) -> String {
    // Deterministic 7-hex-ish "sha".
    let mut out = String::with_capacity(7);
    for _ in 0..7 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let v = (x & 0x0f) as u8;
        out.push(char::from(b"0123456789abcdef"[v as usize]));
    }
    out
}

fn actions_url(owner: &str, repo: &str, run_id: u64) -> String {
    format!("https://github.com/{owner}/{repo}/actions/runs/{run_id}")
}

fn pr_url(owner: &str, repo: &str, number: i64) -> String {
    format!("https://github.com/{owner}/{repo}/pull/{number}")
}

fn checks_for(profile: CiProfile, owner: &str, repo: &str, _number: i64, now: i64, salt: u64) -> (CiState, Vec<CiCheck>) {
    let base_run = 8_100_000u64 + (salt % 900_000);
    let url = |off: u64| Some(actions_url(owner, repo, base_run + off));

    let mk = |name: &str, state: CiCheckState, started_at_unix: Option<i64>, off: u64| CiCheck {
        name: name.to_string(),
        state,
        url: url(off),
        started_at_unix,
    };

    match profile {
        CiProfile::Green => (
            CiState::Success,
            vec![
                mk("build / linux", CiCheckState::Success, None, 11),
                mk("test / unit", CiCheckState::Success, None, 22),
                mk("lint", CiCheckState::Success, None, 33),
                mk("e2e / chrome", CiCheckState::Neutral, None, 44),
            ],
        ),
        CiProfile::RedNew | CiProfile::RedStuck => (
            CiState::Failure,
            vec![
                mk("build / linux", CiCheckState::Success, None, 11),
                mk("test / unit", CiCheckState::Failure, None, 22),
                mk("lint", CiCheckState::Success, None, 33),
                mk("e2e / chrome", CiCheckState::Failure, None, 44),
            ],
        ),
        CiProfile::RunningLong => (
            CiState::Running,
            vec![
                mk("build / linux", CiCheckState::Success, None, 11),
                mk(
                    "test / integration",
                    CiCheckState::Running,
                    Some(now.saturating_sub(68 * 60)),
                    22,
                ),
                mk("lint", CiCheckState::Success, None, 33),
                mk("deploy / preview", CiCheckState::Running, Some(now.saturating_sub(41 * 60)), 44),
            ],
        ),
        CiProfile::RunningShort => (
            CiState::Running,
            vec![
                mk("build / linux", CiCheckState::Running, Some(now.saturating_sub(4 * 60)), 11),
                mk("test / unit", CiCheckState::None, None, 22),
                mk("lint", CiCheckState::None, None, 33),
            ],
        ),
        CiProfile::NoCi => (CiState::None, Vec::new()),
    }
}

pub fn generate_demo_prs(now: i64, tick: u64) -> Vec<Pr> {
    let specs: &[DemoPrSpec] = &[
        DemoPrSpec {
            owner: "acme-inc",
            repo: "billing-api",
            number: 842,
            author: "anika",
            title: "Fix idempotency for retries on charge capture",
            updated_age_secs: 2 * 3600,
            review: ReviewState::Requested,
            ci: CiProfile::Green,
        },
        DemoPrSpec {
            owner: "orbit",
            repo: "web",
            number: 1932,
            author: "santiago",
            title: "Add keyboard navigation to project switcher",
            updated_age_secs: 28 * 60,
            review: ReviewState::None,
            ci: CiProfile::RedNew,
        },
        DemoPrSpec {
            owner: "windmill-labs",
            repo: "infra",
            number: 317,
            author: "chen",
            title: "Bump Postgres to 16.2 and tune autovacuum thresholds",
            updated_age_secs: 4 * 86400,
            review: ReviewState::None,
            ci: CiProfile::RedStuck,
        },
        DemoPrSpec {
            owner: "paperplane",
            repo: "mobile",
            number: 501,
            author: "sofia",
            title: "Reduce cold-start time by deferring analytics init",
            updated_age_secs: 19 * 60,
            review: ReviewState::None,
            ci: CiProfile::RunningLong,
        },
        DemoPrSpec {
            owner: "acme-inc",
            repo: "design-system",
            number: 128,
            author: "mia",
            title: "Button: add loading state and improve focus ring",
            updated_age_secs: 6 * 60,
            review: ReviewState::None,
            ci: CiProfile::RunningShort,
        },
        DemoPrSpec {
            owner: "honeycombio",
            repo: "otel-collector",
            number: 77,
            author: "devin",
            title: "Add tail-sampling defaults for high-cardinality traces",
            updated_age_secs: 7 * 86400,
            review: ReviewState::None,
            ci: CiProfile::Green,
        },
        DemoPrSpec {
            owner: "orbit",
            repo: "api",
            number: 1104,
            author: "jules",
            title: "Rate limit /v1/events and emit structured logs",
            updated_age_secs: 16 * 3600,
            review: ReviewState::Approved,
            ci: CiProfile::Green,
        },
        DemoPrSpec {
            owner: "paperplane",
            repo: "docs",
            number: 42,
            author: "noah",
            title: "Docs: clarify OAuth scopes and add troubleshooting",
            updated_age_secs: 3 * 86400,
            review: ReviewState::None,
            ci: CiProfile::NoCi,
        },
        DemoPrSpec {
            owner: "acme-inc",
            repo: "monorepo",
            number: 2551,
            author: "anika",
            title: "Refactor: extract feature flags into shared crate",
            updated_age_secs: 11 * 3600,
            review: ReviewState::Requested,
            ci: CiProfile::RunningLong,
        },
        DemoPrSpec {
            owner: "windmill-labs",
            repo: "sdk-rust",
            number: 98,
            author: "chen",
            title: "Add retry policy for 429/503 responses",
            updated_age_secs: 12 * 86400,
            review: ReviewState::None,
            ci: CiProfile::Green,
        },
        DemoPrSpec {
            owner: "orbit",
            repo: "web",
            number: 1940,
            author: "sofia",
            title: "Fix flaky onboarding test on CI runners",
            updated_age_secs: 50 * 60,
            review: ReviewState::None,
            ci: CiProfile::RedNew,
        },
        DemoPrSpec {
            owner: "paperplane",
            repo: "backend",
            number: 611,
            author: "devin",
            title: "Graceful shutdown: drain queue workers before exit",
            updated_age_secs: 26 * 3600,
            review: ReviewState::Approved,
            ci: CiProfile::NoCi,
        },
        DemoPrSpec {
            owner: "honeycombio",
            repo: "ui",
            number: 390,
            author: "mia",
            title: "Charts: fix tooltip positioning near viewport edges",
            updated_age_secs: 9 * 3600,
            review: ReviewState::None,
            ci: CiProfile::Green,
        },
        DemoPrSpec {
            owner: "acme-inc",
            repo: "payments-worker",
            number: 219,
            author: "santiago",
            title: "Handle duplicate webhook deliveries and add metrics",
            updated_age_secs: 3 * 3600,
            review: ReviewState::Requested,
            ci: CiProfile::RedNew,
        },
        DemoPrSpec {
            owner: "windmill-labs",
            repo: "infra",
            number: 321,
            author: "jules",
            title: "Terraform: split prod/staging state and add drift detection",
            updated_age_secs: 18 * 86400,
            review: ReviewState::None,
            ci: CiProfile::Green,
        },
        DemoPrSpec {
            owner: "paperplane",
            repo: "mobile",
            number: 523,
            author: "noah",
            title: "Fix crash when resuming from background on iOS 17.2",
            updated_age_secs: 90 * 60,
            review: ReviewState::None,
            ci: CiProfile::RunningLong,
        },
    ];

    specs
        .iter()
        .map(|s| {
            let key = format!("{}/{}#{}", s.owner, s.repo, s.number);
            let salt = fnv1a_64(&key) ^ tick.rotate_left(13);
            let url = pr_url(s.owner, s.repo, s.number);

            let (ci_state, ci_checks) = checks_for(s.ci, s.owner, s.repo, s.number, now, salt);

            // Keep some PRs "stuck" so the second startup pass has a stable failure.
            let stable = matches!(s.ci, CiProfile::RedStuck | CiProfile::NoCi | CiProfile::Green);
            let sha_salt = if stable {
                fnv1a_64(&key)
            } else {
                salt ^ tick.wrapping_mul(0x9e3779b97f4a7c15)
            };
            let sha = short_sha(sha_salt);

            // Make a few PRs feel alive across refreshes by shifting updatedAt slightly.
            let wobble = if matches!(s.ci, CiProfile::RunningLong | CiProfile::RunningShort | CiProfile::RedNew) {
                ((tick as i64) % 7) * 60
            } else {
                0
            };
            let updated_at_unix = now.saturating_sub(s.updated_age_secs.saturating_sub(wobble));

            Pr {
                pr_key: key,
                owner: s.owner.to_string(),
                repo: s.repo.to_string(),
                number: s.number,
                author: s.author.to_string(),
                title: s.title.to_string(),
                url,
                updated_at_unix,
                last_commit_sha: Some(sha),
                ci_state,
                ci_checks,
                review_state: s.review.clone(),
            }
        })
        .collect()
}
