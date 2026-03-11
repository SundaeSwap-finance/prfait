use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;
use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use sem_core::model::change::ChangeType;

use crate::action::Action;
use crate::components::Component;
use crate::github::PrData;
use crate::review::{DiffLineInfo, DiffSide, DragState, InlineEditor, LineMap, PendingComment, PrComment, ReviewThread};
use crate::structural_diff::{self, Block as DiffBlock};
use crate::components::pr_panel::{OverlapMap, compute_entity_overlaps, sort_entities_by_risk};
use inspect_core::types::{EntityReview, ReviewResult, RiskLevel};

thread_local! {
    static SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

/// Syntax-highlight a line of code, returning styled spans with the given background.
fn highlight_line_spans(file_path: &str, line_text: &str, bg: Color, hl: &mut Option<HighlightLines<'static>>) -> Vec<Span<'static>> {
    SYNTAX_SET.with(|ss| {
        THEME_SET.with(|ts| {
            let ext = Path::new(file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("txt");
            let syntax = ss
                .find_syntax_by_extension(ext)
                .unwrap_or_else(|| ss.find_syntax_plain_text());
            let theme = &ts.themes["base16-ocean.dark"];

            // SAFETY: ss and ts are thread-local statics that live for the thread's lifetime.
            // HighlightLines borrows from them, but we can't express that with thread_local!
            // since the closure doesn't return a borrow. We transmute to 'static to make
            // HighlightLines storable across calls, which is safe because the thread_local
            // values are never dropped while the thread is alive.
            let syntax: &'static syntect::parsing::SyntaxReference = unsafe { std::mem::transmute(syntax) };
            let theme: &'static syntect::highlighting::Theme = unsafe { std::mem::transmute(theme) };
            let ss: &'static SyntaxSet = unsafe { std::mem::transmute(ss) };

            let h = hl.get_or_insert_with(|| HighlightLines::new(syntax, theme));
            let highlighted = h.highlight_line(line_text, ss).unwrap_or_default();

            highlighted
                .into_iter()
                .map(|(style, text)| {
                    Span::styled(
                        text.to_string(),
                        Style::default()
                            .fg(Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b))
                            .bg(bg),
                    )
                })
                .collect()
        })
    })
}

/// Identifies a collapsible gap region.
/// - `entity_index`: which entity (by position in the sorted list)
/// - `gap_index`: sequential counter of gaps within that entity
///   - For structural diffs: each Unchanged block with >2 stmts
///   - For inline diffs: any gap means expand the whole entity to full context
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GapId {
    entity_index: usize,
    gap_index: usize,
}

/// Target for clicking an overlap flag ("also in #N") to navigate to that PR's file.
#[derive(Debug, Clone)]
struct OverlapClickTarget {
    repo: String,
    pr_number: u64,
    file_path: String,
}

/// Context needed to fetch full file content for structural diffs
pub struct FileContext {
    pub local_path: Option<std::path::PathBuf>,
    pub base_ref: String,
    pub head_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    Unified,
    SideBySide,
}

/// What the panel is currently showing, so we can re-render on mode/width change
#[derive(Clone)]
enum PanelContent {
    Empty,
    PrSummary {
        repo: String,
        pr_number: u64,
        result: ReviewResult,
        /// entity_name → other PRs that also touch it
        entity_overlaps: HashMap<String, Vec<(String, u64)>>,
        /// PR description body (may be empty)
        pr_body: String,
        /// GitHub URL for the PR
        pr_html_url: String,
    },
    FileDiff {
        entities: Vec<EntityReview>,
        file_path: String,
        full_files: Option<(String, String)>,
        /// entity_name → other PRs that also touch it
        entity_overlaps: HashMap<String, Vec<(String, u64)>>,
    },
}

pub struct DiffPanel {
    pub focused: bool,
    pub diff_mode: DiffMode,
    current_file: Option<String>,
    current_context: Option<(String, u64)>,
    content: PanelContent,
    lines: Vec<Line<'static>>,
    /// Vertical scroll
    scroll_y: u16,
    /// Horizontal scroll
    scroll_x: u16,
    total_lines: u16,
    /// Max line width (for horizontal scroll bounds)
    max_width: u16,
    /// Last rendered inner width (to re-render side-by-side on resize)
    last_inner_width: u16,
    /// Which collapsed gaps are currently expanded
    expanded: HashSet<GapId>,
    /// Maps rendered line index → clickable GapId
    gap_map: Vec<(usize, GapId)>,
    /// Maps rendered line index → diff line info (for review comments)
    pub line_map: LineMap,
    /// Maps rendered line index → file path (for clickable PR summary entities)
    summary_click_map: Vec<(usize, String)>,
    /// Maps rendered line index → cross-PR navigation target (for overlap flags)
    overlap_click_map: Vec<(usize, OverlapClickTarget)>,
    base_overlap_click_map: Vec<(usize, OverlapClickTarget)>,
    /// Inner area from last render (for screen→line coordinate conversion)
    pub last_inner_area: Rect,
    /// Cursor position: index into `lines`/`line_map` for keyboard navigation
    pub cursor_line: Option<usize>,
    /// Expanded existing review threads: (file_path, line, side)
    pub expanded_threads: HashSet<(String, usize, DiffSide)>,
    /// Cached base lines from rebuild (before overlay mutations)
    base_lines: Vec<Line<'static>>,
    base_line_map: LineMap,
    base_gap_map: Vec<(usize, GapId)>,
    base_total_lines: u16,
    base_max_width: u16,
    /// Whether base lines need rebuilding
    lines_dirty: bool,
}

impl DiffPanel {
    pub fn new() -> Self {
        Self {
            focused: false,
            diff_mode: DiffMode::Unified,
            current_file: None,
            current_context: None,
            content: PanelContent::Empty,
            lines: Vec::new(),
            scroll_y: 0,
            scroll_x: 0,
            total_lines: 0,
            max_width: 0,
            last_inner_width: 0,
            expanded: HashSet::new(),
            gap_map: Vec::new(),
            line_map: Vec::new(),
            summary_click_map: Vec::new(),
            overlap_click_map: Vec::new(),
            base_overlap_click_map: Vec::new(),
            last_inner_area: Rect::default(),
            cursor_line: None,
            expanded_threads: HashSet::new(),
            base_lines: Vec::new(),
            base_line_map: Vec::new(),
            base_gap_map: Vec::new(),
            base_total_lines: 0,
            base_max_width: 0,
            lines_dirty: true,
        }
    }

    pub fn show_file(
        &mut self,
        repo: &str,
        pr_number: u64,
        path: &str,
        result: &ReviewResult,
        file_ctx: Option<&FileContext>,
        cached_files: Option<(String, String)>,
        overlaps: &OverlapMap,
    ) {
        self.current_file = Some(path.to_string());
        self.current_context = Some((repo.to_string(), pr_number));
        self.scroll_y = 0;
        self.scroll_x = 0;
        self.expanded.clear();

        let mut entities: Vec<EntityReview> = result
            .entity_reviews
            .iter()
            .filter(|e| e.file_path == path)
            .cloned()
            .collect();
        let entity_overlaps = compute_entity_overlaps(&entities, repo, pr_number, overlaps);
        sort_entities_by_risk(&mut entities, &entity_overlaps);

        // Use cached content if available, otherwise fetch synchronously
        let full_files = cached_files.or_else(|| {
            let ctx = file_ctx?;
            let local = ctx.local_path.as_ref()?;
            let before = git_show_file(local, &ctx.base_ref, path)?;
            let after = git_show_file(local, &ctx.head_ref, path)?;
            Some((before, after))
        });

        self.content = PanelContent::FileDiff {
            entities: entities.clone(),
            file_path: path.to_string(),
            full_files: full_files.clone(),
            entity_overlaps,
        };
        self.lines_dirty = true;
        self.rebuild_base_lines();
        self.cursor_line = self.first_commentable();
    }

    pub fn show_pr_summary(&mut self, repo: &str, pr_number: u64, result: &ReviewResult, overlaps: &OverlapMap, pr_data: Option<&PrData>) {
        self.current_file = None;
        self.current_context = Some((repo.to_string(), pr_number));
        self.scroll_y = 0;
        self.scroll_x = 0;
        self.expanded.clear();

        let entity_overlaps = compute_entity_overlaps(&result.entity_reviews, repo, pr_number, overlaps);

        self.content = PanelContent::PrSummary {
            repo: repo.to_string(),
            pr_number,
            result: result.clone(),
            entity_overlaps,
            pr_body: pr_data.map(|d| d.body.clone()).unwrap_or_default(),
            pr_html_url: pr_data.map(|d| d.html_url.clone()).unwrap_or_default(),
        };
        self.lines_dirty = true;
        self.rebuild_base_lines();
        self.cursor_line = None;
    }

    fn rebuild_base_lines(&mut self) {
        if !self.lines_dirty {
            return;
        }
        let mut gap_map = Vec::new();
        let mut line_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        self.summary_click_map.clear();
        let lines = match &self.content {
            PanelContent::Empty => Vec::new(),
            PanelContent::PrSummary {
                repo,
                pr_number,
                result,
                entity_overlaps,
                pr_body,
                pr_html_url,
            } => {
                let mut click_map = Vec::new();
                let lines = render_pr_summary(result, repo, *pr_number, &mut click_map, entity_overlaps, &mut overlap_click_map, pr_body, pr_html_url);
                self.summary_click_map = click_map;
                lines
            }
            PanelContent::FileDiff {
                entities,
                file_path,
                full_files,
                entity_overlaps,
            } => match self.diff_mode {
                DiffMode::Unified => render_entity_diff(entities, file_path, full_files.as_ref(), &self.expanded, &mut gap_map, &mut line_map, entity_overlaps, &mut overlap_click_map),
                DiffMode::SideBySide => {
                    let col_width = if self.last_inner_width > 3 {
                        (self.last_inner_width / 2).saturating_sub(1) as usize
                    } else {
                        40
                    };
                    render_entity_diff_side_by_side(entities, file_path, full_files.as_ref(), col_width, &self.expanded, &mut gap_map, &mut line_map, entity_overlaps, &mut overlap_click_map)
                }
            },
        };
        // Pad line_map to match lines length
        while line_map.len() < lines.len() {
            line_map.push(None);
        }
        let total_lines = lines.len() as u16;
        let max_width = lines.iter().map(|l| line_width(l)).max().unwrap_or(0) as u16;

        self.base_lines = lines;
        self.base_line_map = line_map;
        self.base_gap_map = gap_map;
        self.base_overlap_click_map = overlap_click_map;
        self.base_total_lines = total_lines;
        self.base_max_width = max_width;
        self.lines_dirty = false;

        // Re-validate cursor after rebuild (line count may have changed)
        if let Some(cursor) = self.cursor_line {
            if cursor >= self.base_lines.len() || !matches!(self.base_line_map.get(cursor), Some(Some(info)) if info.commentable) {
                // Cursor is out of range or no longer commentable — find nearest
                let nearest = self.base_line_map.iter().enumerate()
                    .filter(|(_, e)| matches!(e, Some(info) if info.commentable))
                    .min_by_key(|(idx, _)| (*idx as isize - cursor as isize).unsigned_abs())
                    .map(|(idx, _)| idx);
                self.cursor_line = nearest;
            }
        }
    }

    /// Build a display-ready copy of lines with review UI overlays.
    /// Clones from cached base lines and applies overlays on top.
    pub fn inject_review_ui(
        &mut self,
        inline_editor: Option<&InlineEditor>,
        comments: &[PendingComment],
        drag: &DragState,
        inner_width: u16,
        existing_threads: &[ReviewThread],
        pr_comments: &[PrComment],
    ) {
        // Detect width change for side-by-side mode
        if self.diff_mode == DiffMode::SideBySide && inner_width != self.last_inner_width {
            self.lines_dirty = true;
        }
        self.last_inner_width = inner_width;

        // Only rebuild base lines when dirty
        self.rebuild_base_lines();

        // Clone from cached base state
        self.lines = self.base_lines.clone();
        self.line_map = self.base_line_map.clone();
        self.gap_map = self.base_gap_map.clone();
        self.overlap_click_map = self.base_overlap_click_map.clone();
        self.total_lines = self.base_total_lines;
        self.max_width = self.base_max_width;

        let file_path = match &self.current_file {
            Some(f) => f.clone(),
            None => {
                // PR summary view — only inject discussion comments
                if matches!(self.content, PanelContent::PrSummary { .. }) && !pr_comments.is_empty() {
                    let discussion_lines = render_discussion_comments(pr_comments, self.last_inner_width as usize);
                    for line in discussion_lines {
                        self.lines.push(line);
                        self.line_map.push(None);
                    }
                    self.total_lines = self.lines.len() as u16;
                    self.max_width = self.lines.iter().map(|l| line_width(l)).max().unwrap_or(0) as u16;
                }
                return;
            }
        };

        // 1. Apply cursor highlight
        if let Some(cursor) = self.cursor_line {
            if cursor < self.lines.len() {
                let cursor_bg = Color::Rgb(40, 40, 60);
                let line = &self.lines[cursor];
                let new_spans: Vec<Span<'static>> = line.spans.iter().map(|s| {
                    Span::styled(s.content.clone(), s.style.bg(cursor_bg))
                }).collect();
                self.lines[cursor] = Line::from(new_spans);
            }
        }

        // 2. Apply drag highlight
        if let DragState::Dragging { start_rendered_line, current_rendered_line } = drag {
            let lo = (*start_rendered_line).min(*current_rendered_line);
            let hi = (*start_rendered_line).max(*current_rendered_line);
            let highlight_bg = Color::Rgb(50, 50, 80);
            for i in lo..=hi {
                if i < self.lines.len() {
                    let line = &self.lines[i];
                    let new_spans: Vec<Span<'static>> = line.spans.iter().map(|s| {
                        Span::styled(s.content.clone(), s.style.bg(highlight_bg))
                    }).collect();
                    self.lines[i] = Line::from(new_spans);
                }
            }
        }

        // 2. Add pending comment markers
        for comment in comments.iter().filter(|c| c.file_path == file_path) {
            if let Some(rendered_idx) = self.find_rendered_line(comment.line, comment.side) {
                if rendered_idx < self.lines.len() {
                    let marker = Span::styled("● ", Style::default().fg(Color::Yellow));
                    let mut spans = vec![marker];
                    spans.extend(self.lines[rendered_idx].spans.clone());
                    self.lines[rendered_idx] = Line::from(spans);
                }
            }
        }

        // 3. Add existing thread markers
        for thread in existing_threads.iter().filter(|t| t.path == file_path) {
            if let Some(rendered_idx) = self.find_rendered_line(thread.line, thread.diff_side) {
                if rendered_idx < self.lines.len() {
                    let marker_color = if thread.is_resolved {
                        Color::DarkGray
                    } else {
                        Color::Cyan
                    };
                    let count = thread.comments.len();
                    let marker_text = if count > 1 {
                        format!("◆{} ", count)
                    } else {
                        "◆ ".to_string()
                    };
                    let marker = Span::styled(marker_text, Style::default().fg(marker_color));
                    let mut spans = vec![marker];
                    spans.extend(self.lines[rendered_idx].spans.clone());
                    self.lines[rendered_idx] = Line::from(spans);
                }
            }
        }

        // 4. Inject expanded thread comments (in reverse order to keep insertion indices stable)
        let mut thread_insertions: Vec<(usize, Vec<Line<'static>>)> = Vec::new();
        for thread in existing_threads.iter().filter(|t| t.path == file_path) {
            let key = (file_path.clone(), thread.line, thread.diff_side);
            if self.expanded_threads.contains(&key) {
                if let Some(rendered_idx) = self.find_rendered_line(thread.line, thread.diff_side) {
                    let thread_lines = render_thread_comments(thread, self.last_inner_width as usize);
                    thread_insertions.push((rendered_idx + 1, thread_lines));
                }
            }
        }
        // Sort by insertion point descending so earlier insertions don't shift later ones
        thread_insertions.sort_by(|a, b| b.0.cmp(&a.0));
        for (insert_at, thread_lines) in thread_insertions {
            let insert_at = insert_at.min(self.lines.len());
            let count = thread_lines.len();
            for (i, line) in thread_lines.into_iter().enumerate() {
                self.lines.insert(insert_at + i, line);
                self.line_map.insert(insert_at + i, None);
            }
            for (line_idx, _) in self.gap_map.iter_mut() {
                if *line_idx >= insert_at {
                    *line_idx += count;
                }
            }
            if let Some(ref mut cursor) = self.cursor_line {
                if *cursor >= insert_at {
                    *cursor += count;
                }
            }
        }

        // 5. Inject inline editor
        if let Some(editor) = inline_editor {
            if editor.target_file_path != file_path {
                return;
            }
            let insert_at = (editor.anchor_rendered_line + 1).min(self.lines.len());
            let editor_lines = render_inline_editor(editor, self.last_inner_width as usize);
            let count = editor_lines.len();
            for (i, line) in editor_lines.into_iter().enumerate() {
                self.lines.insert(insert_at + i, line);
                self.line_map.insert(insert_at + i, None);
            }
            for (line_idx, _) in self.gap_map.iter_mut() {
                if *line_idx >= insert_at {
                    *line_idx += count;
                }
            }
            if let Some(ref mut cursor) = self.cursor_line {
                if *cursor >= insert_at {
                    *cursor += count;
                }
            }
        }

        // 6. Inject PR discussion comments in summary view
        if matches!(self.content, PanelContent::PrSummary { .. }) && !pr_comments.is_empty() {
            let discussion_lines = render_discussion_comments(pr_comments, self.last_inner_width as usize);
            for line in discussion_lines {
                self.lines.push(line);
                self.line_map.push(None);
            }
        }

        self.total_lines = self.lines.len() as u16;
        self.max_width = self.lines.iter().map(|l| line_width(l)).max().unwrap_or(0) as u16;
    }

    /// Find the rendered line index for a given file line number and side.
    fn find_rendered_line(&self, file_line: usize, side: DiffSide) -> Option<usize> {
        self.line_map.iter().position(|entry| {
            if let Some(info) = entry {
                info.file_line == file_line && info.side == side
            } else {
                false
            }
        })
    }

    pub fn current_file(&self) -> Option<&str> {
        self.current_file.as_deref()
    }

    pub fn current_context(&self) -> Option<&(String, u64)> {
        self.current_context.as_ref()
    }

    /// Resolve a screen coordinate to a rendered line index.
    pub fn screen_to_line_idx(&self, _col: u16, row: u16) -> Option<usize> {
        let inner = self.last_inner_area;
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        if row < inner.y || row >= inner.y + inner.height {
            return None;
        }
        let line_idx = self.scroll_y as usize + (row - inner.y) as usize;
        if line_idx < self.lines.len() {
            Some(line_idx)
        } else {
            None
        }
    }

    /// Find the next commentable line after `from`.
    fn next_commentable(&self, from: usize) -> Option<usize> {
        self.line_map
            .iter()
            .enumerate()
            .skip(from + 1)
            .find(|(_, entry)| matches!(entry, Some(info) if info.commentable))
            .map(|(idx, _)| idx)
    }

    /// Find the previous commentable line before `from`.
    fn prev_commentable(&self, from: usize) -> Option<usize> {
        self.line_map
            .iter()
            .enumerate()
            .take(from)
            .rev()
            .find(|(_, entry)| matches!(entry, Some(info) if info.commentable))
            .map(|(idx, _)| idx)
    }

    /// Find the first commentable line.
    fn first_commentable(&self) -> Option<usize> {
        self.line_map
            .iter()
            .position(|entry| matches!(entry, Some(info) if info.commentable))
    }

    /// Adjust scroll so cursor_line is within the visible viewport.
    fn ensure_cursor_visible(&mut self) {
        if let Some(cursor) = self.cursor_line {
            let viewport_height = self.last_inner_area.height as usize;
            if viewport_height == 0 {
                return;
            }
            let scroll = self.scroll_y as usize;
            if cursor < scroll {
                self.scroll_y = cursor as u16;
            } else if cursor >= scroll + viewport_height {
                self.scroll_y = (cursor - viewport_height + 1) as u16;
            }
        }
    }
}

