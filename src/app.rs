use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use tokio::sync::{mpsc, Mutex};

use crate::action::Action;
use crate::analysis::AnalysisManager;
use crate::checks::CheckManager;
use crate::components::diff_panel::{DiffPanel, FileContext};
use crate::components::pr_panel::{NodeId, PrPanel};
use crate::components::status_bar::StatusBar;
use crate::components::Component;
use crate::config::Config;
use crate::github::{GithubClient, PrData};
use crate::review::{BodyEditor, BodyEditorPurpose, DragState, InlineEditor, PendingComment, ReviewEvent, ReviewState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    PrList,
    DiffView,
}

pub struct App {
    pub should_quit: bool,
    pub focus: Focus,
    pub pr_panel: PrPanel,
    pub diff_panel: DiffPanel,
    pub status_bar: StatusBar,
    pub config: Config,
    pub action_tx: mpsc::UnboundedSender<Action>,
    pub github_client: Arc<GithubClient>,
    pub analysis: AnalysisManager,
    pub checks: CheckManager,
    pub review: ReviewState,
    pr_cache: HashMap<String, Vec<PrData>>,
    comments_cache: HashMap<(String, u64), crate::review::PrComments>,
    /// Cached git file contents: (repo, pr_number, file_path) → (before, after)
    file_content_cache: Arc<Mutex<HashMap<(String, u64, String), (String, String)>>>,
    show_sidebar: bool,
    show_help: bool,
    /// Search state
    search_input: Option<String>,
    search_query: Option<String>,
    search_matches: Vec<usize>,
    search_match_idx: Option<usize>,
    /// Saved scroll/cursor before search, restored on Esc
    search_saved_scroll: u16,
    search_saved_cursor: Option<usize>,
    /// Esc-to-quit confirmation: true if Esc was pressed once recently
    esc_pending: bool,
    /// Layout areas from last render (for mouse hit-testing)
    left_area: Rect,
    right_area: Rect,
}

impl App {
    pub fn new(
        config: Config,
        action_tx: mpsc::UnboundedSender<Action>,
        github_client: Arc<GithubClient>,
    ) -> Self {
        let analysis = AnalysisManager::new();
        Self {
            should_quit: false,
            focus: Focus::PrList,
            pr_panel: PrPanel::new(),
            diff_panel: DiffPanel::new(),
            status_bar: StatusBar::new(),
            config,
            action_tx,
            github_client,
            analysis,
            checks: CheckManager::new(),
            review: ReviewState::new(),
            pr_cache: HashMap::new(),
            comments_cache: HashMap::new(),
            file_content_cache: Arc::new(Mutex::new(HashMap::new())),
            show_sidebar: true,
            show_help: false,
            search_input: None,
            search_query: None,
            search_matches: Vec::new(),
            search_match_idx: None,
            search_saved_scroll: 0,
            search_saved_cursor: None,
            esc_pending: false,
            left_area: Rect::default(),
            right_area: Rect::default(),
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        // When search input is active, route keys there
        if self.search_input.is_some() {
            return self.handle_search_key(key);
        }

        // When body editor is active, route keys there
        if self.review.body_editor.is_some() {
            return self.handle_body_editor_key(key);
        }

        // When inline editor is active, route keys there
        if self.review.inline_editor.is_some() {
            return self.handle_editor_key(key);
        }

        // When submit mode is active, route keys to submit handler
        if self.review.submit_mode {
            return self.handle_submit_key(key);
        }

        // When help overlay is shown, any key dismisses it
        if self.show_help {
            self.show_help = false;
            return Action::Noop;
        }

        // Any non-Esc key clears the esc-pending state
        if key.code != KeyCode::Esc {
            self.esc_pending = false;
        }

        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc => {
                // Clear search if active
                if self.search_query.is_some() {
                    self.search_query = None;
                    self.search_matches.clear();
                    self.search_match_idx = None;
                    return Action::Noop;
                }
                // Go back: DiffView → PrList (show sidebar)
                if self.focus == Focus::DiffView {
                    self.show_sidebar = true;
                    self.focus = Focus::PrList;
                    self.pr_panel.focused = true;
                    self.diff_panel.focused = false;
                    return Action::Noop;
                }
                // PrList: double-tap Esc to quit
                if self.esc_pending {
                    return Action::Quit;
                }
                self.esc_pending = true;
                return Action::Noop;
            }
            KeyCode::Tab => {
                if self.focus == Focus::PrList {
                    // Leaving PrList: hide sidebar, focus diff
                    self.show_sidebar = false;
                    self.focus = Focus::DiffView;
                    self.pr_panel.focused = false;
                    self.diff_panel.focused = true;
                } else {
                    // Leaving DiffView: show sidebar, focus PrList
                    self.show_sidebar = true;
                    self.focus = Focus::PrList;
                    self.pr_panel.focused = true;
                    self.diff_panel.focused = false;
                }
                return Action::Noop;
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                return Action::Noop;
            }
            KeyCode::Char('d') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::ToggleDiffMode
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Quit
            }
            _ => {}
        }

