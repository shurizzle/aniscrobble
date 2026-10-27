mod app;
mod cache;
mod model;
mod net;
mod term;
mod ui;

use std::collections::HashMap;
use std::path::Path;
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
use crate::tui::model::MediaKind;
use crate::tui::net::NetMsg;

#[tokio::main]
pub async fn run() -> Result<()> {
    // The picker queries the terminal (graphics protocol, font size) and needs
    // the terminal in its normal state: do it before entering raw mode.
    let picker = Arc::new(build_picker());
    let (tx, mut rx) = mpsc::channel::<NetMsg>(64);
    let (db, token, viewer, progress) = stored_account();
    let (config, config_error) = crate::config::Config::load();
    let mut app = App::with_account(picker, tx, db, token, viewer, progress);
    app.external_command = config.external.command;
    app.error = config_error;

    let mut terminal = term::init();
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let result = loop {
        match event_loop(&mut terminal, &mut app, &mut events, &mut rx, &mut ticker).await {
            Ok(Outcome::Quit) => break Ok(()),
            Ok(Outcome::External(kind, id)) => {
                // The event reader thread would steal the child's stdin: drop
                // it before handing the terminal over.
                drop(events);
                term::suspend();
                if let Some(command) = app.external_command.clone()
                    && let Err(err) = run_external(&command, kind, id).await
                {
                    app.error = Some(err);
                }
                terminal = term::resume();
                events = EventStream::new();
                app.dirty = true;
            }
            Err(err) => break Err(err),
        }
    };
    term::restore();
    result
}

enum Outcome {
    Quit,
    /// Suspend the TUI and run the configured command for this media.
    External(MediaKind, i64),
}

/// Run the external command with the media type and id, inheriting the
/// terminal so the script owns it completely. A failure waits for Enter before
/// the TUI comes back, so the error stays readable.
async fn run_external(command: &Path, kind: MediaKind, id: i64) -> Result<(), String> {
    let command = command.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let status = std::process::Command::new(&command)
            .arg(kind.label())
            .arg(id.to_string())
            .status();
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(pause(&format!("external command exited with {status}"))),
            Err(err) => Err(pause(&format!("cannot run external command: {err}"))),
        }
    })
    .await
    .map_err(|err| format!("external command task failed: {err}"))?
}

/// Print the failure and block until the user presses Enter. The TUI is still
/// suspended here, so the message and the script's output are on screen.
fn pause(message: &str) -> String {
    use std::io::{BufRead, Write};

    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "\n{message}\nPress ENTER to continue");
    let _ = stdout.flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    message.to_string()
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    events: &mut EventStream,
    rx: &mut mpsc::Receiver<NetMsg>,
    ticker: &mut Interval,
) -> Result<Outcome> {
    loop {
        if app.dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.dirty = false;
        }
        if !app.running {
            return Ok(Outcome::Quit);
        }

        tokio::select! {
            biased;
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => app.on_key(key),
                Some(Ok(Event::Mouse(mouse))) => app.on_mouse(mouse),
                Some(Ok(Event::Resize(_, _))) => app.dirty = true,
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.into()),
                None => return Ok(Outcome::Quit),
            },
            Some(msg) = rx.recv() => app.on_net(msg),
            _ = ticker.tick() => {
                if app.tick() {
                    app.dirty = true;
                }
            }
        }

        if let Some((kind, id)) = app.pending_external.take() {
            return Ok(Outcome::External(kind, id));
        }
    }
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
