/// Mapping from a rendered line index to its diff-level metadata.
pub type LineMap = Vec<Option<DiffLineInfo>>;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffSide {
    Left,
    Right,
}

/// Information about a single rendered diff line, used to map clicks to GitHub review comments.
#[derive(Debug, Clone)]
pub struct DiffLineInfo {
    pub file_line: usize,  // 1-based line number in the file (for GitHub API)
    pub side: DiffSide,    // Left (base) or Right (head)
    pub commentable: bool, // On or near a changed line
}

/// A queued review comment that hasn't been submitted yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingComment {
    pub file_path: String,
    pub line: usize,                  // 1-based file line (end of range, or single line)
    pub start_line: Option<usize>,    // For range comments (start of range)
    pub side: DiffSide,
    pub body: String,
    /// If set, this is a reply to an existing thread (database ID of the top comment)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_comment_id: Option<u64>,
}

/// State machine for mouse drag interactions.
#[derive(Debug, Clone)]
pub enum DragState {
    Idle,
    Pressed {
        rendered_line: usize,
        #[allow(dead_code)]
        col: u16,
        #[allow(dead_code)]
        row: u16,
    },
    Dragging {
        start_rendered_line: usize,
        current_rendered_line: usize,
    },
}

/// Inline text editor for composing review comments.
#[derive(Debug, Clone)]
pub struct InlineEditor {
    pub lines: Vec<String>,
    pub cursor: (usize, usize),          // (line, col)
    pub anchor_rendered_line: usize,     // Where in the diff this editor is anchored
    pub target_file_path: String,
    pub target_line: usize,              // 1-based file line
    pub target_start_line: Option<usize>,
    pub target_side: DiffSide,
    pub editing_index: Option<usize>,    // If editing an existing comment
    /// If set, this editor is composing a reply to an existing thread
    pub reply_to_comment_id: Option<u64>,
}

impl InlineEditor {
    pub fn new(
        anchor_rendered_line: usize,
        target_file_path: String,
        target_line: usize,
        target_start_line: Option<usize>,
        target_side: DiffSide,
    ) -> Self {
        Self {
            lines: vec![String::new()],
            cursor: (0, 0),
            anchor_rendered_line,
            target_file_path,
            target_line,
            target_start_line,
            target_side,
            editing_index: None,
            reply_to_comment_id: None,
        }
    }

    /// Create an editor pre-filled with existing comment text.
    pub fn for_existing(
        anchor_rendered_line: usize,
        comment: &PendingComment,
        index: usize,
    ) -> Self {
        let lines: Vec<String> = comment.body.lines().map(|l| l.to_string()).collect();
        let last_line = lines.len().saturating_sub(1);
        let last_col = lines.last().map(|l| l.len()).unwrap_or(0);
        Self {
            lines: if lines.is_empty() { vec![String::new()] } else { lines },
            cursor: (last_line, last_col),
            anchor_rendered_line,
            target_file_path: comment.file_path.clone(),
            target_line: comment.line,
            target_start_line: comment.start_line,
            target_side: comment.side,
            editing_index: Some(index),
            reply_to_comment_id: comment.reply_to_comment_id,
        }
    }

    pub fn body(&self) -> String {
        self.lines.join("\n")
    }

    pub fn insert_char(&mut self, c: char) {
        let (line, col) = self.cursor;
        self.lines[line].insert(col, c);
        self.cursor.1 += 1;
    }

    pub fn insert_newline(&mut self) {
        let (line, col) = self.cursor;
        let rest = self.lines[line][col..].to_string();
        self.lines[line].truncate(col);
        self.lines.insert(line + 1, rest);
        self.cursor = (line + 1, 0);
    }

    pub fn backspace(&mut self) {
        let (line, col) = self.cursor;
        if col > 0 {
            self.lines[line].remove(col - 1);
            self.cursor.1 -= 1;
        } else if line > 0 {
            let removed = self.lines.remove(line);
            let prev_len = self.lines[line - 1].len();
            self.lines[line - 1].push_str(&removed);
            self.cursor = (line - 1, prev_len);
        }
    }

    pub fn delete(&mut self) {
        let (line, col) = self.cursor;
        if col < self.lines[line].len() {
            self.lines[line].remove(col);
        } else if line + 1 < self.lines.len() {
            let next = self.lines.remove(line + 1);
            self.lines[line].push_str(&next);
        }
    }

    pub fn move_left(&mut self) {
        let (line, col) = self.cursor;
        if col > 0 {
            self.cursor.1 -= 1;
        } else if line > 0 {
            self.cursor = (line - 1, self.lines[line - 1].len());
        }
    }

    pub fn move_right(&mut self) {
        let (line, col) = self.cursor;
        if col < self.lines[line].len() {
            self.cursor.1 += 1;
        } else if line + 1 < self.lines.len() {
            self.cursor = (line + 1, 0);
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self.cursor.1.min(self.lines[self.cursor.0].len());
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.0 + 1 < self.lines.len() {
            self.cursor.0 += 1;
            self.cursor.1 = self.cursor.1.min(self.lines[self.cursor.0].len());
        }
    }

    pub fn move_home(&mut self) {
        self.cursor.1 = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor.1 = self.lines[self.cursor.0].len();
    }
}

/// What a body editor is being used for.
#[derive(Debug, Clone)]
pub enum BodyEditorPurpose {
    /// Collecting review body for a submit (RequestChanges or Comment with no inline comments)
    ReviewBody(ReviewEvent),
    /// Composing a top-level PR issue comment
    IssueComment,
}

/// Popup text editor for composing review bodies or top-level PR comments.
#[derive(Debug, Clone)]
pub struct BodyEditor {
    pub lines: Vec<String>,
    pub cursor: (usize, usize), // (line, col)
    pub purpose: BodyEditorPurpose,
}

impl BodyEditor {
    pub fn new(purpose: BodyEditorPurpose) -> Self {
        Self {
            lines: vec![String::new()],
            cursor: (0, 0),
            purpose,
        }
    }

