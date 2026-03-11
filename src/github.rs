use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

use crate::review::{DiffSide, PrComment, PrComments, ReviewThread, ThreadComment};

#[derive(Debug, Clone)]
pub struct PrData {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub head_ref: String,
    pub base_ref: String,
    pub head_sha: String,
    pub updated_at: String,
    pub files: Vec<PrFileData>,
    /// PR description / body text (may be empty)
    pub body: String,
    /// URL to the PR on GitHub
    pub html_url: String,
}

#[derive(Debug, Clone)]
pub struct PrFileData {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub change_type: String,
}

pub struct GithubClient {
    http: reqwest::Client,
}

// GraphQL response types
#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Deserialize)]
struct RepoData {
    repository: RepoNode,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoNode {
    pull_requests: PrConnection,
}

#[derive(Deserialize)]
struct PrConnection {
    nodes: Vec<GqlPr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPr {
    number: u64,
    title: String,
    author: Option<GqlAuthor>,
    additions: u64,
    deletions: u64,
    changed_files: u64,
    head_ref_name: String,
    base_ref_name: String,
    head_ref_oid: String,
    updated_at: String,
    files: Option<GqlFileConnection>,
    body: Option<String>,
    url: String,
}

#[derive(Deserialize)]
struct GqlAuthor {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFileConnection {
    nodes: Vec<GqlFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFile {
    path: String,
    additions: u64,
    deletions: u64,
    change_type: String,
}

// GraphQL response types for review threads / PR comments
#[derive(Deserialize)]
struct CommentsRepoData {
    repository: CommentsRepoNode,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentsRepoNode {
    pull_request: Option<CommentsPrNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentsPrNode {
    review_threads: GqlThreadConnection,
    comments: GqlIssueCommentConnection,
}

#[derive(Deserialize)]
struct GqlThreadConnection {
    nodes: Vec<GqlReviewThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlReviewThread {
    is_resolved: bool,
    path: String,
    line: Option<u64>,
    start_line: Option<u64>,
    diff_side: Option<String>,
    comments: GqlThreadCommentConnection,
}

#[derive(Deserialize)]
struct GqlThreadCommentConnection {
    nodes: Vec<GqlThreadComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlThreadComment {
    author: Option<GqlAuthor>,
    body: String,
    created_at: String,
}

#[derive(Deserialize)]
struct GqlIssueCommentConnection {
    nodes: Vec<GqlIssueComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlIssueComment {
    author: Option<GqlAuthor>,
    body: String,
    created_at: String,
}

impl GithubClient {
    pub fn new(token: &str) -> color_eyre::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("prfait/0.1"));
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self { http })
    }

    /// Resolve a GitHub token from config, env, or `gh auth token`
    pub fn resolve_token(config_token: Option<&str>) -> color_eyre::Result<String> {
        if let Some(t) = config_token {
            return Ok(t.to_string());
        }
        if let Ok(t) = std::env::var("GITHUB_TOKEN") {
            return Ok(t);
        }
        let output = std::process::Command::new("gh")
            .args(["auth", "token"])
            .output();
        match output {
            Ok(o) if o.status.success() => {
                Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
            }
            _ => {
                color_eyre::eyre::bail!(
                    "No GitHub token found. Set github_token in config, GITHUB_TOKEN env, or install gh CLI."
                )
            }
        }
    }

    pub async fn list_open_prs(&self, repo: &str) -> color_eyre::Result<Vec<PrData>> {
        let (owner, name) = repo
            .split_once('/')
            .ok_or_else(|| color_eyre::eyre::eyre!("Repo must be owner/name, got: {repo}"))?;

        const QUERY: &str = r#"
query($owner: String!, $repo: String!) {
  repository(owner: $owner, name: $repo) {
    pullRequests(first: 50, states: [OPEN], orderBy: {field: UPDATED_AT, direction: DESC}) {
      nodes {
        number title body url
        author { login }
        additions deletions changedFiles
        headRefName baseRefName headRefOid updatedAt
        files(first: 100) {
          nodes { path additions deletions changeType }
        }
      }
    }
  }
}
"#;

        let body = serde_json::json!({
            "query": QUERY,
            "variables": { "owner": owner, "repo": name },
        });

        let resp = self
            .http
            .post("https://api.github.com/graphql")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            color_eyre::eyre::bail!("GitHub API {status}: {text}");
        }

        let gql: GqlResponse<RepoData> = resp.json().await?;
        if let Some(errors) = gql.errors {
            let msgs: Vec<_> = errors.iter().map(|e| e.message.as_str()).collect();
            color_eyre::eyre::bail!("GraphQL errors: {}", msgs.join("; "));
        }

        let data = gql
            .data
            .ok_or_else(|| color_eyre::eyre::eyre!("No data in GraphQL response"))?;

        let prs = data
            .repository
            .pull_requests
            .nodes
            .into_iter()
            .map(|pr| PrData {
                number: pr.number,
                title: pr.title,
                author: pr.author.map(|a| a.login).unwrap_or_default(),
                additions: pr.additions,
                deletions: pr.deletions,
                changed_files: pr.changed_files,
                head_ref: pr.head_ref_name,
                base_ref: pr.base_ref_name,
                head_sha: pr.head_ref_oid,
                updated_at: pr.updated_at,
                files: pr
                    .files
                    .map(|fc| {
                        fc.nodes
                            .into_iter()
                            .map(|f| PrFileData {
                                path: f.path,
                                additions: f.additions,
                                deletions: f.deletions,
                                change_type: f.change_type,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                body: pr.body.unwrap_or_default(),
                html_url: pr.url,
            })
            .collect();

        Ok(prs)
    }

    /// Fetch existing review threads and PR discussion comments.
    pub async fn get_pr_comments(&self, repo: &str, number: u64) -> color_eyre::Result<PrComments> {
        let (owner, name) = repo
            .split_once('/')
            .ok_or_else(|| color_eyre::eyre::eyre!("Repo must be owner/name, got: {repo}"))?;

        const QUERY: &str = r#"
query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          isResolved
          path
          line
          startLine
          diffSide
          comments(first: 50) {
            nodes {
              author { login }
              body
              createdAt
            }
          }
        }
      }
      comments(first: 100) {
        nodes {
          author { login }
          body
          createdAt
        }
      }
    }
  }
}
"#;

        let body = serde_json::json!({
            "query": QUERY,
            "variables": { "owner": owner, "repo": name, "number": number },
        });

        let resp = self
            .http
            .post("https://api.github.com/graphql")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            color_eyre::eyre::bail!("GitHub API {status}: {text}");
        }

