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
    ScrollLeft(u16),
    ScrollRight(u16),
    ScrollToTop,
    ScrollToBottom,

    // Data events
    PrsLoaded(String, Vec<PrData>),
    AnalysisComplete(String, u64, Box<ReviewResult>),
    CommentsLoaded(String, u64, Box<PrComments>),
    LoadError(String),

    // Diff cursor navigation
    CursorDown,
    CursorUp,
    CursorComment,

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
    ReviewSubmitted(String),
    ReviewError(String),

    // Review: external editor
    OpenInEditor,
    SuspendForEditor(String, usize, String), // temp_path, line_number, original_content

    // Triggers
    RefreshPrs,
    AnalyzePr(String, u64),
    ToggleDiffMode,

    Tick,
    Render,
    Resize(u16, u16),
    Noop,
}