impl Component for DiffPanel {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::ScrollDown(1),
            KeyCode::Char('k') | KeyCode::Up => Action::ScrollUp(1),
            KeyCode::Char('h') | KeyCode::Left => Action::ScrollLeft(4),
            KeyCode::Char('l') | KeyCode::Right => Action::ScrollRight(4),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Action::ScrollDown(20)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Action::ScrollUp(20)
            }
            KeyCode::Char('g') => Action::ScrollToTop,
            KeyCode::Char('G') => Action::ScrollToBottom,
            KeyCode::Char('0') | KeyCode::Home => Action::ScrollLeft(u16::MAX),
            KeyCode::Char('$') | KeyCode::End => Action::ScrollRight(u16::MAX),
            _ => Action::Noop,
        }
    }

    fn update(&mut self, action: &Action) -> Action {
        match action {
            Action::CursorDown => {
                let next = match self.cursor_line {
                    Some(cur) => self.next_commentable(cur),
                    None => self.first_commentable(),
                };
                if let Some(pos) = next {
                    self.cursor_line = Some(pos);
                    self.ensure_cursor_visible();
                }
            }
            Action::CursorUp => {
                if let Some(cur) = self.cursor_line {
                    if let Some(pos) = self.prev_commentable(cur) {
                        self.cursor_line = Some(pos);
                        self.ensure_cursor_visible();
                    }
                }
            }
            Action::ScrollDown(n) => {
                self.scroll_y = self
                    .scroll_y
                    .saturating_add(*n)
                    .min(self.total_lines.saturating_sub(1));
            }
            Action::ScrollUp(n) => {
                self.scroll_y = self.scroll_y.saturating_sub(*n);
            }
            Action::ScrollLeft(n) => {
                self.scroll_x = self.scroll_x.saturating_sub(*n);
            }
            Action::ScrollRight(n) => {
                self.scroll_x = self.scroll_x.saturating_add(*n).min(self.max_width);
            }
            Action::ScrollToTop => {
                self.scroll_y = 0;
            }
            Action::ScrollToBottom => {
                self.scroll_y = self.total_lines.saturating_sub(1);
            }
            Action::ToggleDiffMode => {
                self.diff_mode = match self.diff_mode {
                    DiffMode::Unified => DiffMode::SideBySide,
                    DiffMode::SideBySide => DiffMode::Unified,
                };
                self.lines_dirty = true;
                self.rebuild_base_lines();
            }
            Action::DiffClick(col, row) => {
                let inner = self.last_inner_area;
                if inner.width > 0 && inner.height > 0
                    && *col >= inner.x && *col < inner.x + inner.width
                    && *row >= inner.y && *row < inner.y + inner.height
                {
                    let line_idx = self.scroll_y as usize + (*row - inner.y) as usize;
                    // Check overlap click map first (cross-PR navigation)
                    if let Some(target) = self.overlap_click_map.iter()
                        .find(|(li, _)| *li == line_idx)
                        .map(|(_, t)| t.clone())
                    {
                        return Action::NavigateToFile(target.repo, target.pr_number, target.file_path);
                    }
                    // Check summary click map (entity → file navigation)
                    if let Some(file_path) = self.summary_click_map.iter()
                        .find(|(li, _)| *li == line_idx)
                        .map(|(_, p)| p.clone())
                    {
                        if let PanelContent::PrSummary { ref repo, pr_number, .. } = self.content {
                            return Action::NavigateToFile(repo.clone(), pr_number, file_path);
                        }
                    }
                    // Then check gap map (expand/collapse)
                    if let Some(gap_id) = self.gap_map.iter().find(|(li, _)| *li == line_idx).map(|(_, g)| *g) {
                        if self.expanded.contains(&gap_id) {
                            self.expanded.remove(&gap_id);
                        } else {
                            self.expanded.insert(gap_id);
                        }
                        self.lines_dirty = true;
                        self.rebuild_base_lines();
                    }
                }
            }
            _ => {}
        }
        Action::Noop
    }

    fn render(&mut self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let file_title = self
            .current_file
            .as_deref()
            .unwrap_or("Select a file to view diff");

        let mode_tag = match self.diff_mode {
            DiffMode::Unified => "",
            DiffMode::SideBySide => " [side-by-side]",
        };

        let block = Block::bordered()
            .title(Span::raw(format!(" {file_title}{mode_tag} ")))
            .border_style(border_style);

        let inner = block.inner(area);
        self.last_inner_area = inner;

        // Only clone the visible slice of lines (viewport clipping)
        let start = self.scroll_y as usize;
        let end = (start + inner.height as usize).min(self.lines.len());
        let visible = if start < self.lines.len() {
            self.lines[start..end].to_vec()
        } else {
            Vec::new()
        };
        let paragraph = Paragraph::new(visible)
            .block(block)
            .scroll((0, self.scroll_x));

        frame.render_widget(paragraph, area);

        // Vertical scrollbar
        if self.total_lines > inner.height {
            let mut scrollbar_state = ScrollbarState::new(self.total_lines as usize)
                .position(self.scroll_y as usize);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
        }

        // Horizontal scroll indicator in bottom-right of block
        if self.scroll_x > 0 {
            let indicator = format!("←{}", self.scroll_x);
            let x = area.right().saturating_sub(indicator.len() as u16 + 2);
            let y = area.bottom().saturating_sub(1);
            if x > area.left() && y > area.top() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        indicator,
                        Style::default().fg(Color::DarkGray),
                    )),
                    Rect::new(x, y, area.right() - x - 1, 1),
                );
            }
        }
    }
}

