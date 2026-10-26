mod app;
mod cache;
mod model;
mod net;
mod term;
mod ui;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui_image::picker::{Picker, ProtocolType};
use tokio::sync::mpsc;
use tokio::time::{Interval, MissedTickBehavior};

use crate::tui::app::App;
use crate::tui::net::NetMsg;

#[tokio::main]
pub async fn run() -> Result<()> {
    // The picker queries the terminal (graphics protocol, font size) and needs
    // the terminal in its normal state: do it before entering raw mode.
    let picker = Arc::new(build_picker());
    let (tx, mut rx) = mpsc::channel::<NetMsg>(64);
    let (db, token, viewer, progress) = stored_account();
    let mut app = App::with_account(picker, tx, db, token, viewer, progress);

    let mut terminal = term::init();
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let result = event_loop(&mut terminal, &mut app, &mut events, &mut rx, &mut ticker).await;
    term::restore();
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    events: &mut EventStream,
    rx: &mut mpsc::Receiver<NetMsg>,
    ticker: &mut Interval,
) -> Result<()> {
    loop {
        if app.dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.dirty = false;
        }
        if !app.running {
            break;
        }

        tokio::select! {
            biased;
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => app.on_key(key),
                Some(Ok(Event::Mouse(mouse))) => app.on_mouse(mouse),
                Some(Ok(Event::Resize(_, _))) => app.dirty = true,
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.into()),
                None => break,
            },
            Some(msg) = rx.recv() => app.on_net(msg),
            _ = ticker.tick() => {
                if app.tick() {
                    app.dirty = true;
                }
            }
        }
    }

    Ok(())
}

fn stored_account() -> (
    Option<crate::database::Database>,
    Option<String>,
    Option<u64>,
    HashMap<i64, i64>,
) {
    let Some(db) = crate::database::Database::new().ok() else {
        return (None, None, None, HashMap::new());
    };
    let login = db.login().ok().flatten();
    let viewer = login.as_ref().map(|user| user.id);
    let token = login.map(|user| user.token);
    let progress = db
        .progresses()
        .map(|progress| {
            progress
                .into_iter()
                .map(|(id, episode)| (id as i64, episode as i64))
                .collect()
        })
        .unwrap_or_default();
    (Some(db), token, viewer, progress)
}

fn build_picker() -> Picker {
    let mut picker = match Picker::from_query_stdio() {
        Ok(picker) => picker,
        Err(_) => Picker::halfblocks(),
    };

    if let Ok(value) = std::env::var("ANISCROBBLE_TUI_PROTOCOL") {
        let forced = match value.as_str() {
            "halfblocks" => Some(ProtocolType::Halfblocks),
            "sixel" => Some(ProtocolType::Sixel),
            "kitty" => Some(ProtocolType::Kitty),
            "iterm2" => Some(ProtocolType::Iterm2),
            _ => None,
        };
        if let Some(forced) = forced {
            picker.set_protocol_type(forced);
        }
    }

    picker
}
