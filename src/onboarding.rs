use inquire::{MultiSelect, Select, Text};

use crate::config::{Config, RepoConfig};

pub fn run_onboarding() -> color_eyre::Result<Config> {
    eprintln!();
    eprintln!("Welcome to PRfait! Let's set up your configuration.");
    eprintln!();

    // 1. Token method
    let token_options = vec!["Use gh CLI (recommended)", "Paste a personal access token"];
    let token_choice = Select::new(
        "How would you like to authenticate with GitHub?",
        token_options,
    )
    .prompt()?;

    let github_token = if token_choice.starts_with("Paste") {
        let pat = Text::new("GitHub personal access token:").prompt()?;
        Some(pat)
    } else {
        None
    };

    // 2. Validate the token works
    let token_for_validation = if let Some(ref pat) = github_token {
        pat.clone()
    } else {
        let output = std::process::Command::new("gh")
            .args(["auth", "token"])
            .output();
        match output {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => {
                eprintln!("Could not get token from gh CLI. Make sure `gh auth login` has been run.");
                std::process::exit(1);
            }
        }
    };

    eprint!("Validating token... ");
    let mut username = String::new();
    let validation = std::process::Command::new("gh")
        .args(["api", "/user"])
        .env("GH_TOKEN", &token_for_validation)
        .output();
    match validation {
        Ok(o) if o.status.success() => {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                if let Some(login) = val.get("login").and_then(|v| v.as_str()) {
                    eprintln!("authenticated as {login}");
                    username = login.to_string();
                } else {
                    eprintln!("ok");
                }
            } else {
                eprintln!("ok");
            }
        }
        _ => {
            eprintln!("failed!");
            eprintln!("Could not validate the GitHub token. Please check your token and try again.");
            std::process::exit(1);
        }
    }

    // 3. Build owner list (user + orgs) to scope searches
    eprint!("Fetching organizations...");
    let mut owners: Vec<String> = Vec::new();
    if !username.is_empty() {
        owners.push(username);
    }
    if let Some(orgs) = fetch_orgs(&token_for_validation) {
        eprintln!(" {}", orgs.join(", "));
        owners.extend(orgs);
    } else {
        eprintln!(" none found");
    }

    // 4. Search-select loop for repos
    eprintln!();
    eprintln!("Search for repos to watch. Results are scoped to your account and organizations.");
    eprintln!("Search as many times as you like, then press enter on an empty query to finish.");
    eprintln!();

    let mut selected_repos: Vec<String> = Vec::new();

    loop {
        if !selected_repos.is_empty() {
            eprintln!("Selected so far: {}", selected_repos.join(", "));
        }

        let query = Text::new("Search repos (empty to finish):")
            .prompt_skippable()?
            .unwrap_or_default();
        if query.is_empty() {
            break;
        }

        eprint!("Searching...");
        let results = search_repos(&token_for_validation, &query, &owners);
        eprintln!(" {} results", results.len());

        if results.is_empty() {
            eprintln!("No repos found. Try a different search term.");
            continue;
        }

        // Filter out already-selected repos
        let available: Vec<String> = results
            .into_iter()
            .filter(|r| !selected_repos.contains(r))
            .collect();

        if available.is_empty() {
            eprintln!("All results already selected.");
            continue;
        }

        let picks = MultiSelect::new("Select repos (type to filter, space to toggle):", available)
            .prompt()?;

        selected_repos.extend(picks);
    }

    // Offer manual entry for repos not found by search
    loop {
        let repo = Text::new("Add repo manually (owner/name, empty to finish):")
            .prompt_skippable()?
            .unwrap_or_default();
        if repo.is_empty() {
            break;
        }
        if !selected_repos.contains(&repo) {
            selected_repos.push(repo);
        }
    }

    if selected_repos.is_empty() {
        eprintln!("No repos selected. You can edit the config file later.");
    }

    // 4. Resolve local clone paths
    //    Ask for a project folder, then auto-match repos by directory name.
    //    Only prompt individually for repos that don't match.
    let mut repos = Vec::new();

    if !selected_repos.is_empty() {
        let project_dir = Text::new("Project folder (e.g. ~/proj, empty to skip):")
            .prompt_skippable()?
            .unwrap_or_default();

        let project_dir = expand_tilde(&project_dir);

        // Try to auto-match each repo
        let mut unmatched = Vec::new();
        for name in &selected_repos {
            // repo_name is the part after the slash, e.g. "prfait" from "SundaeSwap-finance/prfait"
            let repo_name = name.rsplit('/').next().unwrap_or(name);
            let matched = if !project_dir.is_empty() {
                let base = std::path::Path::new(&project_dir);
                // Try: <dir>/<repo_name>, <dir>/<owner>/<repo_name>, <dir>/<repo_name>/.git
                let candidates = [
                    base.join(repo_name),
                    base.join(name),
                ];
                candidates.into_iter().find(|p| p.join(".git").is_dir())
            } else {
                None
            };

            if let Some(path) = matched {
                eprintln!("  {} -> {}", name, path.display());
                repos.push(RepoConfig {
                    name: name.clone(),
                    local_path: Some(path),
                    checks: vec![],
                    prefer_ci: true,
                });
            } else {
                unmatched.push(name.clone());
            }
        }

        // Ask individually for unmatched repos
        if !unmatched.is_empty() {
            eprintln!();
            eprintln!(
                "{} repo(s) not found in project folder. Enter paths manually (or skip).",
                unmatched.len()
            );
            for name in &unmatched {
                let local_path = Text::new(&format!("Local clone path for {name} (empty to skip):"))
                    .prompt_skippable()?
                    .unwrap_or_default();

                let local_path = expand_tilde(&local_path);

                repos.push(RepoConfig {
                    name: name.clone(),
                    local_path: if local_path.is_empty() {
                        None
                    } else {
                        Some(local_path.into())
                    },
                    checks: vec![],
                    prefer_ci: true,
                });
            }
        }
    }

    let config = Config {
        github_token,
        editor: None,
        repos,
        score_dampening: vec![],
    };

    // 5. Write config
    config.save()?;
    let path = crate::config::config_path();
    eprintln!();
    eprintln!("Config saved to {}", path.display());
    eprintln!();

    Ok(config)
}

/// Expand a leading `~` or `~/` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &path[1..]);
        }
    }
    path.to_string()
}

fn fetch_orgs(token: &str) -> Option<Vec<String>> {
    let output = std::process::Command::new("gh")
        .args(["api", "/user/orgs", "--jq", ".[].login"])
        .env("GH_TOKEN", token)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let orgs: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(String::from).collect();
    if orgs.is_empty() { None } else { Some(orgs) }
}

fn search_repos(token: &str, query: &str, owners: &[String]) -> Vec<String> {
    let mut args = vec![
        "search".to_string(),
        "repos".to_string(),
        query.to_string(),
        "--json".to_string(),
        "fullName".to_string(),
        "-L".to_string(),
        "50".to_string(),
    ];
    for owner in owners {
        args.push("--owner".to_string());
        args.push(owner.clone());
    }

    let output = std::process::Command::new("gh")
        .args(&args)
        .env("GH_TOKEN", token)
        .output();

    let Some(output) = output.ok() else { return vec![] };
    if !output.status.success() {
        return vec![];
    }
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return vec![];
    };
    val.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("fullName")?.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}