    pub fn body(&self) -> String {
        self.lines.join("\n")
    }

    pub fn insert_char(&mut self, c: char) {
        let (line, col) = self.cursor;
        self.lines[line].insert(col, c);
        self.cursor.1 += 1;
    }

    pub fn insert_newline(&mut self) {
        let (line, col) = self.cursor;
        let rest = self.lines[line][col..].to_string();
        self.lines[line].truncate(col);
        self.lines.insert(line + 1, rest);
        self.cursor = (line + 1, 0);
    }

    pub fn backspace(&mut self) {
        let (line, col) = self.cursor;
        if col > 0 {
            self.lines[line].remove(col - 1);
            self.cursor.1 -= 1;
        } else if line > 0 {
            let removed = self.lines.remove(line);
            let prev_len = self.lines[line - 1].len();
            self.lines[line - 1].push_str(&removed);
            self.cursor = (line - 1, prev_len);
        }
    }

    pub fn delete(&mut self) {
        let (line, col) = self.cursor;
        if col < self.lines[line].len() {
            self.lines[line].remove(col);
        } else if line + 1 < self.lines.len() {
            let next = self.lines.remove(line + 1);
            self.lines[line].push_str(&next);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor.1 > 0 {
            self.cursor.1 -= 1;
        } else if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self.lines[self.cursor.0].len();
        }
    }

    pub fn move_right(&mut self) {
        let (line, col) = self.cursor;
        if col < self.lines[line].len() {
            self.cursor.1 += 1;
        } else if line + 1 < self.lines.len() {
            self.cursor = (line + 1, 0);
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self.cursor.1.min(self.lines[self.cursor.0].len());
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.0 + 1 < self.lines.len() {
            self.cursor.0 += 1;
            self.cursor.1 = self.cursor.1.min(self.lines[self.cursor.0].len());
        }
    }

    pub fn move_home(&mut self) {
        self.cursor.1 = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor.1 = self.lines[self.cursor.0].len();
    }
}

/// An existing review thread from GitHub (inline comment thread on a file).
#[derive(Debug, Clone)]
pub struct ReviewThread {
    pub is_resolved: bool,
    pub path: String,
    pub line: usize,
    pub start_line: Option<usize>,
    pub diff_side: DiffSide,
    pub comments: Vec<ThreadComment>,
}

impl ReviewThread {
    /// Database ID of the thread's first (top-level) comment, used for replies.
    pub fn first_comment_id(&self) -> Option<u64> {
        self.comments.first().and_then(|c| if c.id > 0 { Some(c.id) } else { None })
    }
}

/// A single comment within a review thread.
#[derive(Debug, Clone)]
pub struct ThreadComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// A general PR discussion comment (not attached to code).
#[derive(Debug, Clone)]
pub struct PrComment {
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// Container for all existing comments fetched from GitHub.
#[derive(Debug, Clone)]
pub struct PrComments {
    pub threads: Vec<ReviewThread>,
    pub comments: Vec<PrComment>,
}

/// Review event type for GitHub API.
#[derive(Debug, Clone)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Comment => "COMMENT",
        }
    }
}

/// Top-level review state held by App.
pub struct ReviewState {
    pub comments: Vec<PendingComment>,
    pub drag: DragState,
    pub inline_editor: Option<InlineEditor>,
    pub body_editor: Option<BodyEditor>,
    pub submit_mode: bool,
    // PR context for submitting reviews
    pub repo: Option<String>,
    pub pr_number: Option<u64>,
    pub head_sha: Option<String>,
}

impl ReviewState {
    pub fn new() -> Self {
        Self {
            comments: Vec::new(),
            drag: DragState::Idle,
            inline_editor: None,
            body_editor: None,
            submit_mode: false,
            repo: None,
            pr_number: None,
            head_sha: None,
        }
    }

    /// Find the index of a pending comment for a given file/line.
    pub fn find_comment_at(&self, file_path: &str, file_line: usize, side: DiffSide) -> Option<usize> {
        self.comments.iter().position(|c| {
            c.file_path == file_path && c.line == file_line && c.side == side
        })
    }

    /// Count comments for the currently viewed file.
    pub fn comments_for_file(&self, file_path: &str) -> Vec<(usize, &PendingComment)> {
        self.comments
            .iter()
            .enumerate()
            .filter(|(_, c)| c.file_path == file_path)
            .collect()
    }

    /// Save pending comments to disk cache, keyed by repo+PR.
    pub fn save_to_disk(&self) {
        let (repo, pr_number) = match (&self.repo, self.pr_number) {
            (Some(r), Some(n)) => (r, n),
            _ => return,
        };
        let dir = comments_cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let filename = format!("{}__{}.json", repo.replace('/', "__"), pr_number);
        let path = dir.join(filename);
        if self.comments.is_empty() {
            let _ = std::fs::remove_file(path);
        } else if let Ok(json) = serde_json::to_string(&self.comments) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Load pending comments from disk cache for a given repo+PR.
    pub fn load_from_disk(&mut self) {
        let (repo, pr_number) = match (&self.repo, self.pr_number) {
            (Some(r), Some(n)) => (r, n),
            _ => return,
        };
        let dir = comments_cache_dir();
        let filename = format!("{}__{}.json", repo.replace('/', "__"), pr_number);
        let path = dir.join(filename);
        self.comments.clear();
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(comments) = serde_json::from_str::<Vec<PendingComment>>(&data) {
                self.comments = comments;
            }
        }
    }
}

fn comments_cache_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"))
        .join("prfait")
        .join("comments")
}