        match self.focus {
            Focus::PrList => match key.code {
                KeyCode::Char('j') | KeyCode::Down => Action::TreeDown,
                KeyCode::Char('k') | KeyCode::Up => Action::TreeUp,
                KeyCode::Char('h') | KeyCode::Left => Action::TreeLeft,
                KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => Action::TreeRight,
                KeyCode::Char('c') => {
                    if matches!(self.pr_panel.selected_node(), Some(NodeId::Pr(..))) {
                        self.review.body_editor = Some(BodyEditor::new(
                            BodyEditorPurpose::IssueComment,
                        ));
                    }
                    Action::Noop
                }
                KeyCode::Char('x') => {
                    if let Some(NodeId::File(repo, pr, path)) = self.pr_panel.selected_node().cloned() {
                        Action::MarkFileReviewed(repo, pr, path)
                    } else {
                        Action::Noop
                    }
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Action::OpenReviewSubmit
                }
                KeyCode::Char('r') => Action::RefreshPrs,
                _ => Action::Noop,
            },
            Focus::DiffView => match key.code {
                KeyCode::Char('J') => Action::ScrollHalfPageDown,
                KeyCode::Char('K') => Action::ScrollHalfPageUp,
                KeyCode::Char('j') | KeyCode::Down => Action::CursorDown,
                KeyCode::Char('k') | KeyCode::Up => Action::CursorUp,
                KeyCode::Enter | KeyCode::Char('c') => Action::CursorComment,
                KeyCode::Char('{') => Action::JumpPrevHunk,
                KeyCode::Char('}') => Action::JumpNextHunk,
                KeyCode::Char('x') => {
                    if let Some((repo, pr_number)) = self.diff_panel.current_context().cloned() {
                        if let Some(file) = self.diff_panel.current_file().map(|f| f.to_string()) {
                            Action::MarkFileReviewed(repo, pr_number, file)
                        } else {
                            Action::Noop
                        }
                    } else {
                        Action::Noop
                    }
                }
                KeyCode::Char('h') | KeyCode::Left => Action::ScrollLeft(4),
                KeyCode::Char('l') | KeyCode::Right => Action::ScrollRight(4),
                KeyCode::PageDown => Action::ScrollHalfPageDown,
                KeyCode::PageUp => Action::ScrollHalfPageUp,
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Action::OpenReviewSubmit
                }
                KeyCode::Char('e') => Action::OpenInEditor,
                KeyCode::Char('g') => Action::ScrollToTop,
                KeyCode::Char('G') => Action::ScrollToBottom,
                KeyCode::Char('0') | KeyCode::Home => Action::ScrollLeft(u16::MAX),
                KeyCode::Char('$') | KeyCode::End => Action::ScrollRight(u16::MAX),
                KeyCode::Char('/') => {
                    self.search_saved_scroll = self.diff_panel.scroll_y;
                    self.search_saved_cursor = self.diff_panel.cursor_line;
                    self.search_input = Some(String::new());
                    Action::Noop
                }
                KeyCode::Char('n') => Action::SearchNext,
                KeyCode::Char('N') => Action::SearchPrev,
                _ => Action::Noop,
            },
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                // Abort search, restore position
                self.search_input = None;
                self.search_query = None;
                self.search_matches.clear();
                self.search_match_idx = None;
                self.diff_panel.scroll_y = self.search_saved_scroll;
                self.diff_panel.cursor_line = self.search_saved_cursor;
                Action::Noop
            }
            KeyCode::Enter => {
                // Commit search, stay at current position
                let query = self.search_input.take().unwrap_or_default();
                if query.is_empty() {
                    self.search_query = None;
                    self.search_matches.clear();
                    self.search_match_idx = None;
                } else {
                    self.search_query = Some(query);
                    self.rebuild_search_matches();
                    if !self.search_matches.is_empty() {
                        let scroll = self.diff_panel.scroll_y as usize;
                        let idx = self.search_matches.iter()
                            .position(|&line| line >= scroll)
                            .unwrap_or(0);
                        self.search_match_idx = Some(idx);
                        self.jump_to_search_match(self.search_matches[idx]);
                    }
                }
                Action::Noop
            }
            KeyCode::Backspace => {
                if let Some(ref mut input) = self.search_input {
                    input.pop();
                }
                Action::Noop
            }
            KeyCode::Char(c) => {
                if let Some(ref mut input) = self.search_input {
                    input.push(c);
                }
                Action::Noop
            }
            _ => Action::Noop,
        }
    }

    fn jump_to_search_match(&mut self, line: usize) {
        self.diff_panel.scroll_y = line as u16;
        self.diff_panel.cursor_line = self.diff_panel.nearest_commentable_in_viewport(line);
    }

    fn rebuild_search_matches(&mut self) {
        self.search_matches.clear();
        self.search_match_idx = None;
        let query = match &self.search_query {
            Some(q) => q.to_lowercase(),
            None => return,
        };
        for (i, line) in self.diff_panel.lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            if text.to_lowercase().contains(&query) {
                self.search_matches.push(i);
            }
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Action {
        let editor = match self.review.inline_editor.as_mut() {
            Some(e) => e,
            None => return Action::Noop,
        };

        match key.code {
            KeyCode::Esc => Action::CancelComment,
            // Alt+Enter or Ctrl+S to save
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                let body = editor.body();
                if body.trim().is_empty() {
                    match editor.editing_index {
                        Some(idx) => Action::DeleteComment(idx),
                        None => Action::CancelComment,
                    }
                } else {
                    Action::SaveComment(body)
                }
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let body = editor.body();
                if body.trim().is_empty() {
                    match editor.editing_index {
                        Some(idx) => Action::DeleteComment(idx),
                        None => Action::CancelComment,
                    }
                } else {
                    Action::SaveComment(body)
                }
            }
            // Ctrl+D to delete the comment being edited
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(idx) = editor.editing_index {
                    Action::DeleteComment(idx)
                } else {
                    Action::CancelComment
                }
            }
            KeyCode::Enter => {
                editor.insert_newline();
                Action::Noop
            }
            KeyCode::Backspace => {
                editor.backspace();
                Action::Noop
            }
            KeyCode::Delete => {
                // If editor is empty and we're editing an existing comment, delete it
                if editor.body().is_empty() {
                    if let Some(idx) = editor.editing_index {
                        return Action::DeleteComment(idx);
                    }
                }
                editor.delete();
                Action::Noop
            }
            KeyCode::Left => {
                editor.move_left();
                Action::Noop
            }
            KeyCode::Right => {
                editor.move_right();
                Action::Noop
            }
            KeyCode::Up => {
                editor.move_up();
                Action::Noop
            }
            KeyCode::Down => {
                editor.move_down();
                Action::Noop
            }
            KeyCode::Home => {
                editor.move_home();
                Action::Noop
            }
            KeyCode::End => {
                editor.move_end();
                Action::Noop
            }
            KeyCode::Char(c) => {
                editor.insert_char(c);
                Action::Noop
            }
            _ => Action::Noop,
        }
    }

    fn handle_submit_key(&mut self, key: KeyEvent) -> Action {
        self.review.submit_mode = false;
        self.status_bar.submit_mode = false;
        match key.code {
            KeyCode::Char('a') => Action::SubmitReview(ReviewEvent::Approve),
            KeyCode::Char('r') => {
                if self.review.comments.is_empty() {
                    // No inline comments — collect a review body
                    self.review.body_editor = Some(BodyEditor::new(
                        BodyEditorPurpose::ReviewBody(ReviewEvent::RequestChanges),
                    ));
                    Action::Noop
                } else {
                    Action::SubmitReview(ReviewEvent::RequestChanges)
                }
            }
            KeyCode::Char('c') => {
                if self.review.comments.is_empty() {
                    // No inline comments — collect a review body
                    self.review.body_editor = Some(BodyEditor::new(
                        BodyEditorPurpose::ReviewBody(ReviewEvent::Comment),
                    ));
                    Action::Noop
                } else {
                    Action::SubmitReview(ReviewEvent::Comment)
                }
            }
            _ => Action::Noop,
        }
    }

    fn handle_body_editor_key(&mut self, key: KeyEvent) -> Action {
        let editor = match self.review.body_editor.as_mut() {
            Some(e) => e,
            None => return Action::Noop,
        };

        match key.code {
            KeyCode::Esc => {
                self.review.body_editor = None;
                Action::Noop
            }
            // Alt+Enter or Ctrl+S to submit
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                self.submit_body_editor()
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.submit_body_editor()
            }
            KeyCode::Enter => {
                editor.insert_newline();
                Action::Noop
            }
            KeyCode::Backspace => {
                editor.backspace();
                Action::Noop
            }
            KeyCode::Delete => {
                editor.delete();
                Action::Noop
            }
            KeyCode::Left => {
                editor.move_left();
                Action::Noop
            }
            KeyCode::Right => {
                editor.move_right();
                Action::Noop
            }
            KeyCode::Up => {
                editor.move_up();
                Action::Noop
            }
            KeyCode::Down => {
                editor.move_down();
                Action::Noop
            }
            KeyCode::Home => {
                editor.move_home();
                Action::Noop
            }
            KeyCode::End => {
                editor.move_end();
                Action::Noop
            }
            KeyCode::Char(c) => {
                editor.insert_char(c);
                Action::Noop
            }
            _ => Action::Noop,
        }
    }

    fn submit_body_editor(&mut self) -> Action {
        let editor = match self.review.body_editor.take() {
            Some(e) => e,
            None => return Action::Noop,
        };
        let body = editor.body();
        if body.trim().is_empty() {
            return Action::Noop;
        }
        match editor.purpose {
            BodyEditorPurpose::ReviewBody(event) => {
                Action::SubmitReviewWithBody(event, body)
            }
            BodyEditorPurpose::IssueComment => {
                Action::PostIssueComment(body)
            }
        }
    }

    pub fn update(&mut self, action: &Action) -> Action {
        match action {
            Action::Quit => {
                self.should_quit = true;
                return Action::Noop;
            }
            Action::FocusNext => {
                self.show_sidebar = !self.show_sidebar;
                return Action::Noop;
            }
            Action::RefreshPrs => {
                self.start_loading_prs();
                return Action::Noop;
            }
            _ => {}
        }

        // Forward to components
        let pr_action = self.pr_panel.update(action);
        self.diff_panel.update(action);
        self.status_bar.update(action);

        // Handle review-specific actions
        match action {
            Action::DiffMouseDown(col, row) => {
                self.handle_mouse_down(*col, *row);
            }
            Action::DiffMouseUp(col, row) => {
                return self.handle_mouse_up(*col, *row);
            }
            Action::DiffMouseDrag(col, row) => {
                self.handle_mouse_drag(*col, *row);
            }
            Action::SaveComment(body) => {
                self.save_comment(body.clone());
                self.review.save_to_disk();
            }
            Action::CancelComment => {
                self.review.inline_editor = None;
            }
            Action::DeleteComment(idx) => {
                if *idx < self.review.comments.len() {
                    self.review.comments.remove(*idx);
                }
                self.review.inline_editor = None;
                self.status_bar.review_count = self.review.comments.len();
                self.sync_comment_count();
                self.review.save_to_disk();
            }
            Action::OpenReviewSubmit => {
                self.review.submit_mode = true;
                self.status_bar.submit_mode = true;
            }
            Action::SubmitReview(event) => {
                self.submit_review(event.clone(), String::new());
            }
            Action::SubmitReviewWithBody(event, body) => {
                self.submit_review(event.clone(), body.clone());
            }
            Action::PostIssueComment(body) => {
                self.post_issue_comment(body.clone());
            }
            Action::IssueCommentPosted => {
                // Refresh comments to show the new one
                if let (Some(repo), Some(pr)) = (&self.review.repo, self.review.pr_number) {
                    self.fetch_pr_comments(repo, pr);
                }
            }
            Action::IssueCommentError(_) => {
                // Error shown via status bar
            }
            Action::ReviewSubmitted(_url) => {
                self.review.comments.clear();
                self.status_bar.review_count = 0;
                self.sync_comment_count();
                self.review.save_to_disk();
            }
            Action::ReviewError(_) => {
                // Keep comments, error shown in status bar
            }
            Action::MarkFileReviewed(repo, pr_number, file_path) => {
                if self.pr_panel.is_reviewed(repo, *pr_number, file_path) {
                    self.pr_panel.unmark_reviewed(repo, *pr_number, file_path);
                } else {
                    self.pr_panel.mark_reviewed(repo, *pr_number, file_path);
                    if let Some(next) = self.pr_panel.next_unreviewed_file(repo, *pr_number) {
                        self.navigate_to_file(repo, *pr_number, &next);
                    }
                }
            }
            Action::NavigateToFile(repo, pr_number, file_path) => {
                self.navigate_to_file(repo, *pr_number, file_path);
            }
            Action::SearchNext => {
                if !self.search_matches.is_empty() {
                    let idx = match self.search_match_idx {
                        Some(i) => (i + 1) % self.search_matches.len(),
                        None => 0,
                    };
                    self.search_match_idx = Some(idx);
                    self.jump_to_search_match(self.search_matches[idx]);
                }
            }
            Action::SearchPrev => {
                if !self.search_matches.is_empty() {
                    let idx = match self.search_match_idx {
                        Some(0) | None => self.search_matches.len() - 1,
                        Some(i) => i - 1,
                    };
                    self.search_match_idx = Some(idx);
                    self.jump_to_search_match(self.search_matches[idx]);
                }
            }
            Action::CursorComment => {
                if let Some(cursor) = self.diff_panel.cursor_line {
                    // Check if this line has an existing thread
                    let has_thread = self.cursor_has_thread(cursor);
                    let has_pending = self.cursor_has_pending(cursor);
                    if has_thread && !has_pending {
                        let is_expanded = self.cursor_thread_expanded(cursor);
                        if !is_expanded {
                            // First press: expand thread
                            self.toggle_thread_expansion(cursor);
                        } else {
                            // Already expanded: open reply editor
                            self.open_reply_editor(cursor);
                        }
                    } else {
                        self.open_inline_editor(cursor, None);
                    }
                }
            }
            Action::OpenInEditor => {
                return self.prepare_editor_launch();
            }
            _ => {}
        }

        // Handle check actions
        match action {
            Action::ChecksStarted(repo, pr, sha) => {
                self.checks.mark_in_flight(repo, *pr);
                self.checks.states.insert(
                    (repo.clone(), *pr),
                    crate::checks::PrCheckState {
                        sha: sha.clone(),
                        source: crate::checks::CheckSource::Local,
                        checks: vec![],
                    },
                );
            }
            Action::ChecksUpdate(repo, pr, results) => {
                if let Some(state) = self.checks.states.get_mut(&(repo.clone(), *pr)) {
                    state.checks = results.clone();
                }
                self.refresh_summary_if_viewing(repo, *pr);
            }
            Action::ChecksComplete(repo, pr, state) => {
                self.checks.clear_in_flight(repo, *pr);
                self.checks.states.insert((repo.clone(), *pr), state.clone());
                self.refresh_summary_if_viewing(repo, *pr);
            }
            _ => {}
        }

        // Handle data loading — eagerly analyze all PRs
        if let Action::PrsLoaded(repo, prs) = action {
            self.pr_cache.insert(repo.clone(), prs.clone());
            for pr in prs {
                self.trigger_analysis(repo, pr.number);
                self.fetch_pr_comments(repo, pr.number);
            }
        }

        // Handle loaded comments — just cache; render() reads from cache directly
        if let Action::CommentsLoaded(repo, pr_number, comments) = action {
            self.comments_cache.insert((repo.clone(), *pr_number), (**comments).clone());
        }

        // When analysis completes, auto-update diff if we're on a matching file/PR
        if let Action::AnalysisComplete(repo, pr_number, result) = action {
            self.update_diff_for_selection(Some((repo.clone(), *pr_number, (**result).clone())));
            // Eagerly prefetch file content for this PR
            self.prefetch_file_content(repo, *pr_number, result);
            // Store head_sha for review submission
            if let Some(prs) = self.pr_cache.get(repo) {
                if let Some(pr) = prs.iter().find(|p| p.number == *pr_number) {
                    let changed_pr = self.review.repo.as_deref() != Some(repo)
                        || self.review.pr_number != Some(*pr_number);
                    let head_sha = pr.head_sha.clone();
                    self.review.repo = Some(repo.clone());
                    self.review.pr_number = Some(*pr_number);
                    self.review.head_sha = Some(head_sha.clone());
                    if changed_pr {
                        self.review.load_from_disk();
                        self.status_bar.review_count = self.review.comments.len();
                        self.sync_comment_count();
                    }
                    // Always reload reviewed files so SHA changes trigger invalidation
                    self.pr_panel.load_reviewed(repo, *pr_number, &head_sha);

                    // Trigger checks only for the currently selected PR
                    let is_selected = matches!(
                        self.pr_panel.selected_node(),
                        Some(NodeId::Pr(r, n)) | Some(NodeId::File(r, n, _))
                            if r == repo && *n == *pr_number
                    );
                    if is_selected {
                        self.maybe_trigger_checks(repo, *pr_number);
                    }
                }
            }
        }

        // Handle follow-up from tree navigation
        match &pr_action {
            Action::AnalyzePr(repo, pr_number) => {
                self.trigger_analysis(repo, *pr_number);
                self.update_diff_for_selection(None);
            }
            _ => {}
        }

        Action::Noop
    }

    /// Update the diff panel based on current tree selection
    fn update_diff_for_selection(
        &mut self,
        new_analysis: Option<(String, u64, inspect_core::types::ReviewResult)>,
    ) {
        let selected = self.pr_panel.selected_node().cloned();

        match selected {
            Some(NodeId::File(ref repo, pr_number, ref path)) => {
                let result = if let Some((ref r, pn, ref res)) = new_analysis {
                    if r == repo && pn == pr_number {
                        Some(res.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(result) = result.or_else(|| self.pr_panel.get_analysis(repo, pr_number))
                {
                    let file_ctx = self.build_file_context(repo, pr_number);
                    let cached = self.lookup_cached_content(repo, pr_number, path);
                    self.diff_panel
                        .show_file(repo, pr_number, path, &result, file_ctx.as_ref(), cached, &self.pr_panel.overlap_map);
                }
            }
            Some(NodeId::Pr(ref repo, pr_number)) => {
                let result = if let Some((ref r, pn, ref res)) = new_analysis {
                    if r == repo && pn == pr_number {
                        Some(res.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(result) = result.or_else(|| self.pr_panel.get_analysis(repo, pr_number))
                {
                    let pr_data = self.pr_panel.get_pr(repo, pr_number);
                    let checks = self.pr_panel.get_check_state(repo, pr_number).cloned();
                    self.diff_panel
                        .show_pr_summary(repo, pr_number, &result, &self.pr_panel.overlap_map, pr_data, checks.as_ref());
                }
            }
            _ => {}
        }
    }

    fn navigate_to_file(&mut self, repo: &str, pr_number: u64, file_path: &str) {
        // Select the file node in the tree (open parent nodes too)
        let repo_id = NodeId::Repo(repo.to_string());
        let pr_id = NodeId::Pr(repo.to_string(), pr_number);
        let file_id = NodeId::File(repo.to_string(), pr_number, file_path.to_string());
        self.pr_panel.tree_state.open(vec![repo_id.clone()]);
        self.pr_panel.tree_state.open(vec![repo_id.clone(), pr_id.clone()]);
        self.pr_panel.tree_state.select(vec![repo_id, pr_id, file_id]);

        // Show the file diff
        if let Some(result) = self.pr_panel.get_analysis(repo, pr_number) {
            let file_ctx = self.build_file_context(repo, pr_number);
            let cached = self.lookup_cached_content(repo, pr_number, file_path);
            self.diff_panel.show_file(repo, pr_number, file_path, &result, file_ctx.as_ref(), cached, &self.pr_panel.overlap_map);
        }

        // Rebuild search matches for new content
        if self.search_query.is_some() {
            self.rebuild_search_matches();
        }
    }

    pub fn handle_mouse_click(&mut self, col: u16, row: u16) -> Action {
        if self.show_help {
            self.show_help = false;
            return Action::Noop;
        }
        if self.left_area.contains((col, row).into()) {
            self.focus = Focus::PrList;
            self.pr_panel.focused = true;
            self.diff_panel.focused = false;
            Action::TreeClick(col, row)
        } else if self.right_area.contains((col, row).into()) {
            self.focus = Focus::DiffView;
            self.pr_panel.focused = false;
            self.diff_panel.focused = true;
            Action::DiffMouseDown(col, row)
        } else {
            Action::Noop
        }
    }

    fn handle_mouse_down(&mut self, col: u16, row: u16) {
        if let Some(line_idx) = self.diff_panel.screen_to_line_idx(col, row) {
            // Check if this is a gap line first
            let is_gap = self.diff_panel.line_map.get(line_idx)
                .map(|e| e.is_none())
                .unwrap_or(true);

            if is_gap {
                // Forward to gap/summary click handler
                let result = self.diff_panel.update(&Action::DiffClick(col, row));
                if let Action::NavigateToFile(repo, pr, path) = result {
                    self.navigate_to_file(&repo, pr, &path);
                }
                return;
            }

            self.review.drag = DragState::Pressed {
                rendered_line: line_idx,
                col,
                row,
            };
        }
    }

    pub fn handle_mouse_up_event(&mut self, col: u16, row: u16) -> Action {
        if !self.right_area.contains((col, row).into()) {
            self.review.drag = DragState::Idle;
            return Action::Noop;
        }
        Action::DiffMouseUp(col, row)
    }

    fn handle_mouse_up(&mut self, _col: u16, _row: u16) -> Action {
        let drag = std::mem::replace(&mut self.review.drag, DragState::Idle);

        match drag {
            DragState::Pressed { rendered_line, .. } => {
                // Single click — open editor for that line
                self.diff_panel.cursor_line = Some(rendered_line);
                self.open_inline_editor(rendered_line, None);
            }
            DragState::Dragging { start_rendered_line, current_rendered_line } => {
                // Drag complete — open editor for range
                let lo = start_rendered_line.min(current_rendered_line);
                let hi = start_rendered_line.max(current_rendered_line);
                self.diff_panel.cursor_line = Some(hi);
                self.open_inline_editor(hi, Some(lo));
            }
            DragState::Idle => {}
        }

        Action::Noop
    }

    fn handle_mouse_drag(&mut self, col: u16, row: u16) {
        if let Some(line_idx) = self.diff_panel.screen_to_line_idx(col, row) {
            match self.review.drag {
                DragState::Pressed { rendered_line, .. } => {
                    if line_idx != rendered_line {
                        self.review.drag = DragState::Dragging {
                            start_rendered_line: rendered_line,
                            current_rendered_line: line_idx,
                        };
                    }
                }
                DragState::Dragging { start_rendered_line, .. } => {
                    self.review.drag = DragState::Dragging {
                        start_rendered_line,
                        current_rendered_line: line_idx,
                    };
                }
                _ => {}
            }
        }
    }

    fn open_inline_editor(&mut self, rendered_line: usize, range_start: Option<usize>) {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f.to_string(),
            None => return,
        };

        // Get line info from line_map
        let info = match self.diff_panel.line_map.get(rendered_line) {
            Some(Some(info)) if info.commentable => info.clone(),
            _ => return,
        };

        // Check for range start info
        let start_line_info = range_start.and_then(|start_idx| {
            self.diff_panel.line_map.get(start_idx)?.as_ref().map(|i| i.file_line)
        });

        // Check if there's already a comment at this position
        if let Some(idx) = self.review.find_comment_at(&file_path, info.file_line, info.side) {
            let comment = self.review.comments[idx].clone();
            self.review.inline_editor = Some(InlineEditor::for_existing(rendered_line, &comment, idx));
        } else {
            self.review.inline_editor = Some(InlineEditor::new(
                rendered_line,
                file_path,
                info.file_line,
                start_line_info,
                info.side,
            ));
        }
    }

    fn cursor_has_thread(&self, rendered_line: usize) -> bool {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f,
            None => return false,
        };
        let info = match self.diff_panel.line_map.get(rendered_line) {
            Some(Some(info)) => info,
            _ => return false,
        };
        let threads = match self.diff_panel.current_context()
            .and_then(|(repo, pr)| self.comments_cache.get(&(repo.clone(), *pr)))
        {
            Some(c) => &c.threads,
            None => return false,
        };
        threads.iter().any(|t| t.path == file_path && t.line == info.file_line && t.diff_side == info.side)
    }

    fn cursor_has_pending(&self, rendered_line: usize) -> bool {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f,
            None => return false,
        };
        let info = match self.diff_panel.line_map.get(rendered_line) {
            Some(Some(info)) => info,
            _ => return false,
        };
        self.review.find_comment_at(file_path, info.file_line, info.side).is_some()
    }

    fn cursor_thread_expanded(&self, rendered_line: usize) -> bool {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f,
            None => return false,
        };
        let info = match self.diff_panel.line_map.get(rendered_line) {
            Some(Some(info)) => info,
            _ => return false,
        };
        let key = (file_path.to_string(), info.file_line, info.side);
        self.diff_panel.expanded_threads.contains(&key)
    }

    fn open_reply_editor(&mut self, rendered_line: usize) {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f.to_string(),
            None => return,
        };
        let info = match self.diff_panel.line_map.get(rendered_line) {
            Some(Some(info)) if info.commentable => info.clone(),
            _ => return,
        };

        // Find the thread at this position to get its first_comment_id
        let first_comment_id = self.diff_panel.current_context()
            .and_then(|(repo, pr)| self.comments_cache.get(&(repo.clone(), *pr)))
            .and_then(|comments| {
                comments.threads.iter()
                    .find(|t| t.path == file_path && t.line == info.file_line && t.diff_side == info.side)
                    .and_then(|t| t.first_comment_id())
            });

        let first_comment_id = match first_comment_id {
            Some(id) => id,
            None => return,
        };

        let mut editor = InlineEditor::new(
            rendered_line,
            file_path,
            info.file_line,
            None,
            info.side,
        );
        editor.reply_to_comment_id = Some(first_comment_id);
        self.review.inline_editor = Some(editor);
    }

    fn toggle_thread_expansion(&mut self, rendered_line: usize) {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f.to_string(),
            None => return,
        };
        let info = match self.diff_panel.line_map.get(rendered_line) {
            Some(Some(info)) => info.clone(),
            _ => return,
        };
        let key = (file_path, info.file_line, info.side);
        if !self.diff_panel.expanded_threads.remove(&key) {
            self.diff_panel.expanded_threads.insert(key);
        }
    }

    fn save_comment(&mut self, body: String) {
        let editor = match self.review.inline_editor.take() {
            Some(e) => e,
            None => return,
        };

        if let Some(idx) = editor.editing_index {
            if body.trim().is_empty() {
                // Empty save on existing comment = delete it
                if idx < self.review.comments.len() {
                    self.review.comments.remove(idx);
                }
            } else if idx < self.review.comments.len() {
                self.review.comments[idx].body = body;
            }
        } else {
            // Add new comment
            self.review.comments.push(PendingComment {
                file_path: editor.target_file_path,
                line: editor.target_line,
                start_line: editor.target_start_line,
                side: editor.target_side,
                body,
                reply_to_comment_id: editor.reply_to_comment_id,
            });
        }

        self.status_bar.review_count = self.review.comments.len();
        self.sync_comment_count();
    }

    fn sync_comment_count(&mut self) {
        if let (Some(repo), Some(pr)) = (&self.review.repo, self.review.pr_number) {
            self.pr_panel.set_comment_counts(repo, pr, &self.review.comments);
        }
    }

    fn post_issue_comment(&self, body: String) {
        let repo = match &self.review.repo {
            Some(r) => r.clone(),
            None => return,
        };
        let pr_number = match self.review.pr_number {
            Some(n) => n,
            None => return,
        };
        let tx = self.action_tx.clone();
        let github_client = self.github_client.clone();
        tokio::spawn(async move {
            match github_client.post_issue_comment(&repo, pr_number, &body).await {
                Ok(()) => {
                    let _ = tx.send(Action::IssueCommentPosted);
                }
                Err(e) => {
                    let _ = tx.send(Action::IssueCommentError(format!("Comment failed: {e}")));
                }
            }
        });
    }

    fn submit_review(&self, event: ReviewEvent, body: String) {
        let repo = match &self.review.repo {
            Some(r) => r.clone(),
            None => return,
        };
        let pr_number = match self.review.pr_number {
            Some(n) => n,
            None => return,
        };
        let head_sha = match &self.review.head_sha {
            Some(s) => s.clone(),
            None => return,
        };

        // Split into replies (to existing threads) and new comments
        let mut replies: Vec<(u64, String)> = Vec::new();
        let mut new_comments: Vec<inspect_core::github::ReviewCommentInput> = Vec::new();

        for c in &self.review.comments {
            if let Some(comment_id) = c.reply_to_comment_id {
                replies.push((comment_id, c.body.clone()));
            } else {
                new_comments.push(inspect_core::github::ReviewCommentInput {
                    path: c.file_path.clone(),
                    line: c.line as u64,
                    body: c.body.clone(),
                    start_line: c.start_line.map(|sl| sl as u64),
                });
            }
        }

        let review = inspect_core::github::CreateReview {
            commit_id: head_sha,
            event: event.as_str().to_string(),
            body,
            comments: new_comments,
        };

        let tx = self.action_tx.clone();
        let github_client = self.github_client.clone();
        tokio::spawn(async move {
            // Submit replies individually via REST API
            for (comment_id, body) in &replies {
                if let Err(e) = github_client.reply_to_comment(&repo, pr_number, *comment_id, body).await {
                    let _ = tx.send(Action::ReviewError(format!("Reply failed: {e}")));
                    return;
                }
            }

            // Submit the review with new comments (or just the event if no new comments)
            let client = match inspect_core::github::GitHubClient::new() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Action::ReviewError(format!("GitHub client error: {e}")));
                    return;
                }
            };
            match client.create_review(&repo, pr_number, &review).await {
                Ok(resp) => {
                    let _ = tx.send(Action::ReviewSubmitted(resp.html_url));
                    // Refresh comments to show new replies
                    match github_client.get_pr_comments(&repo, pr_number).await {
                        Ok(comments) => {
                            let _ = tx.send(Action::CommentsLoaded(
                                repo,
                                pr_number,
                                Box::new(comments),
                            ));
                        }
                        Err(_) => {}
                    }
                }
                Err(e) => {
                    let _ = tx.send(Action::ReviewError(format!("Review submit failed: {e}")));
                }
            }
        });
    }

    fn prepare_editor_launch(&self) -> Action {
        let file_path = match self.diff_panel.current_file() {
            Some(f) => f.to_string(),
            None => return Action::Noop,
        };

        let (repo, pr_number) = match self.diff_panel.current_context() {
            Some(ctx) => ctx.clone(),
            None => return Action::Noop,
        };

        // Get the HEAD version of the file
        let file_ctx = match self.build_file_context(&repo, pr_number) {
            Some(ctx) => ctx,
            None => return Action::Noop,
        };

        let local = match &file_ctx.local_path {
            Some(p) => p,
            None => return Action::Noop,
        };

        let content = match git_show_file(local, &file_ctx.head_ref, &file_path) {
            Some(c) => c,
            None => return Action::Noop,
        };

        // Write to temp file (preserve extension for editor syntax highlighting)
        let ext = std::path::Path::new(&file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let temp_path = format!("/tmp/prfait-edit-{}{}", std::process::id(), ext);
        if std::fs::write(&temp_path, &content).is_err() {
            return Action::Noop;
        }

        // Determine line number from current scroll position
        let line_number = self.diff_panel.line_map
            .iter()
            .filter_map(|e| e.as_ref())
            .next()
            .map(|info| info.file_line)
            .unwrap_or(1);

        Action::SuspendForEditor(temp_path, line_number, content)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // Clear the entire screen to prevent stale content from bleeding through
        frame.render_widget(Clear, area);

        let [main_area, status_area] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(area);

        // Look up comments for the currently-viewed PR from the cache,
        // keyed by diff_panel.current_context, to avoid stale data races.
        let empty_comments = crate::review::PrComments {
            threads: Vec::new(),
            comments: Vec::new(),
        };
        let current_comments = self
            .diff_panel
            .current_context()
            .and_then(|(repo, pr)| self.comments_cache.get(&(repo.clone(), *pr)))
            .unwrap_or(&empty_comments);

        if self.show_sidebar {
            let [left_area, right_area] =
                Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .areas(main_area);

            self.left_area = left_area;
            self.right_area = right_area;

            let inner_width = right_area.width.saturating_sub(2);
            self.diff_panel.inject_review_ui(
                self.review.inline_editor.as_ref(),
                &self.review.comments,
                &self.review.drag,
                inner_width,
                &current_comments.threads,
                &current_comments.comments,
            );

            self.pr_panel.render(frame, left_area);
            self.diff_panel.render(frame, right_area);
        } else {
            self.left_area = Rect::default();
            self.right_area = main_area;

            let inner_width = main_area.width.saturating_sub(2);
            self.diff_panel.inject_review_ui(
                self.review.inline_editor.as_ref(),
                &self.review.comments,
                &self.review.drag,
                inner_width,
                &current_comments.threads,
                &current_comments.comments,
            );

            self.diff_panel.render(frame, main_area);
        }

        if self.show_help {
            render_help_overlay(frame, main_area);
        }

        if let Some(editor) = &self.review.body_editor {
            render_body_editor_overlay(frame, main_area, editor);
        }

        // Render search input or esc-pending hint over status bar, or normal status bar
        if let Some(ref input) = self.search_input {
            let line = Line::from(vec![
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(input.clone()),
                Span::styled("_", Style::default().bg(Color::Rgb(100, 100, 160))),
            ]);
            let bar = Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 40)));
            frame.render_widget(bar, status_area);
        } else if self.esc_pending {
            let line = Line::from(vec![
                Span::styled(
                    " Press Esc again to quit",
                    Style::default().fg(Color::Yellow),
                ),
            ]);
            let bar = Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 40)));
            frame.render_widget(bar, status_area);
        } else if self.search_query.is_some() {
            let match_info = if self.search_matches.is_empty() {
                "no matches".to_string()
            } else {
                let idx = self.search_match_idx.map(|i| i + 1).unwrap_or(0);
                format!("{}/{}", idx, self.search_matches.len())
            };
            let query = self.search_query.as_deref().unwrap_or("");
            let line = Line::from(vec![
                Span::styled(
                    format!(" /{query}"),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("  [{match_info}]"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled("n", Style::default().fg(Color::Cyan)),
                Span::raw(":next "),
                Span::styled("N", Style::default().fg(Color::Cyan)),
                Span::raw(":prev "),
                Span::styled("Esc", Style::default().fg(Color::Cyan)),
                Span::raw(":clear"),
            ]);
            let bar = Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 40)));
            frame.render_widget(bar, status_area);
        } else {
            self.status_bar.render(frame, status_area);
        }
    }

    pub fn start_loading_prs(&mut self) {
        for repo in &self.config.repos {
            self.pr_panel.set_loading(&repo.name);
            let client = self.github_client.clone();
            let repo_name = repo.name.clone();
            let tx = self.action_tx.clone();

            tokio::spawn(async move {
                match client.list_open_prs(&repo_name).await {
                    Ok(prs) => {
                        let _ = tx.send(Action::PrsLoaded(repo_name, prs));
                    }
                    Err(e) => {
                        let _ = tx.send(Action::LoadError(format!("{repo_name}: {e}")));
                    }
                }
            });
        }
    }

    fn build_file_context(&self, repo: &str, pr_number: u64) -> Option<FileContext> {
        let repo_config = self.config.repos.iter().find(|r| r.name == repo)?;
        let local_path = repo_config.local_path.clone()?;
        let pr = self
            .pr_cache
            .get(repo)?
            .iter()
            .find(|p| p.number == pr_number)?;

        let merge_base = std::process::Command::new("git")
            .args([
                "merge-base",
                &format!("origin/{}", pr.base_ref),
                &format!("origin/{}", pr.head_ref),
            ])
            .current_dir(&local_path)
            .output()
            .ok()?;

        if !merge_base.status.success() {
            return None;
        }

        let base_ref = String::from_utf8_lossy(&merge_base.stdout)
            .trim()
            .to_string();

        Some(FileContext {
            local_path: Some(local_path),
            base_ref,
            head_ref: format!("origin/{}", pr.head_ref),
        })
    }

    /// Look up cached file content (non-blocking try_lock).
    fn lookup_cached_content(&self, repo: &str, pr_number: u64, path: &str) -> Option<(String, String)> {
        let cache = self.file_content_cache.try_lock().ok()?;
        cache.get(&(repo.to_string(), pr_number, path.to_string())).cloned()
    }

    /// Prefetch git file content for all files in a PR's analysis result.
    fn prefetch_file_content(&self, repo: &str, pr_number: u64, result: &inspect_core::types::ReviewResult) {
        let file_ctx = match self.build_file_context(repo, pr_number) {
            Some(ctx) => ctx,
            None => return,
        };
        let local_path = match file_ctx.local_path {
            Some(p) => p,
            None => return,
        };
        let base_ref = file_ctx.base_ref;
        let head_ref = file_ctx.head_ref;

        // Collect unique file paths
        let mut file_paths: Vec<String> = result.entity_reviews.iter()
            .map(|e| e.file_path.clone())
            .collect();
        file_paths.sort();
        file_paths.dedup();

        let cache = self.file_content_cache.clone();
        let repo = repo.to_string();

        for file_path in file_paths {
            let cache = cache.clone();
            let repo = repo.clone();
            let local_path = local_path.clone();
            let base_ref = base_ref.clone();
            let head_ref = head_ref.clone();

            tokio::spawn(async move {
                let key = (repo, pr_number, file_path.clone());

                // Skip if already cached
                {
                    let c = cache.lock().await;
                    if c.contains_key(&key) {
                        return;
                    }
                }

                // Fetch in blocking thread
                let result = tokio::task::spawn_blocking(move || {
                    let before = git_show_file(&local_path, &base_ref, &file_path)
                        .unwrap_or_default();
                    let after = git_show_file(&local_path, &head_ref, &file_path)
                        .unwrap_or_default();
                    (before, after)
                }).await;

                if let Ok(content) = result {
                    cache.lock().await.insert(key, content);
                }
            });
        }
    }

    /// If the diff panel is currently showing this PR's summary, re-render it
    /// so updated check state appears.
    fn refresh_summary_if_viewing(&mut self, repo: &str, pr_number: u64) {
        if let Some(NodeId::Pr(ref r, pn)) = self.pr_panel.selected_node().cloned() {
            if r == repo && pn == pr_number {
                if let Some(result) = self.pr_panel.get_analysis(repo, pr_number) {
                    let pr_data = self.pr_panel.get_pr(repo, pr_number);
                    let checks = self.pr_panel.get_check_state(repo, pr_number).cloned();
                    self.diff_panel.show_pr_summary(
                        repo,
                        pr_number,
                        &result,
                        &self.pr_panel.overlap_map,
                        pr_data,
                        checks.as_ref(),
                    );
                }
            }
        }
    }

    fn maybe_trigger_checks(&mut self, repo: &str, pr_number: u64) {
        let pr = self
            .pr_cache
            .get(repo)
            .and_then(|prs| prs.iter().find(|p| p.number == pr_number));
        let pr = match pr {
            Some(p) => p.clone(),
            None => return,
        };

        if !self.checks.needs_check(repo, pr_number, &pr.head_sha) {
            return;
        }

        let repo_config = match self.config.repos.iter().find(|r| r.name == repo) {
            Some(r) => r.clone(),
            None => return,
        };

        self.checks.mark_in_flight(repo, pr_number);

        let tx = self.action_tx.clone();
        let client = self.github_client.clone();
        let sha = pr.head_sha.clone();
        let head_ref = pr.head_ref.clone();

        tokio::spawn(async move {
            crate::checks::trigger_checks(tx, client, repo_config, pr_number, sha, head_ref).await;
        });
    }

    fn trigger_analysis(&self, repo: &str, pr_number: u64) {
        let pr = self
            .pr_cache
            .get(repo)
            .and_then(|prs| prs.iter().find(|p| p.number == pr_number));

        let pr = match pr {
            Some(p) => p.clone(),
            None => return,
        };

        let repo_config = match self.config.repos.iter().find(|r| r.name == repo) {
            Some(r) => r.clone(),
            None => return,
        };

        let repo_name = repo.to_string();
        let cache = self.analysis.cache.clone();
        let in_progress = self.analysis.in_progress.clone();
        let action_tx = self.action_tx.clone();
        let dampening_rules = self.config.effective_dampening();
        let github_client = self.github_client.clone();

        tokio::spawn(async move {
            let key = (repo_name.clone(), pr_number);

            {
                let in_prog = in_progress.lock().await;
                if in_prog.contains(&key) {
                    return;
                }
            }

            let have_cached = {
                let c = cache.lock().await;
                if let Some(result) = c.get(&key) {
                    let _ = action_tx.send(Action::AnalysisComplete(
                        repo_name.clone(),
                        pr_number,
                        Box::new(result.clone()),
                    ));
                    true
                } else {
                    false
                }
            };

            if have_cached && repo_config.local_path.is_none() {
                return;
            }

            in_progress.lock().await.insert(key.clone());

            // Helper: run remote analysis using the merge-base (not branch tip)
            // so we only see changes introduced by the PR, matching GitHub's diff.
            let run_remote = |repo_name: String, pr: crate::github::PrData, action_tx: mpsc::UnboundedSender<Action>, github_client: Arc<GithubClient>| async move {
                // Fetch merge-base so we compare the right commits
                let base_ref = match github_client.get_merge_base(&repo_name, &pr.base_ref, &pr.head_sha).await {
                    Ok(mb) => mb,
                    Err(e) => {
                        let _ = action_tx.send(Action::LoadError(format!(
                            "{repo_name}#{}: merge-base fetch failed, using branch tip: {e}", pr.number
                        )));
                        pr.base_ref.clone()
                    }
                };
                crate::analysis::analyze_remote_standalone(
                    &repo_name,
                    pr.number,
                    &base_ref,
                    &pr.head_sha,
                    &pr.files,
                )
                .await
            };

            let result = if let Some(ref local) = repo_config.local_path {
                let local_result = crate::analysis::analyze_local_standalone(local, &pr.base_ref, &pr.head_ref).await;
                match local_result {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        // Local analysis failed (e.g. branch not fetchable) — fall back to remote
                        let _ = action_tx.send(Action::LoadError(format!(
                            "{repo_name}#{pr_number}: local analysis failed, trying remote: {e}"
                        )));
                        run_remote(repo_name.clone(), pr, action_tx.clone(), github_client).await
                    }
                }
            } else {
                run_remote(repo_name.clone(), pr, action_tx.clone(), github_client).await
            };

            in_progress.lock().await.remove(&key);

            match result {
                Ok(mut review) => {
                    crate::config::apply_score_dampening(&mut review, &dampening_rules);
                    cache.lock().await.insert(key, review.clone());
                    crate::analysis::save_to_disk_cache(&repo_name, pr_number, &review);
                    let _ = action_tx.send(Action::AnalysisComplete(
                        repo_name,
                        pr_number,
                        Box::new(review),
                    ));
                }
                Err(e) => {
                    let _ = action_tx.send(Action::LoadError(format!(
                        "Analysis failed for {repo_name}#{pr_number}: {e}"
                    )));
                }
            }
        });
    }

    fn fetch_pr_comments(&self, repo: &str, pr_number: u64) {
        let client = self.github_client.clone();
        let repo_name = repo.to_string();
        let tx = self.action_tx.clone();

        tokio::spawn(async move {
            match client.get_pr_comments(&repo_name, pr_number).await {
                Ok(comments) => {
                    let _ = tx.send(Action::CommentsLoaded(
                        repo_name,
                        pr_number,
                        Box::new(comments),
                    ));
                }
                Err(_) => {
                    // Silently ignore — comments are non-critical
                }
            }
        });
    }
}

