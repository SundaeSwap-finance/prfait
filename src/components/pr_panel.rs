use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::Frame;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::action::Action;
use crate::components::Component;
use crate::github::PrData;
use inspect_core::types::{EntityReview, ReviewResult};

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

/// Unique identifier for each node in the PR tree
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeId {
    Repo(String),
    Pr(String, u64),
    File(String, u64, String),
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeId::Repo(name) => write!(f, "repo:{name}"),
            NodeId::Pr(repo, num) => write!(f, "pr:{repo}#{num}"),
            NodeId::File(repo, num, path) => write!(f, "file:{repo}#{num}/{path}"),
        }
    }
}

/// Maps (entity_name, file_path) → list of (repo, pr_number) that touch this entity.
/// Used to detect cross-PR entity overlaps (merge conflict risk signal).
pub type OverlapMap = HashMap<(String, String), Vec<(String, u64)>>;

/// Multiplier applied to risk scores when an entity is modified in multiple open PRs.
pub const OVERLAP_RISK_BOOST: f64 = 1.5;

pub struct PrPanel {
    pub tree_state: TreeState<NodeId>,
    pub focused: bool,
    repos: HashMap<String, Vec<PrData>>,
    analyses: HashMap<(String, u64), ReviewResult>,
    loading_repos: Vec<String>,
    /// Number of pending review comments per (repo, pr_number, file_path)
    file_comment_counts: HashMap<(String, u64, String), usize>,
    /// Files the user has marked as reviewed: (repo, pr_number, file_path)
    reviewed_files: HashSet<(String, u64, String)>,
    /// Cross-PR entity overlap map, rebuilt whenever an analysis arrives
    pub overlap_map: OverlapMap,
}

impl PrPanel {
    pub fn new() -> Self {
        Self {
            tree_state: TreeState::default(),
            focused: true,
            repos: HashMap::new(),
            analyses: HashMap::new(),
            loading_repos: Vec::new(),
            file_comment_counts: HashMap::new(),
            reviewed_files: HashSet::new(),
            overlap_map: HashMap::new(),
        }
    }

    pub fn set_loading(&mut self, repo: &str) {
        if !self.loading_repos.contains(&repo.to_string()) {
            self.loading_repos.push(repo.to_string());
        }
    }

    pub fn set_prs(&mut self, repo: String, prs: Vec<PrData>) {
        self.loading_repos.retain(|r| r != &repo);
        self.repos.insert(repo, prs);
    }

    pub fn set_analysis(&mut self, repo: String, pr_number: u64, result: ReviewResult) {
        self.analyses.insert((repo, pr_number), result);
        self.rebuild_overlap_map();
    }

    /// Rebuild the overlap map from all stored analyses.
    /// Maps (entity_name, file_path) → list of (repo, pr_number) that touch it.
    fn rebuild_overlap_map(&mut self) {
        let mut map: OverlapMap = HashMap::new();
        for ((repo, pr_number), result) in &self.analyses {
            for entity in &result.entity_reviews {
                map.entry((entity.entity_name.clone(), entity.file_path.clone()))
                    .or_default()
                    .push((repo.clone(), *pr_number));
            }
        }
        self.overlap_map = map;
    }

