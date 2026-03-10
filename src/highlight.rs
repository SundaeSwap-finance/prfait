use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::path::Path;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    /// Highlight source code lines with syntax coloring + diff background
    pub fn highlight_diff_lines(
        &self,
        file_path: &str,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Vec<Line<'static>> {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt");

        let syntax = self
            .syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut lines = Vec::new();

        // For now, simple before/after rendering with syntax-aware coloring
        // TODO: integrate syntect-tui for proper token-level highlighting
        if let Some(before) = before {
            let mut h =
                syntect::easy::HighlightLines::new(syntax, theme);
            for line in before.lines() {
                let highlighted = h.highlight_line(line, &self.syntax_set).unwrap_or_default();
                let spans: Vec<Span<'static>> = highlighted
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(
                            text.to_string(),
                            Style::default()
                                .fg(syntect_to_ratatui_color(style.foreground))
                                .bg(Color::Rgb(40, 0, 0)),
                        )
                    })
                    .collect();
                let mut full = vec![Span::styled(
                    "- ",
                    Style::default().fg(Color::Red).bg(Color::Rgb(40, 0, 0)),
                )];
                full.extend(spans);
                lines.push(Line::from(full));
            }
        }

        if let Some(after) = after {
            let mut h =
                syntect::easy::HighlightLines::new(syntax, theme);
            for line in after.lines() {
                let highlighted = h.highlight_line(line, &self.syntax_set).unwrap_or_default();
                let spans: Vec<Span<'static>> = highlighted
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(
                            text.to_string(),
                            Style::default()
                                .fg(syntect_to_ratatui_color(style.foreground))
                                .bg(Color::Rgb(0, 30, 0)),
                        )
                    })
                    .collect();
                let mut full = vec![Span::styled(
                    "+ ",
                    Style::default()
                        .fg(Color::Green)
                        .bg(Color::Rgb(0, 30, 0)),
                )];
                full.extend(spans);
                lines.push(Line::from(full));
            }
        }

        lines
    }
}

fn syntect_to_ratatui_color(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}
