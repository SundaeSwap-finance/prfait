use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::action::Action;
use crate::config::RepoConfig;
use crate::github::GithubClient;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum CheckStatus {
    Pending,
    Running,
    Passed,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    /// URL to the check run (GitHub Actions only)
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CheckSource {
    GithubActions,
    Local,
}

#[derive(Debug, Clone)]
pub struct PrCheckState {
    pub sha: String,
    pub source: CheckSource,
    pub checks: Vec<CheckResult>,
}

// ── CheckManager ─────────────────────────────────────────────────────────

pub struct CheckManager {
    pub states: HashMap<(String, u64), PrCheckState>,
    in_flight: HashSet<(String, u64)>,
}

impl CheckManager {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            in_flight: HashSet::new(),
        }
    }

    /// Returns true if no in-flight task exists and the SHA differs or was never checked.
    pub fn needs_check(&self, repo: &str, pr: u64, sha: &str) -> bool {
        let key = (repo.to_string(), pr);
        if self.in_flight.contains(&key) {
            return false;
        }
        match self.states.get(&key) {
            Some(state) => state.sha != sha,
            None => true,
        }
    }

    pub fn mark_in_flight(&mut self, repo: &str, pr: u64) {
        self.in_flight.insert((repo.to_string(), pr));
    }

    pub fn clear_in_flight(&mut self, repo: &str, pr: u64) {
        self.in_flight.remove(&(repo.to_string(), pr));
    }
}

// ── Auto-detection ───────────────────────────────────────────────────────

/// Auto-detect check commands based on project files in the given path.
pub fn detect_checks(worktree_path: &Path) -> Vec<(String, String)> {
    let mut commands = Vec::new();

    if worktree_path.join("Cargo.toml").exists() {
        commands.push(("Tests".to_string(), "cargo test".to_string()));
        commands.push((
            "Clippy".to_string(),
            "cargo clippy -- -D warnings".to_string(),
        ));
        commands.push(("Fmt".to_string(), "cargo fmt -- --check".to_string()));
    } else if worktree_path.join("package.json").exists() {
        if let Ok(content) = std::fs::read_to_string(worktree_path.join("package.json")) {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(scripts) = pkg.get("scripts").and_then(|s| s.as_object()) {
                    if scripts.contains_key("test") {
                        commands.push(("Tests".to_string(), "npm test".to_string()));
                    }
                    if scripts.contains_key("lint") {
                        commands.push(("Lint".to_string(), "npm run lint".to_string()));
                    }
                }
            }
        }
    } else if worktree_path.join("pyproject.toml").exists() {
        commands.push(("Tests".to_string(), "pytest".to_string()));
        commands.push(("Lint".to_string(), "ruff check .".to_string()));
    } else if worktree_path.join("go.mod").exists() {
        commands.push(("Tests".to_string(), "go test ./...".to_string()));
        commands.push(("Vet".to_string(), "go vet ./...".to_string()));
    }

    commands
}

// ── Local check runner ───────────────────────────────────────────────────

/// Run local checks in a git worktree. Sends progress updates via the action channel.
pub async fn run_local_checks(
    tx: mpsc::UnboundedSender<Action>,
    repo: String,
    pr: u64,
    sha: String,
    head_ref: String,
    local_path: std::path::PathBuf,
    explicit_commands: Vec<(String, String)>,
) {
    let short_sha = &sha[..sha.len().min(8)];
    let wt_path = format!("/tmp/prfait-wt-{}-{}-{}", repo.replace('/', "-"), pr, short_sha);

    // Fetch the ref
    let fetch = tokio::process::Command::new("git")
        .args(["fetch", "origin", &head_ref])
        .current_dir(&local_path)
        .output()
        .await;

    if fetch.is_err() || !fetch.as_ref().unwrap().status.success() {
        let _ = tx.send(Action::ChecksComplete(
            repo.clone(),
            pr,
            PrCheckState {
                sha: sha.clone(),
                source: CheckSource::Local,
                checks: vec![CheckResult {
                    name: "Fetch".to_string(),
                    status: CheckStatus::Failed("git fetch failed".to_string()),
                    url: None,
                }],
            },
        ));
        return;
    }

    // Create worktree
    let wt_add = tokio::process::Command::new("git")
        .args(["worktree", "add", "--detach", &wt_path, &sha])
        .current_dir(&local_path)
        .output()
        .await;

    if wt_add.is_err() || !wt_add.as_ref().unwrap().status.success() {
        let _ = tx.send(Action::ChecksComplete(
            repo.clone(),
            pr,
            PrCheckState {
                sha: sha.clone(),
                source: CheckSource::Local,
                checks: vec![CheckResult {
                    name: "Worktree".to_string(),
                    status: CheckStatus::Failed("git worktree add failed".to_string()),
                    url: None,
                }],
            },
        ));
        return;
    }

    let wt = Path::new(&wt_path);

    // Determine commands: explicit or auto-detected
    let commands = if explicit_commands.is_empty() {
        detect_checks(wt)
    } else {
        explicit_commands
    };

    if commands.is_empty() {
        // Nothing to run — clean up and report empty
        let _ = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force", &wt_path])
            .current_dir(&local_path)
            .output()
            .await;
        let _ = tx.send(Action::ChecksComplete(
            repo,
            pr,
            PrCheckState {
                sha,
                source: CheckSource::Local,
                checks: vec![],
            },
        ));
        return;
    }

    let mut results: Vec<CheckResult> = Vec::new();

    for (name, command) in &commands {
        // Send Running status
        let mut progress = results.clone();
        progress.push(CheckResult {
            name: name.clone(),
            status: CheckStatus::Running,
            url: None,
        });
        let _ = tx.send(Action::ChecksUpdate(repo.clone(), pr, progress));

        // Execute the command
        let output = tokio::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(&wt_path)
            .output()
            .await;

        let status = match output {
            Ok(o) if o.status.success() => CheckStatus::Passed,
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let msg = stderr.lines().last().unwrap_or("failed").to_string();
                CheckStatus::Failed(msg)
            }
            Err(e) => CheckStatus::Failed(e.to_string()),
        };

        results.push(CheckResult {
            name: name.clone(),
            status,
            url: None,
        });
    }

    // Clean up worktree
    let _ = tokio::process::Command::new("git")
        .args(["worktree", "remove", "--force", &wt_path])
        .current_dir(&local_path)
        .output()
        .await;

    let _ = tx.send(Action::ChecksComplete(
        repo,
        pr,
        PrCheckState {
            sha,
            source: CheckSource::Local,
            checks: results,
        },
    ));
}

