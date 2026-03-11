use inspect_core::risk::score_to_level;
use inspect_core::types::ReviewResult;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreDampening {
    pub pattern: String,
    pub multiplier: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub github_token: Option<String>,
    pub editor: Option<String>,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub score_dampening: Vec<ScoreDampening>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckConfig {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub local_path: Option<PathBuf>,
    #[serde(default)]
    pub checks: Vec<CheckConfig>,
    #[serde(default = "default_true")]
    pub prefer_ci: bool,
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn load() -> color_eyre::Result<Self> {
        let path = config_path();
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&text)?;
            Ok(config)
        } else {
            Ok(Config {
                github_token: None,
                editor: None,
                repos: vec![],
                score_dampening: vec![],
            })
        }
    }

    pub fn save(&self) -> color_eyre::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    pub fn resolve_editor(&self) -> String {
        self.editor
            .clone()
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| "vi".to_string())
    }

    pub fn effective_dampening(&self) -> Vec<ScoreDampening> {
        if !self.score_dampening.is_empty() {
            return self.score_dampening.clone();
        }
        default_dampening()
    }
}

fn default_dampening() -> Vec<ScoreDampening> {
    [
        ("*.lock", 0.1),
        (".editorconfig", 0.1),
        (".gitignore", 0.1),
        (".gitattributes", 0.1),
        (".prettierrc", 0.1),
        (".prettierignore", 0.1),
        (".eslintignore", 0.1),
        ("CLAUDE.md", 0.15),
        ("README.md", 0.2),
        ("CHANGELOG.md", 0.2),
        ("LICENSE", 0.1),
        (".github/*", 0.25),
    ]
    .into_iter()
    .map(|(pattern, multiplier)| ScoreDampening {
        pattern: pattern.to_string(),
        multiplier,
    })
    .collect()
}

fn file_matches_pattern(file_path: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        // "*.lock" — match suffix on the filename component
        let filename = file_path.rsplit('/').next().unwrap_or(file_path);
        filename.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix("/*") {
        // ".github/*" — match if any path component equals the prefix, or path starts with it
        file_path.starts_with(&format!("{prefix}/"))
            || file_path.contains(&format!("/{prefix}/"))
    } else {
        // Exact match on filename component
        let filename = file_path.rsplit('/').next().unwrap_or(file_path);
        filename == pattern
    }
}

pub fn apply_score_dampening(result: &mut ReviewResult, rules: &[ScoreDampening]) {
    for entity in &mut result.entity_reviews {
        if let Some(rule) = rules
            .iter()
            .find(|r| file_matches_pattern(&entity.file_path, &r.pattern))
        {
            entity.risk_score *= rule.multiplier;
            entity.risk_level = score_to_level(entity.risk_score);
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prfait")
        .join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_pattern() {
        assert!(file_matches_pattern("Cargo.lock", "*.lock"));
        assert!(file_matches_pattern("src/bun.lock", "*.lock"));
        assert!(!file_matches_pattern("src/lock.rs", "*.lock"));
    }

    #[test]
    fn prefix_dir_pattern() {
        assert!(file_matches_pattern(".github/workflows/ci.yml", ".github/*"));
        assert!(file_matches_pattern(".github/dependabot.yml", ".github/*"));
        assert!(!file_matches_pattern("src/.github", ".github/*"));
    }

    #[test]
    fn exact_filename_pattern() {
        assert!(file_matches_pattern("CLAUDE.md", "CLAUDE.md"));
        assert!(file_matches_pattern("subdir/CLAUDE.md", "CLAUDE.md"));
        assert!(!file_matches_pattern("NOT_CLAUDE.md", "CLAUDE.md"));
    }

    #[test]
    fn effective_dampening_uses_defaults() {
        let config = Config {
            github_token: None,
            editor: None,
            repos: vec![],
            score_dampening: vec![],
        };
        let rules = config.effective_dampening();
        assert!(!rules.is_empty());
        assert!(rules.iter().any(|r| r.pattern == "*.lock"));
    }

    #[test]
    fn effective_dampening_uses_user_config() {
        let config = Config {
            github_token: None,
            editor: None,
            repos: vec![],
            score_dampening: vec![ScoreDampening {
                pattern: "*.toml".to_string(),
                multiplier: 0.5,
            }],
        };
        let rules = config.effective_dampening();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "*.toml");
    }
}