/// Compute the display width of a Line (sum of span content lengths)
fn line_width(line: &Line) -> usize {
    line.spans.iter().map(|s| s.content.len()).sum()
}

/// Get file content at a git ref. Uses merge-base ref for "before" and origin/branch for "after".
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

const CONTEXT_LINES: usize = 3;

// Explicit RGB colors to avoid terminal theme remapping
const COL_RED: Color = Color::Rgb(220, 80, 80);
const COL_GREEN: Color = Color::Rgb(80, 200, 80);
const COL_CONTEXT: Color = Color::Rgb(120, 120, 120);
const COL_HUNK: Color = Color::Rgb(80, 180, 220);
const COL_HL_DEL_FG: Color = Color::Rgb(255, 255, 255);
const COL_HL_DEL_BG: Color = Color::Rgb(140, 30, 30);
const COL_HL_INS_FG: Color = Color::Rgb(255, 255, 255);
const COL_HL_INS_BG: Color = Color::Rgb(20, 110, 20);
const COL_GAP: Color = Color::Rgb(160, 120, 200);

fn render_entity_diff(
    entities: &[EntityReview],
    file_path: &str,
    full_files: Option<&(String, String)>,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    entity_overlaps: &HashMap<String, Vec<(String, u64)>>,
    overlap_click_map: &mut Vec<(usize, OverlapClickTarget)>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if entities.is_empty() {
        // Fall back to full-file character-level diff when we have content
        if let Some((before, after)) = full_files {
            if before != after {
                lines.push(Line::from(Span::styled(
                    "  No semantic changes — showing full file diff:",
                    Style::default().fg(COL_CONTEXT),
                )));
                line_map.push(None);
                lines.push(Line::from(""));
                line_map.push(None);
                render_inline_diff(before, after, &mut lines, 0, expanded, gap_map, line_map, 1);
                return lines;
            }
        }
        lines.push(Line::from(Span::styled(
            "No changes detected in this file.",
            Style::default().fg(COL_CONTEXT),
        )));
        line_map.push(None);
        return lines;
    }

    // Parse trees once for all entities (avoids N re-parses)
    let parsed_before = full_files.and_then(|(bf, _)| structural_diff::parse_file(bf, file_path));
    let parsed_after = full_files.and_then(|(_, af)| structural_diff::parse_file(af, file_path));

    for (entity_idx, entity) in entities.iter().enumerate() {
        if entity_idx > 0 {
            lines.push(Line::from(""));
            line_map.push(None);
        }

        // Entity header
        let risk_color = risk_color(entity.risk_level);
        let change_symbol = match entity.change_type {
            ChangeType::Added => "+",
            ChangeType::Deleted => "-",
            ChangeType::Renamed => ">",
            ChangeType::Moved => ">>",
            _ => "~",
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{change_symbol} "),
                Style::default().fg(risk_color),
            ),
            Span::styled(
                format!("{} ", entity.entity_type),
                Style::default().fg(Color::Rgb(80, 140, 220)),
            ),
            Span::styled(
                entity.entity_name.clone(),
                Style::default().fg(Color::Rgb(220, 220, 220)),
            ),
            Span::styled(
                format!(
                    "  risk:{:.2} blast:{} deps:{}",
                    entity.risk_score, entity.blast_radius, entity.dependent_count
                ),
                Style::default().fg(COL_CONTEXT),
            ),
        ]));
        line_map.push(None);

        // Classification + flags + overlap warning
        let mut flags = vec![format!("{}", entity.classification)];
        if entity.is_public_api {
            flags.push("public API".to_string());
        }
        if entity.structural_change == Some(false) {
            flags.push("cosmetic".to_string());
        }
        let mut flag_spans: Vec<Span<'static>> = vec![
            Span::styled(format!("  {}", flags.join(" | ")), Style::default().fg(COL_CONTEXT)),
        ];
        if let Some(others) = entity_overlaps.get(&entity.entity_name) {
            let pr_labels: Vec<String> = others.iter().map(|(_, pr)| format!("#{pr}")).collect();
            flag_spans.push(Span::styled(
                format!(" | also in {}", pr_labels.join(", ")),
                Style::default().fg(Color::Rgb(220, 150, 50)),
            ));
            // Make the flags line clickable → navigate to first overlapping PR
            if let Some((other_repo, other_pr)) = others.first() {
                overlap_click_map.push((lines.len(), OverlapClickTarget {
                    repo: other_repo.clone(),
                    pr_number: *other_pr,
                    file_path: file_path.to_string(),
                }));
            }
        }
        lines.push(Line::from(flag_spans));
        line_map.push(None);

        lines.push(Line::from(Span::styled(
            "  ─────────────────────────────────────────────────────────────────────────",
            Style::default().fg(COL_CONTEXT),
        )));
        line_map.push(None);

        // Dedent both sides so indentation-only changes don't cause
        // "everything changed" diffs (e.g. method moved between classes)
        let before = dedent(entity.before_content.as_deref().unwrap_or(""));
        let after = dedent(entity.after_content.as_deref().unwrap_or(""));
        let start_line = entity.start_line;

        match entity.change_type {
            ChangeType::Added => {
                let add_bg = Color::Rgb(0, 30, 0);
                let mut hl_state: Option<HighlightLines<'static>> = None;
                for (i, line) in after.lines().enumerate() {
                    let expanded = expand_tabs(line);
                    let mut spans = vec![Span::styled(
                        "  + ".to_string(),
                        Style::default().fg(COL_GREEN).bg(add_bg),
                    )];
                    spans.extend(highlight_line_spans(file_path, &format!("{expanded}\n"), add_bg, &mut hl_state));
                    lines.push(Line::from(spans));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + i,
                        side: DiffSide::Right,
                        commentable: true,
                    }));
                }
            }
            ChangeType::Deleted => {
                let del_bg = Color::Rgb(40, 0, 0);
                let mut hl_state: Option<HighlightLines<'static>> = None;
                for (i, line) in before.lines().enumerate() {
                    let expanded = expand_tabs(line);
                    let mut spans = vec![Span::styled(
                        "  - ".to_string(),
                        Style::default().fg(COL_RED).bg(del_bg),
                    )];
                    spans.extend(highlight_line_spans(file_path, &format!("{expanded}\n"), del_bg, &mut hl_state));
                    lines.push(Line::from(spans));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + i,
                        side: DiffSide::Left,
                        commentable: true,
                    }));
                }
            }
            _ => {
                // Try tree-sitter structural diff using pre-parsed trees or full file content
                let structural = full_files.and_then(|(bf, af)| {
                    if let (Some(bt), Some(at)) = (parsed_before.as_ref(), parsed_after.as_ref()) {
                        structural_diff::structural_diff_with_trees(bf, af, bt, at, &entity.entity_name)
                    } else {
                        structural_diff::structural_diff(bf, af, &entity.entity_name, file_path)
                    }
                });
                if let Some(blocks) = structural {
                    render_structural_blocks(&blocks, &mut lines, entity_idx, expanded, gap_map, line_map, start_line);
                } else {
                    // Fallback: line-level unified diff on entity content
                    render_inline_diff(&before, &after, &mut lines, entity_idx, expanded, gap_map, line_map, start_line);
                }
            }
        }
    }

    lines
}

