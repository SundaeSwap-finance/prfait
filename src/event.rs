use crossterm::event::{EventStream, KeyEventKind};
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub enum Event {
    Key(crossterm::event::KeyEvent),
    Mouse(crossterm::event::MouseEvent),
    Resize(u16, u16),
    Tick,
    Render,
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<Event>,
    _task: JoinHandle<()>,
}

impl EventHandler {
    pub fn new(tick_rate: Duration, render_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_rate);
            let mut render_interval = tokio::time::interval(render_rate);

            loop {
                let crossterm_event = reader.next();
                let tick_delay = tick_interval.tick();
                let render_delay = render_interval.tick();

                tokio::select! {
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(crossterm::event::Event::Key(k))) if k.kind == KeyEventKind::Press => {
                                if tx.send(Event::Key(k)).is_err() { return; }
                            }
                            Some(Ok(crossterm::event::Event::Mouse(m))) => {
                                if tx.send(Event::Mouse(m)).is_err() { return; }
                            }
                            Some(Ok(crossterm::event::Event::Resize(w, h))) => {
                                if tx.send(Event::Resize(w, h)).is_err() { return; }
                            }
                            Some(Err(_)) | None => return,
                            _ => {}
                        }
                    }
                    _ = tick_delay => {
                        if tx.send(Event::Tick).is_err() { return; }
                    }
                    _ = render_delay => {
                        if tx.send(Event::Render).is_err() { return; }
                    }
                }
            }
        });

        Self { rx, _task: task }
    }

    pub async fn next(&mut self) -> color_eyre::Result<Event> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("Event channel closed"))
    }
}
