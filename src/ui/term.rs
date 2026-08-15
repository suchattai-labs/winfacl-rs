//! Terminal session: raw mode + alternate screen + mouse capture, with
//! restore on drop and on panic, so a crash never leaves the shell
//! unusable.

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

pub type Backend = CrosstermBackend<Stdout>;

pub struct Term {
    pub terminal: Terminal<Backend>,
    last_click: Option<(u16, u16, Instant)>,
}

fn restore_terminal() {
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

impl Term {
    pub fn new() -> io::Result<Term> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;

        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            default_hook(info);
        }));

        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Term {
            terminal,
            last_click: None,
        })
    }

    /// Next input event. Key releases and mouse moves are filtered;
    /// left clicks become `Click`/`DoubleClick`, the wheel becomes
    /// `Scroll`. `None` on EOF/lost terminal -- callers treat that as
    /// quit. Resizes surface so layouts can rebuild.
    pub fn next_key(&mut self) -> Option<Key> {
        loop {
            match event::read() {
                Ok(Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    ..
                })) if kind != KeyEventKind::Release => {
                    return Some(Key::Press(code, modifiers));
                }
                Ok(Event::Mouse(MouseEvent {
                    kind, column, row, ..
                })) => match kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let now = Instant::now();
                        let double = self.last_click.is_some_and(|(x, y, t)| {
                            x == column
                                && y == row
                                && now.duration_since(t) < Duration::from_millis(400)
                        });
                        self.last_click = if double {
                            None
                        } else {
                            Some((column, row, now))
                        };
                        return Some(if double {
                            Key::DoubleClick(column, row)
                        } else {
                            Key::Click(column, row)
                        });
                    }
                    MouseEventKind::ScrollUp => return Some(Key::Scroll(-1)),
                    MouseEventKind::ScrollDown => return Some(Key::Scroll(1)),
                    _ => continue,
                },
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
    Click(u16, u16),
    DoubleClick(u16, u16),
    Scroll(i8),
    Resize,
}

impl Key {
    pub fn is_char(&self, c: char) -> bool {
        matches!(self, Key::Press(KeyCode::Char(x), m)
                 if *x == c && !m.contains(KeyModifiers::CONTROL))
    }

    pub fn code(&self) -> Option<KeyCode> {
        match self {
            Key::Press(c, _) => Some(*c),
            _ => None,
        }
    }

    /// Click or double-click position, if any.
    pub fn pos(&self) -> Option<(u16, u16)> {
        match self {
            Key::Click(x, y) | Key::DoubleClick(x, y) => Some((*x, *y)),
            _ => None,
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

pub fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_edges() {
        let r = Rect {
            x: 2,
            y: 3,
            width: 4,
            height: 2,
        };
        assert!(rect_contains(r, 2, 3));
        assert!(rect_contains(r, 5, 4));
        assert!(!rect_contains(r, 6, 4)); // one past the right edge
        assert!(!rect_contains(r, 5, 5)); // one past the bottom edge
        assert!(!rect_contains(r, 1, 3));
    }
}