/// Render entity diffs in side-by-side mode.
/// Each entity shows old content on the left column and new content on the right.
fn render_entity_diff_side_by_side(
    entities: &[EntityReview],
    file_path: &str,
    full_files: Option<&(String, String)>,
    col_width: usize,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    entity_overlaps: &HashMap<String, Vec<(String, u64)>>,
    overlap_click_map: &mut Vec<(usize, OverlapClickTarget)>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if entities.is_empty() {
        // Fall back to full-file side-by-side diff when we have content
        if let Some((before, after)) = full_files {
            if before != after {
                lines.push(Line::from(Span::styled(
                    "  No semantic changes — showing full file diff:",
                    Style::default().fg(COL_CONTEXT),
                )));
                line_map.push(None);
                lines.push(Line::from(""));
                line_map.push(None);
                render_sbs_diff(before, after, col_width, &mut lines, 0, expanded, gap_map, line_map, 1);
                return lines;
            }
        }
        lines.push(Line::from(Span::styled(
            "No changes detected in this file.",
            Style::default().fg(COL_CONTEXT),
        )));
        line_map.push(None);
        return lines;
    }

    // Parse trees once for all entities (avoids N re-parses)
    let parsed_before = full_files.and_then(|(bf, _)| structural_diff::parse_file(bf, file_path));
    let parsed_after = full_files.and_then(|(_, af)| structural_diff::parse_file(af, file_path));

    for (entity_idx, entity) in entities.iter().enumerate() {
        if entity_idx > 0 {
            lines.push(Line::from(""));
            line_map.push(None);
        }

        // Entity header (same as unified)
        let risk_color = risk_color(entity.risk_level);
        let change_symbol = match entity.change_type {
            ChangeType::Added => "+",
            ChangeType::Deleted => "-",
            ChangeType::Renamed => ">",
            ChangeType::Moved => ">>",
            _ => "~",
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{change_symbol} "),
                Style::default().fg(risk_color),
            ),
            Span::styled(
                format!("{} ", entity.entity_type),
                Style::default().fg(Color::Rgb(80, 140, 220)),
            ),
            Span::styled(
                entity.entity_name.clone(),
                Style::default().fg(Color::Rgb(220, 220, 220)),
            ),
            Span::styled(
                format!(
                    "  risk:{:.2} blast:{} deps:{}",
                    entity.risk_score, entity.blast_radius, entity.dependent_count
                ),
                Style::default().fg(COL_CONTEXT),
            ),
        ]));
        line_map.push(None);

        let mut flags = vec![format!("{}", entity.classification)];
        if entity.is_public_api {
            flags.push("public API".to_string());
        }
        if entity.structural_change == Some(false) {
            flags.push("cosmetic".to_string());
        }
        let mut flag_spans: Vec<Span<'static>> = vec![
            Span::styled(format!("  {}", flags.join(" | ")), Style::default().fg(COL_CONTEXT)),
        ];
        if let Some(others) = entity_overlaps.get(&entity.entity_name) {
            let pr_labels: Vec<String> = others.iter().map(|(_, pr)| format!("#{pr}")).collect();
            flag_spans.push(Span::styled(
                format!(" | also in {}", pr_labels.join(", ")),
                Style::default().fg(Color::Rgb(220, 150, 50)),
            ));
            if let Some((other_repo, other_pr)) = others.first() {
                overlap_click_map.push((lines.len(), OverlapClickTarget {
                    repo: other_repo.clone(),
                    pr_number: *other_pr,
                    file_path: file_path.to_string(),
                }));
            }
        }
        lines.push(Line::from(flag_spans));
        line_map.push(None);

        // Column headers
        let sep = "│";
        let left_header = pad_or_trunc("  OLD", col_width);
        let right_header = pad_or_trunc("  NEW", col_width);
        lines.push(Line::from(vec![
            Span::styled(left_header, Style::default().fg(COL_HUNK)),
            Span::styled(sep, Style::default().fg(COL_CONTEXT)),
            Span::styled(right_header, Style::default().fg(COL_HUNK)),
        ]));
        line_map.push(None);

        let divider_left = pad_or_trunc(&"─".repeat(col_width), col_width);
        let divider_right = pad_or_trunc(&"─".repeat(col_width), col_width);
        lines.push(Line::from(vec![
            Span::styled(divider_left, Style::default().fg(COL_CONTEXT)),
            Span::styled("┼", Style::default().fg(COL_CONTEXT)),
            Span::styled(divider_right, Style::default().fg(COL_CONTEXT)),
        ]));
        line_map.push(None);

        let before_raw = entity.before_content.as_deref().unwrap_or("");
        let after_raw = entity.after_content.as_deref().unwrap_or("");
        let before = dedent(&expand_tabs_block(before_raw));
        let after = dedent(&expand_tabs_block(after_raw));
        let start_line = entity.start_line;

        match entity.change_type {
            ChangeType::Added => {
                let add_bg = Color::Rgb(0, 30, 0);
                let mut hl_state: Option<HighlightLines<'static>> = None;
                for (i, line) in after.lines().enumerate() {
                    let left = pad_or_trunc("", col_width);
                    let hl_spans = highlight_line_spans(file_path, &format!("{line}\n"), add_bg, &mut hl_state);
                    let right_spans = pad_highlighted_spans("+ ", &hl_spans, col_width, add_bg);
                    let mut all_spans = vec![
                        Span::styled(left, Style::default()),
                        Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                    ];
                    all_spans.extend(right_spans);
                    lines.push(Line::from(all_spans));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + i,
                        side: DiffSide::Right,
                        commentable: true,
                    }));
                }
            }
            ChangeType::Deleted => {
                let del_bg = Color::Rgb(40, 0, 0);
                let mut hl_state: Option<HighlightLines<'static>> = None;
                for (i, line) in before.lines().enumerate() {
                    let hl_spans = highlight_line_spans(file_path, &format!("{line}\n"), del_bg, &mut hl_state);
                    let left_spans = pad_highlighted_spans("- ", &hl_spans, col_width, del_bg);
                    let right = pad_or_trunc("", col_width);
                    let mut all_spans: Vec<Span<'static>> = Vec::new();
                    all_spans.extend(left_spans);
                    all_spans.push(Span::styled(sep, Style::default().fg(COL_CONTEXT)));
                    all_spans.push(Span::styled(right, Style::default()));
                    lines.push(Line::from(all_spans));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + i,
                        side: DiffSide::Left,
                        commentable: true,
                    }));
                }
            }
            _ => {
                // Try structural diff first, using pre-parsed trees when available
                let structural = full_files.and_then(|(bf, af)| {
                    if let (Some(bt), Some(at)) = (parsed_before.as_ref(), parsed_after.as_ref()) {
                        structural_diff::structural_diff_with_trees(bf, af, bt, at, &entity.entity_name)
                    } else {
                        structural_diff::structural_diff(bf, af, &entity.entity_name, file_path)
                    }
                });

                if let Some(blocks) = structural {
                    render_structural_blocks_sbs(&blocks, col_width, &mut lines, entity_idx, expanded, gap_map, line_map, start_line);
                } else {
                    render_sbs_diff(&before, &after, col_width, &mut lines, entity_idx, expanded, gap_map, line_map, start_line);
                }
            }
        }
    }

    lines
}

/// Render a line-level diff in side-by-side columns
fn render_sbs_diff(
    before: &str,
    after: &str,
    col_width: usize,
    out: &mut Vec<Line<'static>>,
    entity_idx: usize,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    start_line: usize,
) {
    let sep = "│";
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();

    // Trim for diffing, keep originals for rendering
    let old_trimmed: String = old_lines.iter().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n");
    let new_trimmed: String = new_lines.iter().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n");

    let diff = TextDiff::from_lines(&old_trimmed, &new_trimmed);

    // Check if any inline gap for this entity is expanded → use full context
    let any_expanded = expanded.iter().any(|g| g.entity_index == entity_idx);
    let radius = if any_expanded { 1_000_000 } else { CONTEXT_LINES };

    let hunks: Vec<_> = diff.unified_diff().context_radius(radius).iter_hunks().collect();

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        // Insert a clickable gap line between hunks (when not fully expanded)
        if hunk_idx > 0 && !any_expanded {
            let gap_id = GapId { entity_index: entity_idx, gap_index: hunk_idx - 1 };
            let msg = "  ... (click to expand context) ...";
            let left_g = pad_or_trunc(msg, col_width);
            let right_g = pad_or_trunc(msg, col_width);
            let gap_style = Style::default().fg(COL_GAP).add_modifier(Modifier::DIM);
            gap_map.push((out.len(), gap_id));
            out.push(Line::from(vec![
                Span::styled(left_g, gap_style),
                Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                Span::styled(right_g, gap_style),
            ]));
            line_map.push(None);
        }

        // Hunk header (skip when fully expanded)
        if !any_expanded {
            let header = format!("{}", hunk.header());
            let left_h = pad_or_trunc(&header, col_width);
            let right_h = pad_or_trunc(&header, col_width);
            out.push(Line::from(vec![
                Span::styled(left_h, Style::default().fg(COL_HUNK)),
                Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                Span::styled(right_h, Style::default().fg(COL_HUNK)),
            ]));
            line_map.push(None);
        }

        let changes: Vec<_> = hunk.iter_changes().collect();
        let mut i = 0;

        while i < changes.len() {
            let change = &changes[i];
            match change.tag() {
                ChangeTag::Equal => {
                    let old_idx = change.old_index();
                    let old_line = old_idx
                        .and_then(|idx| old_lines.get(idx).copied())
                        .unwrap_or(change.value().trim_end_matches('\n'));
                    let left = pad_or_trunc(&format!("  {old_line}"), col_width);
                    let right = pad_or_trunc(&format!("  {old_line}"), col_width);
                    out.push(Line::from(vec![
                        Span::styled(left, Style::default().fg(COL_CONTEXT)),
                        Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                        Span::styled(right, Style::default().fg(COL_CONTEXT)),
                    ]));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + old_idx.unwrap_or(0),
                        side: DiffSide::Right,
                        commentable: false,
                    }));
                    i += 1;
                }
                ChangeTag::Delete => {
                    // Collect contiguous deletes then inserts
                    let del_start = i;
                    while i < changes.len() && changes[i].tag() == ChangeTag::Delete {
                        i += 1;
                    }
                    let ins_start = i;
                    while i < changes.len() && changes[i].tag() == ChangeTag::Insert {
                        i += 1;
                    }

                    let deletes = &changes[del_start..ins_start];
                    let inserts = &changes[ins_start..i];
                    let max_len = deletes.len().max(inserts.len());

                    for j in 0..max_len {
                        let (left, left_style) = if j < deletes.len() {
                            let old_line = deletes[j]
                                .old_index()
                                .and_then(|idx| old_lines.get(idx).copied())
                                .unwrap_or(deletes[j].value().trim_end_matches('\n'));
                            (
                                pad_or_trunc(&format!("- {old_line}"), col_width),
                                Style::default().fg(COL_RED),
                            )
                        } else {
                            (pad_or_trunc("", col_width), Style::default())
                        };

                        let (right, right_style) = if j < inserts.len() {
                            let new_line = inserts[j]
                                .new_index()
                                .and_then(|idx| new_lines.get(idx).copied())
                                .unwrap_or(inserts[j].value().trim_end_matches('\n'));
                            (
                                pad_or_trunc(&format!("+ {new_line}"), col_width),
                                Style::default().fg(COL_GREEN),
                            )
                        } else {
                            (pad_or_trunc("", col_width), Style::default())
                        };

                        out.push(Line::from(vec![
                            Span::styled(left, left_style),
                            Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                            Span::styled(right, right_style),
                        ]));
                        // Use the new side line if available, else old side
                        let fl = if j < inserts.len() {
                            start_line + inserts[j].new_index().unwrap_or(0)
                        } else if j < deletes.len() {
                            start_line + deletes[j].old_index().unwrap_or(0)
                        } else {
                            start_line
                        };
                        let side = if j < inserts.len() { DiffSide::Right } else { DiffSide::Left };
                        line_map.push(Some(DiffLineInfo { file_line: fl, side, commentable: true }));
                    }
                }
                ChangeTag::Insert => {
                    let new_idx = change.new_index();
                    let new_line = new_idx
                        .and_then(|idx| new_lines.get(idx).copied())
                        .unwrap_or(change.value().trim_end_matches('\n'));
                    let left = pad_or_trunc("", col_width);
                    let right = pad_or_trunc(&format!("+ {new_line}"), col_width);
                    out.push(Line::from(vec![
                        Span::styled(left, Style::default()),
                        Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                        Span::styled(right, Style::default().fg(COL_GREEN)),
                    ]));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + new_idx.unwrap_or(0),
                        side: DiffSide::Right,
                        commentable: true,
                    }));
                    i += 1;
                }
            }
        }
    }

    // If expanded, add a clickable line to collapse
    if any_expanded {
        let gap_id = GapId { entity_index: entity_idx, gap_index: 0 };
        let msg = "  ▼ (click to collapse context)";
        let left_g = pad_or_trunc(msg, col_width);
        let right_g = pad_or_trunc(msg, col_width);
        let gap_style = Style::default().fg(COL_GAP);
        gap_map.push((out.len(), gap_id));
        out.push(Line::from(vec![
            Span::styled(left_g, gap_style),
            Span::styled(sep, Style::default().fg(COL_CONTEXT)),
            Span::styled(right_g, gap_style),
        ]));
        line_map.push(None);
    }
}

