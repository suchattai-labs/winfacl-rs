//! Terminal session: raw mode + alternate screen with restore on drop
//! and on panic, so a crash never leaves the shell unusable.

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io::{self, Stdout};

pub type Backend = CrosstermBackend<Stdout>;

pub struct Term {
    pub terminal: Terminal<Backend>,
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

impl Term {
    pub fn new() -> io::Result<Term> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;

        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            default_hook(info);
        }));

        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Term { terminal })
    }

    /// Next key press (repeats included, releases filtered). `None` on
    /// EOF/lost terminal -- callers treat that as quit. Resize events
    /// surface as `Some(Key::Resize)` so layouts can rebuild.
    pub fn next_key(&mut self) -> Option<Key> {
        loop {
            match event::read() {
                Ok(Event::Key(KeyEvent { code, modifiers, kind, .. }))
                    if kind != KeyEventKind::Release =>
                {
                    return Some(Key::Press(code, modifiers));
                }
                Ok(Event::Resize(..)) => return Some(Key::Resize),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Press(KeyCode, KeyModifiers),
    Resize,
}

impl Key {
    pub fn ch(c: char) -> Key {
        Key::Press(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub fn is_char(&self, c: char) -> bool {
        matches!(self, Key::Press(KeyCode::Char(x), m)
                 if *x == c && !m.contains(KeyModifiers::CONTROL))
    }

    pub fn code(&self) -> Option<KeyCode> {
        match self {
            Key::Press(c, _) => Some(*c),
            Key::Resize => None,
        }
    }
}

/// A centered popup rect no larger than the frame allows.
pub fn popup_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}