        let gql: GqlResponse<CommentsRepoData> = resp.json().await?;
        if let Some(errors) = gql.errors {
            let msgs: Vec<_> = errors.iter().map(|e| e.message.as_str()).collect();
            color_eyre::eyre::bail!("GraphQL errors: {}", msgs.join("; "));
        }

        let data = gql
            .data
            .ok_or_else(|| color_eyre::eyre::eyre!("No data in GraphQL response"))?;

        let pr = data
            .repository
            .pull_request
            .ok_or_else(|| color_eyre::eyre::eyre!("PR not found"))?;

        let threads = pr
            .review_threads
            .nodes
            .into_iter()
            .filter_map(|t| {
                let line = t.line? as usize;
                Some(ReviewThread {
                    is_resolved: t.is_resolved,
                    path: t.path,
                    line,
                    start_line: t.start_line.map(|sl| sl as usize),
                    diff_side: match t.diff_side.as_deref() {
                        Some("LEFT") => DiffSide::Left,
                        _ => DiffSide::Right,
                    },
                    comments: t
                        .comments
                        .nodes
                        .into_iter()
                        .map(|c| ThreadComment {
                            author: c.author.map(|a| a.login).unwrap_or_default(),
                            body: c.body,
                            created_at: c.created_at,
                        })
                        .collect(),
                })
            })
            .collect();

        let comments = pr
            .comments
            .nodes
            .into_iter()
            .map(|c| PrComment {
                author: c.author.map(|a| a.login).unwrap_or_default(),
                body: c.body,
                created_at: c.created_at,
            })
            .collect();

        Ok(PrComments { threads, comments })
    }

    /// Fetch raw diff for a PR (for semantic analysis of remote-only repos)
    pub async fn get_pr_diff(&self, repo: &str, number: u64) -> color_eyre::Result<String> {
        let resp = self
            .http
            .get(format!(
                "https://api.github.com/repos/{repo}/pulls/{number}"
            ))
            .header(ACCEPT, "application/vnd.github.diff")
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            color_eyre::eyre::bail!("GitHub API {status}: {body}");
        }

        Ok(resp.text().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pr_data(body: &str, html_url: &str) -> PrData {
        PrData {
            number: 42,
            title: "Test PR".to_string(),
            author: "octocat".to_string(),
            additions: 10,
            deletions: 5,
            changed_files: 2,
            head_ref: "feature-branch".to_string(),
            base_ref: "main".to_string(),
            head_sha: "abc123".to_string(),
            updated_at: "2025-01-15T08:30:00Z".to_string(),
            files: vec![],
            body: body.to_string(),
            html_url: html_url.to_string(),
        }
    }

    #[test]
    fn pr_data_body_field_stores_description() {
        let pr = make_pr_data("This PR fixes a bug.", "https://github.com/owner/repo/pull/42");
        assert_eq!(pr.body, "This PR fixes a bug.");
    }

    #[test]
    fn pr_data_html_url_field_stores_url() {
        let pr = make_pr_data("desc", "https://github.com/owner/repo/pull/42");
        assert_eq!(pr.html_url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn pr_data_empty_body_is_valid() {
        let pr = make_pr_data("", "https://github.com/owner/repo/pull/42");
        assert_eq!(pr.body, "");
    }

    #[test]
    fn pr_data_empty_html_url_is_valid() {
        let pr = make_pr_data("Some description", "");
        assert_eq!(pr.html_url, "");
    }

    #[test]
    fn pr_data_multiline_body() {
        let body = "Line one\nLine two\nLine three";
        let pr = make_pr_data(body, "https://github.com/owner/repo/pull/1");
        assert_eq!(pr.body.lines().count(), 3);
        assert_eq!(pr.body, body);
    }
}