/// Render structural diff blocks in side-by-side mode
fn render_structural_blocks_sbs(
    blocks: &[DiffBlock],
    col_width: usize,
    out: &mut Vec<Line<'static>>,
    entity_idx: usize,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    start_line: usize,
) {
    let sep = "│";
    let mut gap_counter: usize = 0;

    // Helper: push a sbs line and a None to line_map
    macro_rules! push_sbs {
        ($out:expr, $lm:expr, $left:expr, $ls:expr, $right:expr, $rs:expr) => {{
            $out.push(Line::from(vec![
                Span::styled($left, $ls),
                Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                Span::styled($right, $rs),
            ]));
            $lm.push(None);
        }};
    }

    for block in blocks {
        match block {
            DiffBlock::Unchanged(stmts) => {
                if stmts.len() <= 2 {
                    for stmt in stmts {
                        for line in stmt.lines() {
                            let text = expand_tabs(line);
                            let left = pad_or_trunc(&format!("  {text}"), col_width);
                            let right = pad_or_trunc(&format!("  {text}"), col_width);
                            push_sbs!(out, line_map, left, Style::default().fg(COL_CONTEXT), right, Style::default().fg(COL_CONTEXT));
                        }
                    }
                } else {
                    let gap_id = GapId { entity_index: entity_idx, gap_index: gap_counter };
                    gap_counter += 1;
                    let is_expanded = expanded.contains(&gap_id);

                    if is_expanded {
                        for stmt in stmts {
                            for line in stmt.lines() {
                                let text = expand_tabs(line);
                                let left = pad_or_trunc(&format!("  {text}"), col_width);
                                let right = pad_or_trunc(&format!("  {text}"), col_width);
                                push_sbs!(out, line_map, left, Style::default().fg(COL_CONTEXT), right, Style::default().fg(COL_CONTEXT));
                            }
                        }
                        let msg = format!("  ▼ {} unchanged statements (click to collapse)", stmts.len() - 2);
                        let left = pad_or_trunc(&msg, col_width);
                        let right = pad_or_trunc(&msg, col_width);
                        let gap_style = Style::default().fg(COL_GAP);
                        gap_map.push((out.len(), gap_id));
                        push_sbs!(out, line_map, left, gap_style, right, gap_style);
                    } else {
                        for line in stmts[0].lines() {
                            let text = expand_tabs(line);
                            let left = pad_or_trunc(&format!("  {text}"), col_width);
                            let right = pad_or_trunc(&format!("  {text}"), col_width);
                            push_sbs!(out, line_map, left, Style::default().fg(COL_CONTEXT), right, Style::default().fg(COL_CONTEXT));
                        }
                        let msg = format!("  ▶ ... {} unchanged statements ...", stmts.len() - 2);
                        let left = pad_or_trunc(&msg, col_width);
                        let right = pad_or_trunc(&msg, col_width);
                        let gap_style = Style::default().fg(COL_GAP).add_modifier(Modifier::DIM);
                        gap_map.push((out.len(), gap_id));
                        push_sbs!(out, line_map, left, gap_style, right, gap_style);
                        for line in stmts.last().unwrap().lines() {
                            let text = expand_tabs(line);
                            let left = pad_or_trunc(&format!("  {text}"), col_width);
                            let right = pad_or_trunc(&format!("  {text}"), col_width);
                            push_sbs!(out, line_map, left, Style::default().fg(COL_CONTEXT), right, Style::default().fg(COL_CONTEXT));
                        }
                    }
                }
            }
            DiffBlock::Removed(stmts) => {
                for stmt in stmts {
                    for line in stmt.lines() {
                        let text = expand_tabs(line);
                        let left = pad_or_trunc(&format!("- {text}"), col_width);
                        let right = pad_or_trunc("", col_width);
                        out.push(Line::from(vec![
                            Span::styled(left, Style::default().fg(COL_RED)),
                            Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                            Span::styled(right, Style::default()),
                        ]));
                        line_map.push(Some(DiffLineInfo { file_line: start_line, side: DiffSide::Left, commentable: true }));
                    }
                }
            }
            DiffBlock::Added(stmts) => {
                for stmt in stmts {
                    for line in stmt.lines() {
                        let text = expand_tabs(line);
                        let left = pad_or_trunc("", col_width);
                        let right = pad_or_trunc(&format!("+ {text}"), col_width);
                        out.push(Line::from(vec![
                            Span::styled(left, Style::default()),
                            Span::styled(sep, Style::default().fg(COL_CONTEXT)),
                            Span::styled(right, Style::default().fg(COL_GREEN)),
                        ]));
                        line_map.push(Some(DiffLineInfo { file_line: start_line, side: DiffSide::Right, commentable: true }));
                    }
                }
            }
            DiffBlock::Modified(old_stmts, new_stmts) => {
                let old_combined = old_stmts.iter().map(|s| dedent(s)).collect::<Vec<_>>().join("\n");
                let new_combined = new_stmts.iter().map(|s| dedent(s)).collect::<Vec<_>>().join("\n");
                let mut dummy_gap_map = Vec::new();
                render_sbs_diff(&old_combined, &new_combined, col_width, out, entity_idx, expanded, &mut dummy_gap_map, line_map, start_line);
            }
        }
    }
}

