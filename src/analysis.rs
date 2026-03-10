use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use inspect_core::analyze::{analyze, analyze_remote};
use inspect_core::types::ReviewResult;
use sem_core::git::types::DiffScope;
use tokio::sync::Mutex;

/// Manages analysis caching and deduplication
pub struct AnalysisManager {
    pub cache: Arc<Mutex<HashMap<(String, u64), ReviewResult>>>,
    pub in_progress: Arc<Mutex<std::collections::HashSet<(String, u64)>>>,
}

impl AnalysisManager {
    pub fn new() -> Self {
        let cache = load_disk_cache();
        Self {
            cache: Arc::new(Mutex::new(cache)),
            in_progress: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }
}

/// Directory for persistent cache files
fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("prfait")
}

/// Load all cached analysis results from disk
fn load_disk_cache() -> HashMap<(String, u64), ReviewResult> {
    let dir = cache_dir();
    let mut cache = HashMap::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return cache;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        // Format: "owner__repo__42.json" (repo slashes replaced with __)
        if let Some((repo, pr_num)) = parse_cache_filename(stem) {
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(result) = serde_json::from_str::<ReviewResult>(&data) {
                    cache.insert((repo, pr_num), result);
                }
            }
        }
    }
    cache
}

/// Save a single analysis result to disk
pub fn save_to_disk_cache(repo: &str, pr_number: u64, result: &ReviewResult) {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let filename = format!("{}__{}.json", repo.replace('/', "__"), pr_number);
    let path = dir.join(filename);
    if let Ok(json) = serde_json::to_string(result) {
        let _ = std::fs::write(path, json);
    }
}

/// Parse "owner__repo__42" into ("owner/repo", 42)
fn parse_cache_filename(stem: &str) -> Option<(String, u64)> {
    let last_sep = stem.rfind("__")?;
    let repo_part = &stem[..last_sep];
    let num_part = &stem[last_sep + 2..];
    let pr_num: u64 = num_part.parse().ok()?;
    let repo = repo_part.replace("__", "/");
    Some((repo, pr_num))
}

/// Analyze a local repo (git fetch + merge-base + inspect-core analyze with full graph)
pub async fn analyze_local_standalone(
    local_path: &Path,
    base_ref: &str,
    head_ref: &str,
) -> color_eyre::Result<ReviewResult> {
    let local_path = local_path.to_path_buf();
    let base_ref = base_ref.to_string();
    let head_ref = head_ref.to_string();

    // Fetch all remotes, then explicitly fetch the PR's branch refs
    // (--all may miss branches not matching the local refspec)
    let _ = tokio::process::Command::new("git")
        .args(["fetch", "--all"])
        .current_dir(&local_path)
        .output()
        .await;

    // Explicitly fetch the specific branches we need (handles repos
    // cloned with --single-branch or limited refspecs)
    for branch in [&base_ref, &head_ref] {
        let _ = tokio::process::Command::new("git")
            .args(["fetch", "origin", &format!("{branch}:{branch}")])
            .current_dir(&local_path)
            .output()
            .await;
    }

    // Compute merge-base so we only see changes introduced by the PR branch,
    // not unrelated changes from the base branch moving forward.
    let merge_base = tokio::process::Command::new("git")
        .args([
            "merge-base",
            &format!("origin/{base_ref}"),
            &format!("origin/{head_ref}"),
        ])
        .current_dir(&local_path)
        .output()
        .await?;

    if !merge_base.status.success() {
        let stderr = String::from_utf8_lossy(&merge_base.stderr);
        color_eyre::eyre::bail!("git merge-base failed: {stderr}");
    }

    let merge_base_sha = String::from_utf8_lossy(&merge_base.stdout)
        .trim()
        .to_string();

    // Run analysis in blocking thread (inspect-core is sync)
    // Diff from merge-base to head — this is what GitHub shows as the PR diff
    let result = tokio::task::spawn_blocking(move || {
        let scope = DiffScope::Range {
            from: merge_base_sha,
            to: format!("origin/{head_ref}"),
        };
        analyze(&local_path, scope).map_err(|e| color_eyre::eyre::eyre!("{e}"))
    })
    .await??;

    Ok(result)
}

/// Analyze via GitHub API (no local clone needed, but no blast radius)
pub async fn analyze_remote_standalone(
    repo: &str,
    _pr_number: u64,
    base_ref: &str,
    head_sha: &str,
    files: &[crate::github::PrFileData],
) -> color_eyre::Result<ReviewResult> {
    let inspect_files: Vec<inspect_core::github::PrFile> = files
        .iter()
        .map(|f| inspect_core::github::PrFile {
            filename: f.path.clone(),
            status: map_change_type(&f.change_type),
            additions: f.additions,
            deletions: f.deletions,
            patch: None,
        })
        .collect();

    let inspect_client =
        inspect_core::github::GitHubClient::new().map_err(|e| color_eyre::eyre::eyre!("{e}"))?;

    let file_pairs = inspect_client
        .get_file_pairs(repo, &inspect_files, base_ref, head_sha)
        .await;

    let result = tokio::task::spawn_blocking(move || {
        analyze_remote(&file_pairs).map_err(|e| color_eyre::eyre::eyre!("{e}"))
    })
    .await??;

    Ok(result)
}

fn map_change_type(ct: &str) -> String {
    match ct.to_uppercase().as_str() {
        "ADDED" => "added",
        "DELETED" | "REMOVED" => "removed",
        "RENAMED" => "renamed",
        _ => "modified",
    }
    .to_string()
}