    /// Get other PRs that touch the same (entity_name, file_path), excluding one PR.
    pub fn get_entity_overlaps(&self, entity_name: &str, file_path: &str, exclude_repo: &str, exclude_pr: u64) -> Vec<(String, u64)> {
        self.overlap_map
            .get(&(entity_name.to_string(), file_path.to_string()))
            .map(|prs| {
                prs.iter()
                    .filter(|(r, pr)| !(r == exclude_repo && *pr == exclude_pr))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn is_loading(&self) -> bool {
        !self.loading_repos.is_empty()
    }

    pub fn get_analysis(&self, repo: &str, pr_number: u64) -> Option<ReviewResult> {
        self.analyses.get(&(repo.to_string(), pr_number)).cloned()
    }

    /// Look up cached PrData for a specific PR.
    pub fn get_pr(&self, repo: &str, pr_number: u64) -> Option<&PrData> {
        self.repos.get(repo)?.iter().find(|p| p.number == pr_number)
    }

    /// Update file-level comment counts for a PR from the full pending comments list.
    pub fn set_comment_counts(&mut self, repo: &str, pr_number: u64, comments: &[crate::review::PendingComment]) {
        // Clear old counts for this PR
        self.file_comment_counts.retain(|&(ref r, pr, _), _| !(r == repo && pr == pr_number));
        // Tally per file
        for c in comments {
            *self.file_comment_counts
                .entry((repo.to_string(), pr_number, c.file_path.clone()))
                .or_insert(0) += 1;
        }
    }

    fn pr_comment_count(&self, repo: &str, pr_number: u64) -> usize {
        self.file_comment_counts
            .iter()
            .filter(|((r, pr, _), _)| r == repo && *pr == pr_number)
            .map(|(_, count)| count)
            .sum()
    }

    fn file_comment_count(&self, repo: &str, pr_number: u64, file_path: &str) -> usize {
        self.file_comment_counts
            .get(&(repo.to_string(), pr_number, file_path.to_string()))
            .copied()
            .unwrap_or(0)
    }

    pub fn mark_reviewed(&mut self, repo: &str, pr_number: u64, file_path: &str) {
        self.reviewed_files.insert((repo.to_string(), pr_number, file_path.to_string()));
        self.save_reviewed(repo, pr_number);
    }

    pub fn unmark_reviewed(&mut self, repo: &str, pr_number: u64, file_path: &str) {
        self.reviewed_files.remove(&(repo.to_string(), pr_number, file_path.to_string()));
        self.save_reviewed(repo, pr_number);
    }

    pub fn is_reviewed(&self, repo: &str, pr_number: u64, file_path: &str) -> bool {
        self.reviewed_files.contains(&(repo.to_string(), pr_number, file_path.to_string()))
    }

    /// Returns the next unreviewed file for this PR, sorted by risk descending.
    pub fn next_unreviewed_file(&self, repo: &str, pr_number: u64) -> Option<String> {
        let prs = self.repos.get(repo)?;
        let pr = prs.iter().find(|p| p.number == pr_number)?;
        let analysis = self.analyses.get(&(repo.to_string(), pr_number));

        let mut files_with_risk: Vec<(f64, &str)> = pr.files.iter().map(|f| {
            let risk = analysis
                .map(|a| max_file_risk(a, &f.path, repo, pr_number, &self.overlap_map))
                .unwrap_or(0.0);
            (risk, f.path.as_str())
        }).collect();

        files_with_risk.sort_by(|a, b| {
            b.0.partial_cmp(&a.0).unwrap()
                .then_with(|| a.1.cmp(&b.1))
        });

        files_with_risk.into_iter()
            .find(|(_, path)| !self.is_reviewed(repo, pr_number, path))
            .map(|(_, path)| path.to_string())
    }

    pub fn load_reviewed(&mut self, repo: &str, pr_number: u64) {
        // Clear any existing reviewed state for this PR
        self.reviewed_files.retain(|&(ref r, pr, _)| !(r == repo && pr == pr_number));
        let dir = reviewed_cache_dir();
        let filename = format!("{}__{}.json", repo.replace('/', "__"), pr_number);
        let path = dir.join(filename);
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(files) = serde_json::from_str::<Vec<String>>(&data) {
                for f in files {
                    self.reviewed_files.insert((repo.to_string(), pr_number, f));
                }
            }
        }
    }

    fn save_reviewed(&self, repo: &str, pr_number: u64) {
        let dir = reviewed_cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let filename = format!("{}__{}.json", repo.replace('/', "__"), pr_number);
        let path = dir.join(filename);
        let files: Vec<&str> = self.reviewed_files.iter()
            .filter(|(r, pr, _)| r == repo && *pr == pr_number)
            .map(|(_, _, f)| f.as_str())
            .collect();
        if files.is_empty() {
            let _ = std::fs::remove_file(path);
        } else if let Ok(json) = serde_json::to_string(&files) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Build the tree with labels truncated to fit `max_width`.
    /// Tree widget uses ~3 chars per indent level (arrow + space), plus 2 for border.
    fn build_tree(&self, max_width: u16) -> Vec<TreeItem<'static, NodeId>> {
        let w = max_width as usize;
        // Available chars at each depth (border=2, each indent level ~3 chars)
        let w0 = w.saturating_sub(4); // repo level
        let w1 = w.saturating_sub(7); // PR level
        let w2 = w.saturating_sub(10); // file level

        let mut repo_names: Vec<_> = self.repos.keys().cloned().collect();
        repo_names.sort();

        repo_names
            .into_iter()
            .map(|repo_name| {
                let mut prs = self.repos.get(&repo_name).cloned().unwrap_or_default();
                // Sort stale PRs first, preserving existing order as tiebreaker
                prs.sort_by_key(|pr| !is_stale(pr));
                let pr_items: Vec<TreeItem<'static, NodeId>> = prs
                    .iter()
                    .map(|pr| {
                        let analysis = self.analyses.get(&(repo_name.clone(), pr.number));

                        // Build file items with risk scores and reviewed state for sorting
                        let mut file_entries: Vec<(bool, f64, String, TreeItem<'static, NodeId>)> = pr
                            .files
                            .iter()
                            .map(|f| {
                                let max_risk = analysis
                                    .map(|a| max_file_risk(a, &f.path, &repo_name, pr.number, &self.overlap_map))
                                    .unwrap_or(0.0);

                                let reviewed = self.is_reviewed(&repo_name, pr.number, &f.path);
                                let indicator = change_indicator(&f.change_type);
                                let fc = self.file_comment_count(&repo_name, pr.number, &f.path);
                                let comment_suffix = if fc > 0 {
                                    format!(" [{fc}]")
                                } else {
                                    String::new()
                                };
                                let suffix_len = comment_suffix.len();

                                let check_prefix = if reviewed { "\u{2713} " } else { "" };
                                let label = if max_risk > 0.0 {
                                    format!("{check_prefix}{indicator} {} {max_risk:.2}", f.path)
                                } else {
                                    format!("{check_prefix}{indicator} {}", f.path)
                                };

                                let dim_style = Style::default().fg(Color::DarkGray);

                                let file_label = if reviewed {
                                    if fc > 0 {
                                        Line::from(vec![
                                            Span::styled(truncate(&label, w2.saturating_sub(suffix_len)), dim_style),
                                            Span::styled(comment_suffix, dim_style),
                                        ])
                                    } else {
                                        Line::from(Span::styled(truncate(&label, w2), dim_style))
                                    }
                                } else if fc > 0 {
                                    Line::from(vec![
                                        Span::raw(truncate(&label, w2.saturating_sub(suffix_len))),
                                        Span::styled(comment_suffix, Style::default().fg(Color::Yellow)),
                                    ])
                                } else {
                                    Line::from(truncate(&label, w2))
                                };

                                let item = TreeItem::new_leaf(
                                    NodeId::File(
                                        repo_name.clone(),
                                        pr.number,
                                        f.path.clone(),
                                    ),
                                    file_label,
                                );

                                (reviewed, max_risk, f.path.clone(), item)
                            })
                            .collect();

                        // Sort: unreviewed first (false < true), then by risk descending, then by path
                        file_entries.sort_by(|a, b| {
                            a.0.cmp(&b.0)
                                .then_with(|| b.1.partial_cmp(&a.1).unwrap())
                                .then_with(|| a.2.cmp(&b.2))
                        });
                        let file_items: Vec<_> = file_entries.into_iter().map(|(_, _, _, item)| item).collect();

                        // PR label with colored badge at start
                        let comment_count = self.pr_comment_count(&repo_name, pr.number);
                        let comment_suffix = if comment_count > 0 {
                            format!(" [{comment_count}]")
                        } else {
                            String::new()
                        };
                        let suffix_len = comment_suffix.len();

                        let stale = is_stale(pr);
                        let stale_suffix = " STALE";
                        let stale_len = if stale { stale_suffix.len() } else { 0 };

                        let pr_label = if let Some(a) = analysis {
                            let (badge, badge_color) = risk_badge_styled(&a.stats);
                            let rest = truncate(
                                &format!("#{} {}", pr.number, &pr.title),
                                w1.saturating_sub(badge.len() + 1 + suffix_len + stale_len),
                            );
                            let mut spans = vec![
                                Span::styled(badge, Style::default().fg(badge_color)),
                                Span::raw(" "),
                                Span::raw(rest),
                            ];
                            if stale {
                                spans.push(Span::styled(stale_suffix, Style::default().fg(Color::Magenta)));
                            }
                            if comment_count > 0 {
                                spans.push(Span::styled(
                                    comment_suffix,
                                    Style::default().fg(Color::Yellow),
                                ));
                            }
                            Line::from(spans)
                        } else {
                            let mut spans = vec![Span::raw(truncate(
                                &format!("#{} {}", pr.number, &pr.title),
                                w1.saturating_sub(suffix_len + stale_len),
                            ))];
                            if stale {
                                spans.push(Span::styled(stale_suffix, Style::default().fg(Color::Magenta)));
                            }
                            if comment_count > 0 {
                                spans.push(Span::styled(
                                    comment_suffix,
                                    Style::default().fg(Color::Yellow),
                                ));
                            }
                            Line::from(spans)
                        };

                        TreeItem::new(
                            NodeId::Pr(repo_name.clone(), pr.number),
                            pr_label,
                            file_items,
                        )
                        .expect("valid tree item")
                    })
                    .collect();

                let count = prs.len();
                let label = format!("{repo_name} ({count} PRs)");
                TreeItem::new(
                    NodeId::Repo(repo_name),
                    truncate(&label, w0),
                    pr_items,
                )
                .expect("valid tree item")
            })
            .collect()
    }

    pub fn selected_node(&self) -> Option<&NodeId> {
        self.tree_state.selected().last()
    }

    fn selection_changed_action(&self) -> Action {
        match self.selected_node() {
            Some(NodeId::Pr(repo, pr)) => Action::AnalyzePr(repo.clone(), *pr),
            Some(NodeId::File(repo, pr, _)) => Action::AnalyzePr(repo.clone(), *pr),
            _ => Action::Noop,
        }
    }
}

impl Component for PrPanel {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::TreeDown,
            KeyCode::Char('k') | KeyCode::Up => Action::TreeUp,
            KeyCode::Char('h') | KeyCode::Left => Action::TreeLeft,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => Action::TreeRight,
            KeyCode::Char('r') => Action::RefreshPrs,
            _ => Action::Noop,
        }
    }

    fn update(&mut self, action: &Action) -> Action {
        match action {
            Action::TreeDown => {
                self.tree_state.key_down();
                self.selection_changed_action()
            }
            Action::TreeUp => {
                self.tree_state.key_up();
                self.selection_changed_action()
            }
            Action::TreeLeft => {
                self.tree_state.key_left();
                Action::Noop
            }
            Action::TreeRight => {
                self.tree_state.toggle_selected();
                self.selection_changed_action()
            }
            Action::TreeClick(col, row) => {
                let pos = Position::new(*col, *row);
                let previously_selected = self.tree_state.selected().to_vec();
                if self.tree_state.click_at(pos) {
                    // click_at() handles same-item clicks (toggles expand/collapse)
                    // For different-item clicks, also toggle to match click behavior
                    if self.tree_state.selected() != previously_selected {
                        self.tree_state.toggle_selected();
                    }
                    self.selection_changed_action()
                } else {
                    Action::Noop
                }
            }
            Action::PrsLoaded(repo, prs) => {
                self.set_prs(repo.clone(), prs.clone());
                Action::Noop
            }
            Action::AnalysisComplete(repo, pr_number, result) => {
                self.set_analysis(repo.clone(), *pr_number, *result.clone());
                Action::Noop
            }
            _ => Action::Noop,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = if self.is_loading() {
            " Pull Requests (loading...) "
        } else {
            " Pull Requests "
        };

        let block = Block::bordered()
            .title(Span::raw(title))
            .border_style(border_style);

        let items = self.build_tree(area.width);
        let tree = Tree::new(&items)
            .expect("valid tree")
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(tree, area, &mut self.tree_state);
    }
}

fn change_indicator(change_type: &str) -> &'static str {
    match change_type.to_uppercase().as_str() {
        "ADDED" => "+",
        "DELETED" | "REMOVED" => "-",
        "RENAMED" => ">",
        _ => "~",
    }
}

use inspect_core::types::ReviewStats;

/// Returns (badge_text, color) for a PR's risk level
fn risk_badge_styled(stats: &ReviewStats) -> (String, Color) {
    if stats.by_risk.critical > 0 {
        ("[!!]".to_string(), Color::Red)
    } else if stats.by_risk.high > 0 {
        ("[!]".to_string(), Color::Yellow)
    } else if stats.by_risk.medium > 0 {
        ("[~]".to_string(), Color::Blue)
    } else {
        ("[ok]".to_string(), Color::Green)
    }
}

fn reviewed_cache_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"))
        .join("prfait")
        .join("reviewed")
}