// ── GitHub Actions poller ────────────────────────────────────────────────

/// Fetch and poll GitHub check runs until all complete.
pub async fn fetch_and_poll_github_checks(
    client: Arc<GithubClient>,
    tx: mpsc::UnboundedSender<Action>,
    repo: String,
    pr: u64,
    sha: String,
) {
    loop {
        let json = match client.fetch_check_runs(&repo, &sha).await {
            Ok(v) => v,
            Err(_) => {
                // API error — give up silently
                let _ = tx.send(Action::ChecksComplete(
                    repo,
                    pr,
                    PrCheckState {
                        sha,
                        source: CheckSource::GithubActions,
                        checks: vec![],
                    },
                ));
                return;
            }
        };

        let check_runs = json
            .get("check_runs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let results: Vec<CheckResult> = check_runs
            .iter()
            .map(|run| {
                let name = run
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let status_str = run
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let conclusion = run
                    .get("conclusion")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");

                let url = run
                    .get("html_url")
                    .and_then(|u| u.as_str())
                    .map(|s| s.to_string());

                let status = if status_str == "completed" {
                    match conclusion {
                        "success" | "skipped" | "neutral" => CheckStatus::Passed,
                        _ => CheckStatus::Failed(conclusion.to_string()),
                    }
                } else {
                    CheckStatus::Running
                };

                CheckResult { name, status, url }
            })
            .collect();

        let any_running = results.iter().any(|r| matches!(r.status, CheckStatus::Running));

        if any_running {
            let _ = tx.send(Action::ChecksUpdate(repo.clone(), pr, results));
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        } else {
            let _ = tx.send(Action::ChecksComplete(
                repo,
                pr,
                PrCheckState {
                    sha,
                    source: CheckSource::GithubActions,
                    checks: results,
                },
            ));
            return;
        }
    }
}

// ── Trigger orchestrator ─────────────────────────────────────────────────

/// Decide whether to use GitHub Actions or local checks, then run them.
/// Spawned as a tokio task from App.
pub async fn trigger_checks(
    tx: mpsc::UnboundedSender<Action>,
    client: Arc<GithubClient>,
    repo_config: RepoConfig,
    pr: u64,
    sha: String,
    head_ref: String,
) {
    let repo = repo_config.name.clone();

    // Send ChecksStarted
    let _ = tx.send(Action::ChecksStarted(repo.clone(), pr, sha.clone()));

    // Try GitHub Actions first
    let gh_checks = client.fetch_check_runs(&repo, &sha).await;
    let has_gh_actions = gh_checks
        .as_ref()
        .ok()
        .and_then(|v| v.get("total_count"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0)
        > 0;

    if has_gh_actions && repo_config.prefer_ci {
        // Use GitHub Actions — poll until complete
        fetch_and_poll_github_checks(client, tx, repo, pr, sha).await;
        return;
    }

    // Fall back to local checks if we have a local_path
    if let Some(ref local_path) = repo_config.local_path {
        let explicit_commands: Vec<(String, String)> = repo_config
            .checks
            .iter()
            .map(|c| (c.name.clone(), c.command.clone()))
            .collect();

        run_local_checks(
            tx,
            repo,
            pr,
            sha,
            head_ref,
            local_path.clone(),
            explicit_commands,
        )
        .await;
        return;
    }

    // No GH Actions and no local path — nothing to do
    if has_gh_actions {
        // prefer_ci is false but we have GH Actions — use them anyway
        fetch_and_poll_github_checks(client, tx, repo, pr, sha).await;
    } else {
        // No checks available
        let _ = tx.send(Action::ChecksComplete(
            repo,
            pr,
            PrCheckState {
                sha,
                source: CheckSource::Local,
                checks: vec![],
            },
        ));
    }
}
