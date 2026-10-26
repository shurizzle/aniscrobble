use std::io::stdout;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui::DefaultTerminal;

/// Enter alternate screen and raw mode. The returned terminal restores the
/// previous terminal state on drop, and a panic hook restores it before the
/// panic message is printed.
pub fn init() -> DefaultTerminal {
    let terminal = ratatui::init();
    // Page rows are clickable, so the terminal must report mouse events.
    let _ = execute!(stdout(), EnableMouseCapture);

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        previous(info);
    }));
    terminal
}

pub fn restore() {
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
}