/// Pad or truncate a string to exactly `width` characters
fn pad_or_trunc(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.chars().take(width).collect()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// Pad/truncate highlighted spans to a fixed column width for side-by-side display.
fn pad_highlighted_spans(prefix: &str, spans: &[Span<'static>], width: usize, bg: Color) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut used = 0;

    // Prefix
    let prefix_len = prefix.chars().count();
    if prefix_len >= width {
        result.push(Span::styled(
            prefix.chars().take(width).collect::<String>(),
            Style::default().fg(COL_GREEN).bg(bg),
        ));
        return result;
    }
    result.push(Span::styled(
        prefix.to_string(),
        Style::default().fg(if prefix.starts_with('-') { COL_RED } else { COL_GREEN }).bg(bg),
    ));
    used += prefix_len;

    // Highlighted spans
    for span in spans {
        let text = &span.content;
        let text_clean = text.trim_end_matches('\n');
        let char_count = text_clean.chars().count();
        let remaining = width.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        if char_count <= remaining {
            result.push(Span::styled(text_clean.to_string(), span.style));
            used += char_count;
        } else {
            let truncated: String = text_clean.chars().take(remaining).collect();
            result.push(Span::styled(truncated, span.style));
            used += remaining;
            break;
        }
    }

    // Pad remainder
    if used < width {
        result.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }

    result
}

/// Render structural diff blocks from tree-sitter analysis.
/// Unchanged blocks are collapsed, changed blocks shown as units.
fn render_structural_blocks(
    blocks: &[DiffBlock],
    out: &mut Vec<Line<'static>>,
    entity_idx: usize,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    start_line: usize,
) {
    let mut gap_counter: usize = 0;

    // Helper: push a unified line + None line_map entry
    macro_rules! push_line {
        ($out:expr, $lm:expr, $text:expr, $style:expr) => {{
            $out.push(Line::from(Span::styled($text, $style)));
            $lm.push(None);
        }};
    }

    for block in blocks {
        match block {
            DiffBlock::Unchanged(stmts) => {
                if stmts.len() <= 2 {
                    for stmt in stmts {
                        for line in stmt.lines() {
                            push_line!(out, line_map, format!("    {}", expand_tabs(line)), Style::default().fg(COL_CONTEXT));
                        }
                    }
                } else {
                    let gap_id = GapId { entity_index: entity_idx, gap_index: gap_counter };
                    gap_counter += 1;
                    let is_expanded = expanded.contains(&gap_id);

                    if is_expanded {
                        for stmt in stmts {
                            for line in stmt.lines() {
                                push_line!(out, line_map, format!("    {}", expand_tabs(line)), Style::default().fg(COL_CONTEXT));
                            }
                        }
                        let msg = format!("    ▼ {} unchanged statements (click to collapse)", stmts.len() - 2);
                        gap_map.push((out.len(), gap_id));
                        push_line!(out, line_map, msg, Style::default().fg(COL_GAP));
                    } else {
                        for line in stmts[0].lines() {
                            push_line!(out, line_map, format!("    {}", expand_tabs(line)), Style::default().fg(COL_CONTEXT));
                        }
                        let msg = format!("    ▶ ... {} unchanged statements ...", stmts.len() - 2);
                        gap_map.push((out.len(), gap_id));
                        push_line!(out, line_map, msg, Style::default().fg(COL_GAP).add_modifier(Modifier::DIM));
                        for line in stmts.last().unwrap().lines() {
                            push_line!(out, line_map, format!("    {}", expand_tabs(line)), Style::default().fg(COL_CONTEXT));
                        }
                    }
                }
            }
            DiffBlock::Removed(stmts) => {
                for stmt in stmts {
                    for line in stmt.lines() {
                        out.push(Line::from(Span::styled(
                            format!("  - {}", expand_tabs(line)),
                            Style::default().fg(COL_RED),
                        )));
                        line_map.push(Some(DiffLineInfo { file_line: start_line, side: DiffSide::Left, commentable: true }));
                    }
                }
            }
            DiffBlock::Added(stmts) => {
                for stmt in stmts {
                    for line in stmt.lines() {
                        out.push(Line::from(Span::styled(
                            format!("  + {}", expand_tabs(line)),
                            Style::default().fg(COL_GREEN),
                        )));
                        line_map.push(Some(DiffLineInfo { file_line: start_line, side: DiffSide::Right, commentable: true }));
                    }
                }
            }
            DiffBlock::Modified(old_stmts, new_stmts) => {
                render_modified_block(old_stmts, new_stmts, out, entity_idx, expanded, gap_map, line_map, start_line);
            }
        }
    }
}

/// Render a Modified block by pairing old/new statements by similarity.
/// Matched pairs get inline word-level diffs; unmatched get plain red/green.
fn render_modified_block(
    old_stmts: &[String],
    new_stmts: &[String],
    out: &mut Vec<Line<'static>>,
    entity_idx: usize,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    start_line: usize,
) {
    if old_stmts.len() == 1 && new_stmts.len() == 1 {
        let old = dedent(&old_stmts[0]);
        let new = dedent(&new_stmts[0]);
        render_inline_diff(&old, &new, out, entity_idx, expanded, gap_map, line_map, start_line);
        return;
    }

    // For multi-statement Modified blocks (e.g., several methods changed at class level),
    // try to match old statements to new statements by similarity, then render
    // each matched pair as an inline diff.
    let mut used_new = vec![false; new_stmts.len()];
    let mut pairs: Vec<(usize, Option<usize>)> = Vec::new(); // (old_idx, matched_new_idx)

    for (oi, old) in old_stmts.iter().enumerate() {
        let best = new_stmts
            .iter()
            .enumerate()
            .filter(|(ni, _)| !used_new[*ni])
            .map(|(ni, new)| (ni, stmt_similarity(old, new)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        if let Some((ni, sim)) = best {
            if sim > 0.3 {
                used_new[ni] = true;
                pairs.push((oi, Some(ni)));
                continue;
            }
        }
        pairs.push((oi, None));
    }

    // Render matched pairs as inline diffs, unmatched old as removals
    for (oi, matched_ni) in &pairs {
        let old = &old_stmts[*oi];
        if let Some(ni) = matched_ni {
            let new = &new_stmts[*ni];
            // Dedent both sides so indentation changes don't appear as diffs
            let old_d = dedent(old);
            let new_d = dedent(new);
            // If they're identical after normalization, show as unchanged context
            let old_norm: String = old_d.lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n");
            let new_norm: String = new_d.lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n");
            if old_norm == new_norm {
                for line in old_d.lines() {
                    out.push(Line::from(Span::styled(
                        format!("    {}", expand_tabs(line)),
                        Style::default().fg(COL_CONTEXT),
                    )));
                    line_map.push(None);
                }
            } else {
                render_inline_diff(&old_d, &new_d, out, entity_idx, expanded, gap_map, line_map, start_line);
            }
        } else {
            let old_d = dedent(old);
            for line in old_d.lines() {
                out.push(Line::from(Span::styled(
                    format!("  - {}", expand_tabs(line)),
                    Style::default().fg(COL_RED),
                )));
                line_map.push(Some(DiffLineInfo { file_line: start_line, side: DiffSide::Left, commentable: true }));
            }
        }
    }

    for (ni, new) in new_stmts.iter().enumerate() {
        if !used_new[ni] {
            let new_d = dedent(new);
            for line in new_d.lines() {
                out.push(Line::from(Span::styled(
                    format!("  + {}", expand_tabs(line)),
                    Style::default().fg(COL_GREEN),
                )));
                line_map.push(Some(DiffLineInfo { file_line: start_line, side: DiffSide::Right, commentable: true }));
            }
        }
    }
}

/// Similarity between two statements (0.0–1.0) based on shared lines.
fn stmt_similarity(a: &str, b: &str) -> f64 {
    let a_lines: Vec<&str> = a.lines().map(|l| l.trim()).collect();
    let b_lines: Vec<&str> = b.lines().map(|l| l.trim()).collect();
    let total = a_lines.len() + b_lines.len();
    if total == 0 {
        return 1.0;
    }
    let shared = a_lines.iter().filter(|l| b_lines.contains(l)).count();
    (2.0 * shared as f64) / total as f64
}

/// Render a unified diff between before/after with character-level highlighting.
///
/// Diffs on trimmed content (ignoring indentation entirely), but renders using
/// the old side's indentation for all lines. Insert lines get the old base indent
/// so paired -/+ lines always align.
fn render_inline_diff(
    before: &str,
    after: &str,
    out: &mut Vec<Line<'static>>,
    entity_idx: usize,
    expanded: &HashSet<GapId>,
    gap_map: &mut Vec<(usize, GapId)>,
    line_map: &mut LineMap,
    start_line: usize,
) {
    let before_expanded = expand_tabs_block(before);
    let after_expanded = expand_tabs_block(after);

    // Keep original lines for rendering
    let old_lines: Vec<&str> = before_expanded.lines().collect();
    let new_lines: Vec<&str> = after_expanded.lines().collect();

    // Trimmed versions for diffing (indentation ignored)
    let old_trimmed_joined: String = old_lines.iter().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n");
    let new_trimmed_joined: String = new_lines.iter().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n");

    // Build rebased new lines: shift new indentation to match old base
    let old_base = min_indent(&before_expanded);
    let new_base = min_indent(&after_expanded);
    let rebased_new: Vec<String> = new_lines
        .iter()
        .map(|l| {
            let indent = l.len() - l.trim_start().len();
            let relative = indent.saturating_sub(new_base);
            format!("{}{}", " ".repeat(old_base + relative), l.trim_start())
        })
        .collect();

    let diff = TextDiff::from_lines(&old_trimmed_joined, &new_trimmed_joined);

    // Check if any inline gap for this entity is expanded → use full context
    let any_expanded = expanded.iter().any(|g| g.entity_index == entity_idx);
    let radius = if any_expanded { 1_000_000 } else { CONTEXT_LINES };

    let hunks: Vec<_> = diff.unified_diff().context_radius(radius).iter_hunks().collect();

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        if hunk_idx > 0 && !any_expanded {
            let gap_id = GapId { entity_index: entity_idx, gap_index: hunk_idx - 1 };
            gap_map.push((out.len(), gap_id));
            out.push(Line::from(Span::styled(
                "    ▶ ... (click to expand context) ...",
                Style::default().fg(COL_GAP).add_modifier(Modifier::DIM),
            )));
            line_map.push(None);
        }

        if !any_expanded {
            out.push(Line::from(Span::styled(
                format!("  {}", hunk.header()),
                Style::default().fg(COL_HUNK),
            )));
            line_map.push(None);
        }

        let changes: Vec<_> = hunk.iter_changes().collect();
        let mut i = 0;

        while i < changes.len() {
            let change = &changes[i];
            match change.tag() {
                ChangeTag::Equal => {
                    let old_idx = change.old_index();
                    let rendered = old_idx
                        .and_then(|idx| old_lines.get(idx).copied())
                        .unwrap_or(change.value().trim_end_matches('\n'));
                    out.push(Line::from(Span::styled(
                        format!("    {rendered}"),
                        Style::default().fg(COL_CONTEXT),
                    )));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + old_idx.unwrap_or(0),
                        side: DiffSide::Right,
                        commentable: false,
                    }));
                    i += 1;
                }
                ChangeTag::Delete => {
                    let del_start = i;
                    while i < changes.len() && changes[i].tag() == ChangeTag::Delete {
                        i += 1;
                    }
                    let ins_start = i;
                    while i < changes.len() && changes[i].tag() == ChangeTag::Insert {
                        i += 1;
                    }

                    let deletes = &changes[del_start..ins_start];
                    let inserts = &changes[ins_start..i];
                    let mut used_ins = vec![false; inserts.len()];

                    for del in deletes {
                        let old_rendered = del
                            .old_index()
                            .and_then(|idx| old_lines.get(idx).copied())
                            .unwrap_or(del.value().trim_end_matches('\n'));

                        let best = inserts
                            .iter()
                            .enumerate()
                            .filter(|(k, _)| !used_ins[*k])
                            .map(|(k, ins)| {
                                let v = ins.value().trim_end_matches('\n');
                                (k, line_similarity(old_rendered.trim_start(), v))
                            })
                            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

                        if let Some((best_idx, sim)) = best {
                            if sim > 0.4 {
                                let new_rendered = inserts[best_idx]
                                    .new_index()
                                    .and_then(|idx| rebased_new.get(idx))
                                    .map(|s| s.as_str())
                                    .unwrap_or(
                                        inserts[best_idx].value().trim_end_matches('\n'),
                                    );
                                let (del_line, ins_line) =
                                    inline_word_diff(old_rendered, new_rendered);
                                out.push(del_line);
                                line_map.push(Some(DiffLineInfo {
                                    file_line: start_line + del.old_index().unwrap_or(0),
                                    side: DiffSide::Left,
                                    commentable: true,
                                }));
                                out.push(ins_line);
                                line_map.push(Some(DiffLineInfo {
                                    file_line: start_line + inserts[best_idx].new_index().unwrap_or(0),
                                    side: DiffSide::Right,
                                    commentable: true,
                                }));
                                used_ins[best_idx] = true;
                                continue;
                            }
                        }

                        out.push(Line::from(Span::styled(
                            format!("  - {old_rendered}"),
                            Style::default().fg(COL_RED),
                        )));
                        line_map.push(Some(DiffLineInfo {
                            file_line: start_line + del.old_index().unwrap_or(0),
                            side: DiffSide::Left,
                            commentable: true,
                        }));
                    }

                    for (k, ins) in inserts.iter().enumerate() {
                        if !used_ins[k] {
                            let new_rendered = ins
                                .new_index()
                                .and_then(|idx| rebased_new.get(idx))
                                .map(|s| s.as_str())
                                .unwrap_or(ins.value().trim_end_matches('\n'));
                            out.push(Line::from(Span::styled(
                                format!("  + {new_rendered}"),
                                Style::default().fg(COL_GREEN),
                            )));
                            line_map.push(Some(DiffLineInfo {
                                file_line: start_line + ins.new_index().unwrap_or(0),
                                side: DiffSide::Right,
                                commentable: true,
                            }));
                        }
                    }
                }
                ChangeTag::Insert => {
                    let new_idx = change.new_index();
                    let new_rendered = new_idx
                        .and_then(|idx| rebased_new.get(idx))
                        .map(|s| s.as_str())
                        .unwrap_or(change.value().trim_end_matches('\n'));
                    out.push(Line::from(Span::styled(
                        format!("  + {new_rendered}"),
                        Style::default().fg(COL_GREEN),
                    )));
                    line_map.push(Some(DiffLineInfo {
                        file_line: start_line + new_idx.unwrap_or(0),
                        side: DiffSide::Right,
                        commentable: true,
                    }));
                    i += 1;
                }
            }
        }
    }

    if any_expanded {
        let gap_id = GapId { entity_index: entity_idx, gap_index: 0 };
        gap_map.push((out.len(), gap_id));
        out.push(Line::from(Span::styled(
            "    ▼ (click to collapse context)",
            Style::default().fg(COL_GAP),
        )));
        line_map.push(None);
    }
}

/// Compute similarity ratio between two lines (0.0 = totally different, 1.0 = identical).
/// Uses char-level diff to count matching characters.
fn line_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let diff = TextDiff::from_chars(a, b);
    let equal: usize = diff
        .iter_all_changes()
        .filter(|c| c.tag() == ChangeTag::Equal)
        .map(|c| c.value().len())
        .sum();
    (2.0 * equal as f64) / total as f64
}

