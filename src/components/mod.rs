pub mod diff_panel;
pub mod pr_panel;
pub mod status_bar;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::action::Action;

pub trait Component {
    fn handle_key_event(&mut self, key: KeyEvent) -> Action;
    fn update(&mut self, action: &Action) -> Action;
    fn render(&mut self, frame: &mut Frame, area: Rect);
}