fn truncate(s: &str, max: usize) -> String {
    if max < 4 {
        return s.chars().take(max).collect();
    }
    if s.len() <= max {
        s.to_string()
    } else {
        // Truncate at char boundary
        let end = s
            .char_indices()
            .take_while(|(i, _)| *i < max - 1)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}.", &s[..end])
    }
}

/// Returns true if the PR has not been updated in more than 7 days.
/// Parses GitHub's ISO 8601 timestamps (`YYYY-MM-DDTHH:MM:SSZ`).
fn is_stale(pr: &PrData) -> bool {
    parse_github_timestamp(&pr.updated_at)
        .and_then(|updated| SystemTime::now().duration_since(updated).ok())
        .is_some_and(|age| age > Duration::from_secs(7 * 24 * 60 * 60))
}

/// Parse a `YYYY-MM-DDTHH:MM:SSZ` timestamp into `SystemTime`.
fn parse_github_timestamp(s: &str) -> Option<SystemTime> {
    // Expected: "2025-01-15T08:30:00Z" (20 chars)
    if s.len() < 20 || s.as_bytes()[19] != b'Z' {
        return None;
    }
    let b = s.as_bytes();
    let year: u64 = s[0..4].parse().ok()?;
    let month: u64 = s[5..7].parse().ok()?;
    let day: u64 = s[8..10].parse().ok()?;
    let hour: u64 = s[11..13].parse().ok()?;
    let min: u64 = s[14..16].parse().ok()?;
    let sec: u64 = s[17..19].parse().ok()?;

    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }

    // Days from year 1970 to start of `year`
    let y = year - 1;
    let mut days = 365 * (year - 1970)
        + (y / 4 - 1970 / 4)
        - (y / 100 - 1970 / 100)
        + (y / 400 - 1970 / 400);

    let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap {
            days += 1;
        }
    }
    days += day - 1;

    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

