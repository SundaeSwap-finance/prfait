mod action;
mod analysis;
mod app;
mod checks;
mod components;
mod config;
mod event;
mod github;
mod highlight;
mod onboarding;
mod review;
mod structural_diff;

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::MouseEventKind;
use tokio::sync::mpsc;

use action::Action;
use app::App;
use config::Config;
use event::{Event, EventHandler};
use github::GithubClient;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Load config, running onboarding wizard if no repos configured
    let mut config = Config::load()?;
    if config.repos.is_empty() {
        config = onboarding::run_onboarding()?;
    }

    // Resolve GitHub token
    let token = GithubClient::resolve_token(config.github_token.as_deref())?;
    let github_client = Arc::new(GithubClient::new(&token)?);

    // Set GITHUB_TOKEN env for inspect-core's GitHubClient
    std::env::set_var("GITHUB_TOKEN", &token);

    let editor_cmd = config.resolve_editor();

    let (action_tx, mut action_rx) = mpsc::unbounded_channel::<Action>();

    let mut app = App::new(config, action_tx.clone(), github_client);

    // Start terminal with mouse support
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture
    )?;
    let mut terminal = ratatui::init();
    let mut events = EventHandler::new(Duration::from_millis(250), Duration::from_millis(33));

    // Kick off PR loading
    app.start_loading_prs();

    loop {
        tokio::select! {
            event = events.next() => {
                let action = match event? {
                    Event::Key(key) => app.handle_key_event(key),
                    Event::Mouse(mouse) => match mouse.kind {
                        // Scroll wheel always controls the diff panel
                        MouseEventKind::ScrollUp => Action::ScrollUp(3),
                        MouseEventKind::ScrollDown => Action::ScrollDown(3),
                        MouseEventKind::ScrollLeft => Action::ScrollLeft(4),
                        MouseEventKind::ScrollRight => Action::ScrollRight(4),
                        // Click to focus panel + select / start drag
                        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                            app.handle_mouse_click(mouse.column, mouse.row)
                        }
                        // Drag for range selection
                        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                            Action::DiffMouseDrag(mouse.column, mouse.row)
                        }
                        // Mouse up to complete click/drag
                        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                            app.handle_mouse_up_event(mouse.column, mouse.row)
                        }
                        _ => Action::Noop,
                    },
                    Event::Resize(w, h) => Action::Resize(w, h),
                    Event::Tick => Action::Tick,
                    Event::Render => Action::Render,
                };
                let _ = action_tx.send(action);
            }
            Some(action) = action_rx.recv() => {
                // Handle editor suspension before normal update
                if let Action::SuspendForEditor(ref temp_path, line_number, ref original_content) = action {
                    suspend_for_editor(
                        &mut terminal,
                        &editor_cmd,
                        temp_path,
                        line_number,
                        original_content,
                        &mut app,
                    );
                    continue;
                }

                let follow_up = app.update(&action);
                if !matches!(follow_up, Action::Noop) {
                    let _ = action_tx.send(follow_up);
                }

                if matches!(action, Action::Render) {
                    terminal.draw(|frame| app.render(frame))?;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture
    )?;
    ratatui::restore();
    Ok(())
}

/// Suspend the TUI, launch an external editor, then resume and generate suggestion comments.
fn suspend_for_editor(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    editor_cmd: &str,
    temp_path: &str,
    line_number: usize,
    original_content: &str,
    app: &mut App,
) {
    // Leave TUI mode
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    );

    // Launch editor
    let status = std::process::Command::new(editor_cmd)
        .arg(format!("+{line_number}"))
        .arg(temp_path)
        .status();

    // Restore TUI mode
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    );
    let _ = crossterm::terminal::enable_raw_mode();
    let _ = terminal.clear();

    // Generate suggestion comments from edits
    if let Ok(exit_status) = status {
        if exit_status.success() {
            if let Ok(edited) = std::fs::read_to_string(temp_path) {
                generate_suggestion_comments(&edited, original_content, app);
            }
        }
    }

    // Clean up temp file
    let _ = std::fs::remove_file(temp_path);
}

/// Diff the edited file against the original and generate suggestion comments.
fn generate_suggestion_comments(edited: &str, original: &str, app: &mut App) {
    use similar::{ChangeTag, TextDiff};

    if edited == original {
        return;
    }

    let file_path = match app.diff_panel.current_file() {
        Some(f) => f.to_string(),
        None => return,
    };

    let diff = TextDiff::from_lines(original, edited);
    let mut hunks = Vec::new();

    // Collect changed hunks
    for hunk in diff.unified_diff().context_radius(0).iter_hunks() {
        let mut old_start = None;
        let mut old_end = None;
        let mut new_lines = Vec::new();

        for change in hunk.iter_changes() {
            match change.tag() {
                ChangeTag::Delete => {
                    let idx = change.old_index().unwrap_or(0);
                    if old_start.is_none() {
                        old_start = Some(idx + 1); // 1-based
                    }
                    old_end = Some(idx + 1);
                }
                ChangeTag::Insert => {
                    new_lines.push(change.value().to_string());
                }
                ChangeTag::Equal => {}
            }
        }

        if let (Some(start), Some(end)) = (old_start, old_end) {
            hunks.push((start, end, new_lines));
        }
    }

    for (start, end, new_lines) in hunks {
        let suggestion_body = if new_lines.is_empty() {
            // Deletion suggestion (empty suggestion block)
            "```suggestion\n```".to_string()
        } else {
            let joined: String = new_lines.join("");
            format!("```suggestion\n{}```", joined)
        };

        let comment = review::PendingComment {
            file_path: file_path.clone(),
            line: end,
            start_line: if start < end { Some(start) } else { None },
            side: review::DiffSide::Right,
            body: suggestion_body,
            reply_to_comment_id: None,
        };
        app.review.comments.push(comment);
    }

    app.status_bar.review_count = app.review.comments.len();
    if let (Some(repo), Some(pr)) = (&app.review.repo, app.review.pr_number) {
        app.pr_panel.set_comment_counts(repo, pr, &app.review.comments);
    }
    app.review.save_to_disk();
}