fn git_show_file(
    repo_path: &std::path::Path,
    git_ref: &str,
    file_path: &str,
) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["show", &format!("{git_ref}:{file_path}")])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let width = 62.min(area.width);
    let height = 22.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    let key_style = Style::default().fg(Color::Cyan);
    let desc_style = Style::default().fg(Color::Rgb(200, 200, 200));
    let section_style = Style::default().fg(Color::Rgb(220, 220, 220));
    let dim = Style::default().fg(Color::Rgb(100, 100, 100));

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" Navigation", section_style),
            Span::raw("                     "),
            Span::styled("Review", section_style),
        ]),
        Line::from(vec![
            Span::styled(" ──────────", dim),
            Span::raw("                     "),
            Span::styled("──────", dim),
        ]),
        Line::from(vec![
            Span::styled(" j/k, arrows   ", key_style),
            Span::styled("Navigate    ", desc_style),
            Span::raw("    "),
            Span::styled("Enter/c   ", key_style),
            Span::styled("Comment on line", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" J/K (shift)    ", key_style),
            Span::styled("Half page   ", desc_style),
            Span::raw("    "),
            Span::styled("Alt+Enter ", key_style),
            Span::styled("Save comment", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" { / }          ", key_style),
            Span::styled("Prev/next hk", desc_style),
            Span::raw("    "),
            Span::styled("Esc       ", key_style),
            Span::styled("Cancel comment", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" Tab            ", key_style),
            Span::styled("Switch panel", desc_style),
            Span::raw("    "),
            Span::styled("Ctrl+R    ", key_style),
            Span::styled("Submit review", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" h/l            ", key_style),
            Span::styled("Scroll horiz", desc_style),
            Span::raw("    "),
            Span::styled("e         ", key_style),
            Span::styled("Edit in $EDITOR", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" g / G          ", key_style),
            Span::styled("Top / bottom", desc_style),
            Span::raw("    "),
            Span::styled("/         ", key_style),
            Span::styled("Search in diff", desc_style),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Diff View", section_style),
            Span::raw("                      "),
            Span::styled("General", section_style),
        ]),
        Line::from(vec![
            Span::styled(" ─────────", dim),
            Span::raw("                      "),
            Span::styled("───────", dim),
        ]),
        Line::from(vec![
            Span::styled(" d              ", key_style),
            Span::styled("Toggle mode ", desc_style),
            Span::raw("    "),
            Span::styled("x         ", key_style),
            Span::styled("Mark reviewed", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" click          ", key_style),
            Span::styled("Expand gap  ", desc_style),
            Span::raw("    "),
            Span::styled("r         ", key_style),
            Span::styled("Refresh PRs", desc_style),
        ]),
        Line::from(vec![
            Span::styled(" drag           ", key_style),
            Span::styled("Select range", desc_style),
            Span::raw("    "),
            Span::styled("q         ", key_style),
            Span::styled("Quit", desc_style),
        ]),
        Line::from(vec![
            Span::raw("                               "),
            Span::styled("Esc       ", key_style),
            Span::styled("Back / quit (2x)", desc_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "             Press any key to dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::bordered()
        .title(Span::styled(" Help ", Style::default().fg(Color::Cyan)))
        .border_style(Style::default().fg(Color::Rgb(80, 80, 120)));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, overlay);
}

fn render_body_editor_overlay(frame: &mut Frame, area: Rect, editor: &BodyEditor) {
    let width = 70.min(area.width);
    let height = 16.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    let title = match &editor.purpose {
        BodyEditorPurpose::ReviewBody(ReviewEvent::RequestChanges) => " Request Changes ",
        BodyEditorPurpose::ReviewBody(ReviewEvent::Comment) => " Review Comment ",
        BodyEditorPurpose::ReviewBody(ReviewEvent::Approve) => " Approve ",
        BodyEditorPurpose::IssueComment => " PR Comment ",
    };

    let block = Block::bordered()
        .title(Span::styled(title, Style::default().fg(Color::Yellow)))
        .border_style(Style::default().fg(Color::Rgb(80, 80, 120)));

    let inner = block.inner(overlay);

    // Reserve last line for hints
    let editor_height = inner.height.saturating_sub(1) as usize;
    let hint_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
    let text_area = Rect::new(inner.x, inner.y, inner.width, editor_height as u16);

    // Render the editor text
    let mut lines: Vec<Line> = Vec::new();
    let visible_width = text_area.width as usize;
    for (i, line_text) in editor.lines.iter().enumerate().take(editor_height) {
        if i == editor.cursor.0 {
            // Show cursor
            let col = editor.cursor.1;
            let before = &line_text[..col.min(line_text.len())];
            let cursor_char = line_text.get(col..col + 1).unwrap_or(" ");
            let after = if col + 1 < line_text.len() {
                &line_text[col + 1..]
            } else {
                ""
            };
            lines.push(Line::from(vec![
                Span::raw(truncate_str(before, visible_width)),
                Span::styled(
                    cursor_char.to_string(),
                    Style::default().bg(Color::Rgb(100, 100, 160)).fg(Color::White),
                ),
                Span::raw(after.to_string()),
            ]));
        } else {
            lines.push(Line::from(truncate_str(line_text, visible_width)));
        }
    }

    // Pad remaining lines
    while lines.len() < editor_height {
        lines.push(Line::from(""));
    }

    let hint = Line::from(vec![
        Span::styled("Alt+Enter", Style::default().fg(Color::Cyan)),
        Span::raw(":submit  "),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::raw(":cancel"),
    ]);

    frame.render_widget(block, overlay);
    frame.render_widget(Paragraph::new(lines), text_area);
    frame.render_widget(Paragraph::new(vec![hint]), hint_area);
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[..max].to_string()
    }
}