// ── Cross-PR overlap helpers ────────────────────────────────────────────

/// Build per-entity overlap map for a specific PR, filtering out self-references.
pub fn compute_entity_overlaps(
    entities: &[EntityReview],
    repo: &str,
    pr_number: u64,
    overlap_map: &OverlapMap,
) -> HashMap<String, Vec<(String, u64)>> {
    let mut out = HashMap::new();
    for entity in entities {
        let others: Vec<(String, u64)> = overlap_map
            .get(&(entity.entity_name.clone(), entity.file_path.clone()))
            .map(|prs| {
                prs.iter()
                    .filter(|(r, pn)| !(r == repo && *pn == pr_number))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !others.is_empty() {
            out.insert(entity.entity_name.clone(), others);
        }
    }
    out
}

/// Apply overlap boost to entity risk scores and sort by risk descending, then name.
/// Mutates the slice in place — call on a clone if the originals must stay untouched.
pub fn sort_entities_by_risk(
    entities: &mut [EntityReview],
    entity_overlaps: &HashMap<String, Vec<(String, u64)>>,
) {
    for entity in entities.iter_mut() {
        if entity_overlaps.contains_key(&entity.entity_name) {
            entity.risk_score *= OVERLAP_RISK_BOOST;
        }
    }
    entities.sort_by(|a, b| {
        b.risk_score
            .partial_cmp(&a.risk_score)
            .unwrap()
            .then_with(|| a.entity_name.cmp(&b.entity_name))
    });
}

/// Compute the maximum effective risk score across all entities in a file.
pub fn max_file_risk(
    analysis: &ReviewResult,
    file_path: &str,
    repo: &str,
    pr_number: u64,
    overlap_map: &OverlapMap,
) -> f64 {
    analysis
        .entity_reviews
        .iter()
        .filter(|e| e.file_path == file_path)
        .map(|e| {
            let has_overlap = overlap_map
                .get(&(e.entity_name.clone(), e.file_path.clone()))
                .is_some_and(|prs| prs.iter().any(|(r, pn)| !(r == repo && *pn == pr_number)));
            if has_overlap {
                e.risk_score * OVERLAP_RISK_BOOST
            } else {
                e.risk_score
            }
        })
        .reduce(f64::max)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PrData;

    fn make_pr(number: u64, body: &str, html_url: &str) -> PrData {
        PrData {
            number,
            title: format!("PR #{number}"),
            author: "author".to_string(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            head_ref: "head".to_string(),
            base_ref: "main".to_string(),
            head_sha: "sha".to_string(),
            updated_at: "2025-01-15T08:30:00Z".to_string(),
            files: vec![],
            body: body.to_string(),
            html_url: html_url.to_string(),
        }
    }

    #[test]
    fn get_pr_returns_pr_by_number() {
        let mut panel = PrPanel::new();
        panel.set_prs("owner/repo".to_string(), vec![
            make_pr(1, "first PR body", "https://github.com/owner/repo/pull/1"),
            make_pr(2, "second PR body", "https://github.com/owner/repo/pull/2"),
        ]);

        let pr = panel.get_pr("owner/repo", 1).expect("PR 1 should exist");
        assert_eq!(pr.number, 1);
        assert_eq!(pr.body, "first PR body");
        assert_eq!(pr.html_url, "https://github.com/owner/repo/pull/1");
    }

    #[test]
    fn get_pr_returns_correct_pr_when_multiple_exist() {
        let mut panel = PrPanel::new();
        panel.set_prs("owner/repo".to_string(), vec![
            make_pr(10, "body ten", "https://github.com/owner/repo/pull/10"),
            make_pr(20, "body twenty", "https://github.com/owner/repo/pull/20"),
        ]);

        let pr = panel.get_pr("owner/repo", 20).expect("PR 20 should exist");
        assert_eq!(pr.number, 20);
        assert_eq!(pr.body, "body twenty");
        assert_eq!(pr.html_url, "https://github.com/owner/repo/pull/20");
    }

    #[test]
    fn get_pr_returns_none_for_missing_number() {
        let mut panel = PrPanel::new();
        panel.set_prs("owner/repo".to_string(), vec![
            make_pr(1, "body", "https://github.com/owner/repo/pull/1"),
        ]);

        assert!(panel.get_pr("owner/repo", 999).is_none());
    }

    #[test]
    fn get_pr_returns_none_for_missing_repo() {
        let mut panel = PrPanel::new();
        panel.set_prs("owner/repo".to_string(), vec![
            make_pr(1, "body", "https://github.com/owner/repo/pull/1"),
        ]);

        assert!(panel.get_pr("other/repo", 1).is_none());
    }

    #[test]
    fn get_pr_returns_pr_with_empty_body_and_url() {
        let mut panel = PrPanel::new();
        panel.set_prs("owner/repo".to_string(), vec![
            make_pr(5, "", ""),
        ]);

        let pr = panel.get_pr("owner/repo", 5).expect("PR 5 should exist");
        assert_eq!(pr.body, "");
        assert_eq!(pr.html_url, "");
    }

    #[test]
    fn get_pr_returns_pr_with_multiline_body() {
        let body = "Line 1\nLine 2\nLine 3";
        let mut panel = PrPanel::new();
        panel.set_prs("owner/repo".to_string(), vec![
            make_pr(7, body, "https://github.com/owner/repo/pull/7"),
        ]);

        let pr = panel.get_pr("owner/repo", 7).expect("PR 7 should exist");
        assert_eq!(pr.body.lines().count(), 3);
    }
}
