use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::Action;
use crate::components::Component;

pub struct StatusBar {
    message: Option<(String, MessageKind)>,
    analyzing: Option<String>,
    tick: u8,
    pub review_count: usize,
    pub submit_mode: bool,
}

enum MessageKind {
    Error,
    Info,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            message: None,
            analyzing: None,
            tick: 0,
            review_count: 0,
            submit_mode: false,
        }
    }
}

impl Component for StatusBar {
    fn handle_key_event(&mut self, _key: KeyEvent) -> Action {
        Action::Noop
    }

    fn update(&mut self, action: &Action) -> Action {
        match action {
            Action::LoadError(msg) => {
                self.message = Some((msg.clone(), MessageKind::Error));
            }
            Action::PrsLoaded(_, _) => {
                // Clear error if any
                if matches!(self.message, Some((_, MessageKind::Error))) {
                    self.message = None;
                }
            }
            Action::AnalyzePr(repo, pr) => {
                self.analyzing = Some(format!("{repo}#{pr}"));
            }
            Action::AnalysisComplete(repo, pr, result) => {
                if self.analyzing.as_deref() == Some(&format!("{repo}#{pr}")) {
                    self.analyzing = None;
                }
                self.message = Some((
                    format!(
                        "{repo}#{pr}: {} entities, {}ms",
                        result.stats.total_entities, result.timing.total_ms
                    ),
                    MessageKind::Info,
                ));
            }
            Action::ReviewSubmitted(url) => {
                self.message = Some((format!("Review submitted: {url}"), MessageKind::Info));
            }
            Action::ReviewError(msg) => {
                self.message = Some((msg.clone(), MessageKind::Error));
            }
            Action::Tick => {
                self.tick = self.tick.wrapping_add(1);
            }
            _ => {}
        }
        Action::Noop
    }

    fn render(&mut self, frame: &mut Frame, area: Rect) {
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spin_char = spinner[(self.tick as usize / 2) % spinner.len()];

        let line = if self.submit_mode {
            Line::from(vec![
                Span::styled(" Submit review: ", Style::default().fg(Color::Yellow)),
                Span::styled("[a]", Style::default().fg(Color::Green)),
                Span::raw("pprove "),
                Span::styled("[r]", Style::default().fg(Color::Red)),
                Span::raw("equest changes "),
                Span::styled("[c]", Style::default().fg(Color::Cyan)),
                Span::raw("omment "),
                Span::styled("[Esc]", Style::default().fg(Color::DarkGray)),
                Span::raw(" cancel"),
            ])
        } else if let Some(ref target) = self.analyzing {
            Line::from(vec![Span::styled(
                format!(" {spin_char} Analyzing {target}..."),
                Style::default().fg(Color::Yellow),
            )])
        } else if let Some((ref msg, ref kind)) = self.message {
            let color = match kind {
                MessageKind::Error => Color::Red,
                MessageKind::Info => Color::DarkGray,
            };
            Line::from(vec![Span::styled(
                format!(" {msg}"),
                Style::default().fg(color),
            )])
        } else {
            let mut spans = vec![
                Span::styled(" Tab/click", Style::default().fg(Color::Cyan)),
                Span::raw(":panel "),
                Span::styled("j/k/scroll", Style::default().fg(Color::Cyan)),
                Span::raw(":nav "),
                Span::styled("Enter", Style::default().fg(Color::Cyan)),
                Span::raw(":expand "),
                Span::styled("d", Style::default().fg(Color::Cyan)),
                Span::raw(":side-by-side "),
                Span::styled("x", Style::default().fg(Color::Cyan)),
                Span::raw(":reviewed "),
                Span::styled("r", Style::default().fg(Color::Cyan)),
                Span::raw(":refresh "),
                Span::styled("?", Style::default().fg(Color::Cyan)),
                Span::raw(":help "),
                Span::styled("Esc/q", Style::default().fg(Color::Cyan)),
                Span::raw(":quit"),
            ];
            if self.review_count > 0 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("{} comment{}", self.review_count, if self.review_count == 1 { "" } else { "s" }),
                    Style::default().fg(Color::Yellow),
                ));
                spans.push(Span::raw(" | "));
                spans.push(Span::styled("Ctrl+R", Style::default().fg(Color::Cyan)));
                spans.push(Span::raw(":submit"));
            }
            Line::from(spans)
        };

        let bar = Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 40)));
        frame.render_widget(bar, area);
    }
}