/// Produce a pair of Lines (removed, added) with word-level diff highlighting.
/// Unchanged words are normal red/green; changed words are bold with a background tint.
fn inline_word_diff(old: &str, new: &str) -> (Line<'static>, Line<'static>) {
    let diff = TextDiff::from_words(old, new);

    let mut del_spans: Vec<Span<'static>> = vec![Span::styled("  - ", Style::default().fg(COL_RED))];
    let mut ins_spans: Vec<Span<'static>> = vec![Span::styled("  + ", Style::default().fg(COL_GREEN))];

    // Style definitions
    let del_normal = Style::default().fg(COL_RED);
    let del_highlight = Style::default()
        .fg(COL_HL_DEL_FG)
        .bg(COL_HL_DEL_BG)
        .add_modifier(Modifier::BOLD);
    let ins_normal = Style::default().fg(COL_GREEN);
    let ins_highlight = Style::default()
        .fg(COL_HL_INS_FG)
        .bg(COL_HL_INS_BG)
        .add_modifier(Modifier::BOLD);

    // Accumulate contiguous runs of same style to minimize span count
    let mut del_buf = String::new();
    let mut del_is_hl = false;
    let mut ins_buf = String::new();
    let mut ins_is_hl = false;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                let val: String = change.value().to_string();
                // Flush highlighted buffers before switching to normal
                if del_is_hl && !del_buf.is_empty() {
                    del_spans.push(Span::styled(del_buf.clone(), del_highlight));
                    del_buf.clear();
                }
                if ins_is_hl && !ins_buf.is_empty() {
                    ins_spans.push(Span::styled(ins_buf.clone(), ins_highlight));
                    ins_buf.clear();
                }
                del_buf.push_str(&val);
                ins_buf.push_str(&val);
                del_is_hl = false;
                ins_is_hl = false;
            }
            ChangeTag::Delete => {
                let val: String = change.value().to_string();
                if !del_is_hl && !del_buf.is_empty() {
                    del_spans.push(Span::styled(del_buf.clone(), del_normal));
                    del_buf.clear();
                }
                del_buf.push_str(&val);
                del_is_hl = true;
            }
            ChangeTag::Insert => {
                let val: String = change.value().to_string();
                if !ins_is_hl && !ins_buf.is_empty() {
                    ins_spans.push(Span::styled(ins_buf.clone(), ins_normal));
                    ins_buf.clear();
                }
                ins_buf.push_str(&val);
                ins_is_hl = true;
            }
        }
    }

    // Flush remaining
    if !del_buf.is_empty() {
        del_spans.push(Span::styled(
            del_buf,
            if del_is_hl { del_highlight } else { del_normal },
        ));
    }
    if !ins_buf.is_empty() {
        ins_spans.push(Span::styled(
            ins_buf,
            if ins_is_hl { ins_highlight } else { ins_normal },
        ));
    }

    (Line::from(del_spans), Line::from(ins_spans))
}

/// Strip common leading whitespace from both sides so indentation changes
/// (e.g. method moved between classes) don't cause "everything changed" diffs.
fn dedent(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    if min_indent == 0 {
        return s.to_string();
    }
    lines
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                l.trim()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Expand tabs in an entire block (preserving line structure for diffing)
/// Minimum indentation (number of leading spaces) across non-empty lines.
fn min_indent(s: &str) -> usize {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0)
}

/// Rebase indentation: strip `from_base` spaces from each line and prepend `to_base` spaces.
/// Preserves relative indentation while shifting the base level.
fn rebase_indent(s: &str, from_base: usize, to_base: usize) -> String {
    if from_base == to_base {
        return s.to_string();
    }
    let prefix = " ".repeat(to_base);
    s.lines()
        .map(|l| {
            let stripped = if l.len() >= from_base {
                &l[from_base..]
            } else {
                l.trim_start()
            };
            format!("{prefix}{stripped}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expand_tabs_block(s: &str) -> String {
    s.lines()
        .map(|l| expand_tabs(l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_pr_summary(result: &ReviewResult, repo: &str, pr_number: u64, click_map: &mut Vec<(usize, String)>, entity_overlaps: &HashMap<String, Vec<(String, u64)>>, overlap_click_map: &mut Vec<(usize, OverlapClickTarget)>, pr_body: &str, pr_html_url: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(Span::styled(
        format!(" {repo} #{pr_number} — Analysis Summary"),
        Style::default().fg(COL_HUNK),
    )));

    // Show GitHub URL if available
    if !pr_html_url.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" GitHub: ", Style::default().fg(COL_CONTEXT)),
            Span::styled(
                pr_html_url.to_string(),
                Style::default().fg(Color::Rgb(80, 180, 220)).add_modifier(Modifier::UNDERLINED),
            ),
        ]));
    }

    lines.push(Line::from(""));

    let stats = &result.stats;
    lines.push(Line::from(format!(
        " {} entities changed",
        stats.total_entities
    )));

    lines.push(Line::from(vec![
        Span::styled(
            format!("  {} critical", stats.by_risk.critical),
            Style::default().fg(Color::Rgb(220, 50, 50)),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} high", stats.by_risk.high),
            Style::default().fg(Color::Rgb(220, 180, 50)),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} medium", stats.by_risk.medium),
            Style::default().fg(Color::Rgb(80, 140, 220)),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} low", stats.by_risk.low),
            Style::default().fg(COL_CONTEXT),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        " Changes: {} added, {} modified, {} deleted",
        stats.by_change_type.added, stats.by_change_type.modified, stats.by_change_type.deleted
    )));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Top entities by risk:",
        Style::default().fg(Color::Rgb(220, 220, 220)),
    )));

    let mut sorted_entities = result.entity_reviews.clone();
    sort_entities_by_risk(&mut sorted_entities, entity_overlaps);

    for entity in sorted_entities.iter().take(15) {
        let risk_color = risk_color(entity.risk_level);
        let line_idx = lines.len();
        let mut spans = vec![
            Span::styled(
                format!("  {:.2} ", entity.risk_score),
                Style::default().fg(risk_color),
            ),
            Span::styled(
                format!("{} ", entity.entity_type),
                Style::default().fg(Color::Rgb(80, 140, 220)),
            ),
            Span::raw(format!("{} ", entity.entity_name)),
            Span::styled(
                format!("({})", entity.file_path),
                Style::default().fg(COL_CONTEXT),
            ),
        ];
        if let Some(others) = entity_overlaps.get(&entity.entity_name) {
            let pr_labels: Vec<String> = others.iter().map(|(_, pr)| format!("#{pr}")).collect();
            spans.push(Span::styled(
                format!(" also in {}", pr_labels.join(", ")),
                Style::default().fg(Color::Rgb(220, 150, 50)),
            ));
            if let Some((other_repo, other_pr)) = others.first() {
                overlap_click_map.push((line_idx, OverlapClickTarget {
                    repo: other_repo.clone(),
                    pr_number: *other_pr,
                    file_path: entity.file_path.clone(),
                }));
            }
        }
        lines.push(Line::from(spans));
        click_map.push((line_idx, entity.file_path.clone()));
    }

    if result.entity_reviews.len() > 15 {
        lines.push(Line::from(Span::styled(
            format!("  ... and {} more", result.entity_reviews.len() - 15),
            Style::default().fg(COL_CONTEXT),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " Analysis: {}ms total (diff: {}ms, graph: {}ms, scoring: {}ms)",
            result.timing.total_ms,
            result.timing.diff_ms,
            result.timing.graph_build_ms,
            result.timing.scoring_ms,
        ),
        Style::default().fg(COL_CONTEXT),
    )));

    // Show PR description at the end (after analysis) since it can be large
    if !pr_body.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Description:",
            Style::default().fg(Color::Rgb(220, 220, 220)),
        )));
        for desc_line in pr_body.lines() {
            lines.push(Line::from(format!("   {desc_line}")));
        }
    }

    lines
}

fn risk_color(level: RiskLevel) -> Color {
    match level {
        RiskLevel::Critical => Color::Rgb(220, 50, 50),
        RiskLevel::High => Color::Rgb(220, 180, 50),
        RiskLevel::Medium => Color::Rgb(80, 140, 220),
        RiskLevel::Low => COL_CONTEXT,
    }
}

