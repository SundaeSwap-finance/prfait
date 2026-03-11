use crate::checks;
use crate::github::PrData;
use crate::review::{PrComments, ReviewEvent};
use inspect_core::types::ReviewResult;

#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    FocusNext,

    // Tree navigation
    TreeUp,
    TreeDown,
    TreeLeft,
    TreeRight,
    TreeClick(u16, u16),

    // Diff navigation
    ScrollUp(u16),
    ScrollDown(u16),
    ScrollHalfPageUp,
    ScrollHalfPageDown,
    ScrollLeft(u16),
    ScrollRight(u16),
    ScrollToTop,
    ScrollToBottom,

    // Search
    SearchNext,
    SearchPrev,

    // Data events
    PrsLoaded(String, Vec<PrData>),
    AnalysisComplete(String, u64, Box<ReviewResult>),
    CommentsLoaded(String, u64, Box<PrComments>),
    LoadError(String),

    // Diff cursor navigation
    CursorDown,
    CursorUp,
    CursorComment,
    JumpNextHunk,
    JumpPrevHunk,

    // Diff interaction
    DiffClick(u16, u16),
    NavigateToFile(String, u64, String), // repo, pr_number, file_path
    MarkFileReviewed(String, u64, String), // repo, pr_number, file_path

    // Review: mouse interaction
    DiffMouseDown(u16, u16),
    DiffMouseUp(u16, u16),
    DiffMouseDrag(u16, u16),

    // Review: inline editor
    SaveComment(String),
    CancelComment,
    DeleteComment(usize),

    // Review: submission
    OpenReviewSubmit,
    SubmitReview(ReviewEvent),
    SubmitReviewWithBody(ReviewEvent, String),
    ReviewSubmitted(String),
    ReviewError(String),

    // Top-level PR comment
    PostIssueComment(String),
    IssueCommentPosted,
    IssueCommentError(String),

    // Review: external editor
    OpenInEditor,
    SuspendForEditor(String, usize, String), // temp_path, line_number, original_content

    // Checks
    ChecksStarted(String, u64, String),                   // repo, pr_number, sha
    ChecksUpdate(String, u64, Vec<checks::CheckResult>),  // repo, pr_number, partial results
    ChecksComplete(String, u64, checks::PrCheckState),    // repo, pr_number, final state

    // Triggers
    RefreshPrs,
    AnalyzePr(String, u64),
    ToggleDiffMode,

    Tick,
    Render,
    Resize(u16, u16),
    Noop,
}