/// Expand tab characters to spaces (4-space tab stops) so ratatui measures
/// line widths correctly. Without this, ratatui counts a tab as 1 column
/// but the terminal renders it as jumping to the next 8-col tab stop,
/// causing massive display corruption.
fn expand_tabs(s: &str) -> String {
    let tab_width = 4;
    let mut result = String::with_capacity(s.len());
    let mut col = 0;
    for ch in s.chars() {
        if ch == '\t' {
            let spaces = tab_width - (col % tab_width);
            for _ in 0..spaces {
                result.push(' ');
            }
            col += spaces;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}

/// Render the inline comment editor as a series of Lines to inject into the diff.
/// `panel_width` is the inner width of the diff panel (used to size the editor box).
fn render_inline_editor(editor: &InlineEditor, panel_width: usize) -> Vec<Line<'static>> {
    let border_color = Color::Rgb(80, 180, 220);
    let text_style = Style::default().fg(Color::Rgb(220, 220, 220));
    let hint_style = Style::default().fg(Color::DarkGray);

    // Editor box: leave 2 chars indent + 2 border chars
    let width = panel_width.saturating_sub(4).max(20);
    let top_border = format!("  ┌{}┐", "─".repeat(width));
    let bot_border = format!("  └{}┘", "─".repeat(width));

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(top_border, Style::default().fg(border_color))));

    // Build visual rows by wrapping each editor line to `width`
    let cursor_line = editor.cursor.0;
    let cursor_col = editor.cursor.1;

    for (li, text) in editor.lines.iter().enumerate() {
        let is_placeholder = text.is_empty() && li == 0 && editor.lines.len() == 1;
        let display = if is_placeholder {
            "Type your comment...".to_string()
        } else {
            text.clone()
        };

        let style = if is_placeholder { hint_style } else { text_style };

        // Word-wrap this editor line into visual rows
        let wrapped_rows = wrap_text(&display, width);
        let is_cursor_line = li == cursor_line;

        // Map cursor_col to (visual_row, visual_col) within wrapped rows
        let (cursor_vrow, cursor_vcol) = if is_cursor_line {
            let mut remaining = cursor_col;
            let mut found = (0, 0);
            for (ri, row) in wrapped_rows.iter().enumerate() {
                if remaining <= row.len() || ri == wrapped_rows.len() - 1 {
                    found = (ri, remaining.min(row.len()));
                    break;
                }
                remaining -= row.len();
            }
            found
        } else {
            (0, 0)
        };

        for (ri, row) in wrapped_rows.iter().enumerate() {
            let padded = if row.len() >= width {
                row[..width].to_string()
            } else {
                format!("{}{}", row, " ".repeat(width - row.len()))
            };

            if is_cursor_line && ri == cursor_vrow {
                let col = cursor_vcol.min(padded.len());
                let before = &padded[..col];
                let cursor_ch = padded.get(col..col + 1).unwrap_or(" ");
                let after = if col + 1 < padded.len() { &padded[col + 1..] } else { "" };
                lines.push(Line::from(vec![
                    Span::styled("  │", Style::default().fg(border_color)),
                    Span::styled(before.to_string(), style),
                    Span::styled(
                        cursor_ch.to_string(),
                        Style::default().fg(Color::Black).bg(Color::White),
                    ),
                    Span::styled(after.to_string(), style),
                    Span::styled("│", Style::default().fg(border_color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("  │", Style::default().fg(border_color)),
                    Span::styled(padded, style),
                    Span::styled("│", Style::default().fg(border_color)),
                ]));
            }
        }
    }

    // Hint line
    let hints = if editor.editing_index.is_some() {
        "Alt+Enter/Ctrl+S:save  Ctrl+D:delete  Esc:cancel"
    } else {
        "Alt+Enter/Ctrl+S:save  Esc:cancel"
    };
    let hints_padded = if hints.len() >= width {
        hints[..width].to_string()
    } else {
        format!("{}{}", hints, " ".repeat(width - hints.len()))
    };
    lines.push(Line::from(vec![
        Span::styled("  │", Style::default().fg(border_color)),
        Span::styled(hints_padded, hint_style),
        Span::styled("│", Style::default().fg(border_color)),
    ]));

    lines.push(Line::from(Span::styled(bot_border, Style::default().fg(border_color))));
    lines
}

/// Render an expanded review thread as a bordered box of comments.
fn render_thread_comments(thread: &ReviewThread, panel_width: usize) -> Vec<Line<'static>> {
    let border_color = Color::Cyan;
    let author_style = Style::default().fg(Color::Rgb(100, 200, 220)).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::Rgb(200, 200, 200));
    let resolved_style = Style::default().fg(Color::DarkGray);

    let width = panel_width.saturating_sub(4).max(20);
    let top_border = format!("  ┌{}┐", "─".repeat(width));
    let bot_border = format!("  └{}┘", "─".repeat(width));

    let mut lines = Vec::new();

    // Header
    let header_text = if thread.is_resolved {
        format!("── {} comments [resolved] ──", thread.comments.len())
    } else {
        format!("── {} comments ──", thread.comments.len())
    };
    let header_padded = if header_text.len() >= width {
        header_text[..width].to_string()
    } else {
        format!("{}{}", header_text, " ".repeat(width - header_text.len()))
    };
    lines.push(Line::from(Span::styled(top_border, Style::default().fg(border_color))));
    lines.push(Line::from(vec![
        Span::styled("  │", Style::default().fg(border_color)),
        Span::styled(header_padded, if thread.is_resolved { resolved_style } else { Style::default().fg(Color::Cyan) }),
        Span::styled("│", Style::default().fg(border_color)),
    ]));

    for (i, comment) in thread.comments.iter().enumerate() {
        if i > 0 {
            // Separator between comments
            let sep = format!("{}", "┄".repeat(width));
            lines.push(Line::from(vec![
                Span::styled("  │", Style::default().fg(border_color)),
                Span::styled(sep, Style::default().fg(Color::DarkGray)),
                Span::styled("│", Style::default().fg(border_color)),
            ]));
        }

        // Author line
        let author_text = format!("{}:", comment.author);
        let author_padded = if author_text.len() >= width {
            author_text[..width].to_string()
        } else {
            format!("{}{}", author_text, " ".repeat(width - author_text.len()))
        };
        lines.push(Line::from(vec![
            Span::styled("  │", Style::default().fg(border_color)),
            Span::styled(author_padded, author_style),
            Span::styled("│", Style::default().fg(border_color)),
        ]));

        // Body text (wrapped)
        let body_width = width;
        for body_line in comment.body.lines() {
            let wrapped = wrap_text(body_line, body_width);
            for row in wrapped {
                let padded = if row.len() >= body_width {
                    row[..body_width].to_string()
                } else {
                    format!("{}{}", row, " ".repeat(body_width - row.len()))
                };
                lines.push(Line::from(vec![
                    Span::styled("  │", Style::default().fg(border_color)),
                    Span::styled(padded, text_style),
                    Span::styled("│", Style::default().fg(border_color)),
                ]));
            }
        }
    }

    lines.push(Line::from(Span::styled(bot_border, Style::default().fg(border_color))));
    lines
}

/// Render PR discussion comments as a section to append to the PR summary.
fn render_discussion_comments(comments: &[PrComment], _panel_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ── Discussion ──",
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));

    for comment in comments {
        // Author + date header
        let date = &comment.created_at[..10.min(comment.created_at.len())];
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", comment.author),
                Style::default().fg(Color::Rgb(100, 200, 220)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({})", date),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Body
        for body_line in comment.body.lines() {
            lines.push(Line::from(Span::styled(
                format!("   {}", body_line),
                Style::default().fg(Color::Rgb(200, 200, 200)),
            )));
        }
        lines.push(Line::from(""));
    }

    lines
}

/// Wrap text at `width` characters, breaking at word boundaries when possible.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if text.len() <= width {
        return vec![text.to_string()];
    }

    let mut rows = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= width {
            rows.push(remaining.to_string());
            break;
        }

        // Try to break at last space within width
        let chunk = &remaining[..width];
        let break_at = if let Some(pos) = chunk.rfind(' ') {
            if pos > 0 { pos + 1 } else { width }
        } else {
            width // No space found, hard break
        };

        rows.push(remaining[..break_at].to_string());
        remaining = &remaining[break_at..];
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use inspect_core::types::{
        ChangeTypeBreakdown, ClassificationBreakdown, ReviewResult, ReviewStats, RiskBreakdown,
        Timing,
    };

    fn empty_review_result() -> ReviewResult {
        ReviewResult {
            entity_reviews: vec![],
            groups: vec![],
            stats: ReviewStats {
                total_entities: 0,
                by_risk: RiskBreakdown {
                    critical: 0,
                    high: 0,
                    medium: 0,
                    low: 0,
                },
                by_classification: ClassificationBreakdown {
                    text: 0,
                    syntax: 0,
                    functional: 0,
                    mixed: 0,
                },
                by_change_type: ChangeTypeBreakdown {
                    added: 0,
                    modified: 0,
                    deleted: 0,
                    moved: 0,
                    renamed: 0,
                },
            },
            timing: Timing::default(),
            changes: vec![],
        }
    }

    /// Flatten all spans in all lines to a single string for easy content assertions.
    fn lines_to_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn render_pr_summary_includes_github_url_when_non_empty() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();
        let url = "https://github.com/owner/repo/pull/42";

        let lines = render_pr_summary(
            &result,
            "owner/repo",
            42,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            "",
            url,
        );

        let text = lines_to_text(&lines);
        assert!(
            text.contains(url),
            "Expected rendered output to contain the GitHub URL, got:\n{text}"
        );
    }

    #[test]
    fn render_pr_summary_omits_github_url_when_empty() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();

        let lines = render_pr_summary(
            &result,
            "owner/repo",
            42,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            "",
            "",
        );

        let text = lines_to_text(&lines);
        assert!(
            !text.contains("GitHub:"),
            "Expected no GitHub: label when url is empty, got:\n{text}"
        );
    }

    #[test]
    fn render_pr_summary_includes_description_when_non_empty() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();
        let body = "This PR fixes an important bug.";

        let lines = render_pr_summary(
            &result,
            "owner/repo",
            42,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            body,
            "",
        );

        let text = lines_to_text(&lines);
        assert!(
            text.contains("Description:"),
            "Expected 'Description:' label in output, got:\n{text}"
        );
        assert!(
            text.contains(body),
            "Expected PR body text in output, got:\n{text}"
        );
    }

    #[test]
    fn render_pr_summary_omits_description_when_empty() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();

        let lines = render_pr_summary(
            &result,
            "owner/repo",
            42,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            "",
            "",
        );

        let text = lines_to_text(&lines);
        assert!(
            !text.contains("Description:"),
            "Expected no 'Description:' label when body is empty, got:\n{text}"
        );
    }

    #[test]
    fn render_pr_summary_includes_both_url_and_description() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();
        let body = "Adds feature X to the system.";
        let url = "https://github.com/owner/repo/pull/7";

        let lines = render_pr_summary(
            &result,
            "owner/repo",
            7,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            body,
            url,
        );

        let text = lines_to_text(&lines);
        assert!(text.contains(url), "Expected URL in output");
        assert!(text.contains("Description:"), "Expected Description: label");
        assert!(text.contains(body), "Expected body text in output");
    }

    #[test]
    fn render_pr_summary_multiline_body_each_line_rendered() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();
        let body = "First line\nSecond line\nThird line";

        let lines = render_pr_summary(
            &result,
            "owner/repo",
            1,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            body,
            "",
        );

        let text = lines_to_text(&lines);
        assert!(text.contains("First line"), "Expected first line of body");
        assert!(text.contains("Second line"), "Expected second line of body");
        assert!(text.contains("Third line"), "Expected third line of body");
    }

    #[test]
    fn render_pr_summary_always_includes_title_header() {
        let result = empty_review_result();
        let mut click_map = Vec::new();
        let mut overlap_click_map = Vec::new();
        let entity_overlaps = HashMap::new();

        let lines = render_pr_summary(
            &result,
            "my/repo",
            99,
            &mut click_map,
            &entity_overlaps,
            &mut overlap_click_map,
            "",
            "",
        );

        let text = lines_to_text(&lines);
        assert!(
            text.contains("my/repo #99"),
            "Expected repo and PR number in header, got:\n{text}"
        );
        assert!(
            text.contains("Analysis Summary"),
            "Expected 'Analysis Summary' in header, got:\n{text}"
        );
    }

    #[test]
    fn show_pr_summary_stores_pr_body_and_url_from_pr_data() {
        use crate::github::PrData;

        let pr_data = PrData {
            number: 5,
            title: "Test".to_string(),
            author: "dev".to_string(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            head_ref: "branch".to_string(),
            base_ref: "main".to_string(),
            head_sha: "sha".to_string(),
            updated_at: "2025-01-15T08:30:00Z".to_string(),
            files: vec![],
            body: "PR body text".to_string(),
            html_url: "https://github.com/owner/repo/pull/5".to_string(),
        };

        let result = empty_review_result();
        let overlaps = HashMap::new();
        let mut panel = DiffPanel::new();

        panel.show_pr_summary("owner/repo", 5, &result, &overlaps, Some(&pr_data));

        // After show_pr_summary, base_lines are built by rebuild_base_lines.
        // We verify indirectly by checking lines_dirty is false and base_lines are populated.
        assert!(!panel.lines_dirty, "lines should be built after show_pr_summary");
        assert!(!panel.base_lines.is_empty(), "base_lines should have content");

        let text = lines_to_text(&panel.base_lines);
        assert!(text.contains("PR body text"), "Expected body in rendered lines");
        assert!(
            text.contains("https://github.com/owner/repo/pull/5"),
            "Expected URL in rendered lines"
        );
    }

    #[test]
    fn show_pr_summary_with_no_pr_data_renders_no_url_or_description() {
        let result = empty_review_result();
        let overlaps = HashMap::new();
        let mut panel = DiffPanel::new();

        panel.show_pr_summary("owner/repo", 3, &result, &overlaps, None);

        assert!(!panel.base_lines.is_empty(), "base_lines should have content");

        let text = lines_to_text(&panel.base_lines);
        assert!(!text.contains("GitHub:"), "No URL when pr_data is None");
        assert!(!text.contains("Description:"), "No description when pr_data is None");
    }
}
