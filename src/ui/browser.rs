//! The two-panel SetACL-Studio-style view: filesystem tree on the left,
//! the permission editor embedded on the right with live preview.
//! Port of the C `browser.c`.

use super::dialogs;
use super::editor::{EditorEvent, EditorState};
use super::term::{Key, Term};
use crate::acl::facade::{LoadStatus, Model};
use crate::tree::{NodeId, Tree};
use crossterm::event::KeyCode;
use ratatui::{prelude::*, widgets::Paragraph};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Browser {
    tree: Tree,
    flat: Vec<NodeId>,
    sel: usize,
    top: usize,
    follow: bool,
    focus_editor: bool,

    model: Option<Model>,
    model_path: PathBuf,
    model_err: String,
    editor: EditorState,
}

impl Browser {
    fn reflatten(&mut self) {
        let cur = self.flat.get(self.sel).copied();
        self.flat = self.tree.flatten();
        self.sel = cur
            .and_then(|c| self.flat.iter().position(|&i| i == c))
            .unwrap_or(0);
    }

    fn load_selection(&mut self) {
        let Some(&id) = self.flat.get(self.sel) else {
            return;
        };
        let path = self.tree.path(id);
        if self.model.is_some() && path == self.model_path {
            return;
        }
        self.model_path = path.clone();
        let m = Model::load(&path, self.follow);
        match m.status {
            LoadStatus::Ok | LoadStatus::NotSup => {
                self.editor = EditorState::new(true);
                self.editor.rows_build(&m);
                if !m.acl_supported {
                    self.editor
                        .set_status(true, "Read-only: no ACL support on this filesystem.");
                }
                self.model = Some(m);
            }
            _ => {
                self.model = None;
                self.model_err = match m.status {
                    LoadStatus::NoEnt => "No such file or directory.".into(),
                    LoadStatus::Denied => "Permission denied.".into(),
                    _ => m
                        .load_errno
                        .map_or_else(|| "I/O error".into(), |e| e.to_string()),
                };
            }
        }
    }

    fn dirty(&self) -> bool {
        self.model.as_ref().is_some_and(|m| m.dirty())
    }

    /// Ask before the cursor leaves an object with staged changes.
    fn may_leave(&mut self, term: &mut Term, bg: dialogs::Background) -> bool {
        if !self.dirty() {
            return true;
        }
        if dialogs::confirm(
            term,
            bg,
            "Unsaved changes",
            "This object has staged permission changes.\n\nDiscard them and move on?",
        ) {
            if let Some(m) = &mut self.model {
                m.revert();
                self.editor.rows_build(m);
            }
            return true;
        }
        false
    }

    fn tree_lines(&mut self, area: Rect) -> Vec<Line<'static>> {
        let header = Style::new().bold().fg(Color::Black).bg(Color::Gray);
        let selected = if self.focus_editor {
            header
        } else {
            Style::new().bold().fg(Color::Black).bg(Color::Cyan)
        };
        let w = area.width as usize;
        let h = (area.height as usize).saturating_sub(1);

        if self.sel < self.top {
            self.top = self.sel;
        }
        if self.sel >= self.top + h.max(1) {
            self.top = self.sel + 1 - h.max(1);
        }

        let mut lines = vec![Line::styled(format!("{:<w$}", " Filesystem"), header)];
        for (k, &id) in self.flat.iter().enumerate().skip(self.top).take(h) {
            let n = self.tree.node(id);
            let depth = self.tree.depth(id).min(w.saturating_sub(8) / 2);
            let marker = if n.is_dir {
                if n.expanded {
                    "v"
                } else {
                    ">"
                }
            } else {
                " "
            };
            let mut name = n.name.clone();
            if n.is_symlink {
                name.push('@');
            }
            if n.load_failed {
                name.push_str(" !");
            }
            let body = format!("{:indent$}{marker} {name}", "", indent = 1 + 2 * depth);
            // clip to panel width on character boundaries
            let body: String = body.chars().take(w).collect();
            let style = if k == self.sel {
                selected
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("{body:<w$}"), style));
        }
        lines
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        let title = Style::new().bold().fg(Color::White).bg(Color::Blue);
        let tree_w = (area.width * 35 / 100).clamp(24, 44).min(area.width);

        let hint = "Tab Edit   q Quit";
        let bar = format!(
            " winfacl {:<w$}{hint} ",
            self.model_path.display(),
            w = (area.width as usize).saturating_sub(hint.len() + 11)
        );
        f.render_widget(
            Paragraph::new(Line::styled(bar, title)),
            Rect { height: 1, ..area },
        );

        let below = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        let tree_area = Rect {
            width: tree_w,
            ..below
        };
        let ed_area = Rect {
            x: below.x + tree_w,
            width: below.width.saturating_sub(tree_w),
            ..below
        };

        let tl = self.tree_lines(tree_area);
        f.render_widget(Paragraph::new(tl), tree_area);

        if let Some(m) = &self.model {
            let mut ed = std::mem::replace(&mut self.editor, EditorState::new(true));
            ed.render(f, ed_area, m, self.focus_editor);
            self.editor = ed;
        } else {
            f.render_widget(
                Paragraph::new(format!(
                    "\n  Cannot read this object:\n\n   {}",
                    self.model_err
                )),
                ed_area,
            );
        }
    }

    /// Owned snapshot of the whole two-panel frame for dialog backgrounds.
    fn snapshot(&mut self, term: &mut Term) -> impl FnMut(&mut Frame) + 'static {
        let mut buf: Vec<Line<'static>> = Vec::new();
        let area = term.terminal.get_frame().area();
        // Render into a scratch buffer by reusing render() on a real frame
        // is not possible outside draw(); rebuild the two panels as lines.
        let tree_w = (area.width * 35 / 100).clamp(24, 44).min(area.width) as usize;
        let tl = self.tree_lines(Rect {
            width: tree_w as u16,
            height: area.height.saturating_sub(1),
            ..area
        });
        let el = match &self.model {
            Some(m) => {
                let ed_area = Rect {
                    width: area.width.saturating_sub(tree_w as u16),
                    height: area.height.saturating_sub(1),
                    ..area
                };
                self.editor.lines(ed_area, m, false)
            }
            None => vec![Line::raw(""), Line::raw(format!("  {}", self.model_err))],
        };
        buf.push(Line::raw(""));
        for i in 0..area.height.saturating_sub(1) as usize {
            let left = tl.get(i).cloned().unwrap_or_default();
            let right = el.get(i).cloned().unwrap_or_default();
            // flatten styled lines into plain text for the snapshot
            let lt: String = left.spans.iter().map(|s| s.content.as_ref()).collect();
            let rt: String = right.spans.iter().map(|s| s.content.as_ref()).collect();
            buf.push(Line::raw(format!("{lt:<tree_w$}{rt}")));
        }
        move |f: &mut Frame| {
            f.render_widget(Paragraph::new(buf.clone()), f.area());
        }
    }

    fn move_sel(&mut self, term: &mut Term, delta: isize) {
        let new = self
            .sel
            .saturating_add_signed(delta)
            .min(self.flat.len().saturating_sub(1));
        if new == self.sel {
            return;
        }
        let mut bg = self.snapshot(term);
        if !self.may_leave(term, &mut bg) {
            return;
        }
        self.sel = new;
        self.load_selection();
    }

    fn expand_sel(&mut self) {
        let Some(&id) = self.flat.get(self.sel) else {
            return;
        };
        if self.tree.node(id).is_dir && self.tree.expand(id).is_ok() {
            self.reflatten();
        }
    }

    fn collapse_or_up(&mut self, term: &mut Term) {
        let Some(&id) = self.flat.get(self.sel) else {
            return;
        };
        let n = self.tree.node(id);
        if n.is_dir && n.expanded {
            self.tree.collapse(id);
            self.reflatten();
            return;
        }
        if let Some(parent) = n.parent {
            let mut bg = self.snapshot(term);
            if !self.may_leave(term, &mut bg) {
                return;
            }
            if let Some(pos) = self.flat.iter().position(|&i| i == parent) {
                self.sel = pos;
                self.load_selection();
            }
        }
    }

    /// Hand the keyboard to the embedded editor until it comes back.
    fn enter_editor(&mut self, term: &mut Term) -> bool {
        if self.model.is_none() {
            return true;
        }
        self.focus_editor = true;
        loop {
            let _ = term.terminal.draw(|f| self.render(f));
            let Some(k) = term.next_key() else {
                self.focus_editor = false;
                return false; // EOF: quit the browser too
            };
            if k == Key::Resize {
                continue;
            }
            let mut bg = self.snapshot(term);
            let mut m = self.model.take().unwrap();
            let ev = self.editor.handle_key(k, term, &mut bg, &mut m);
            self.model = Some(m);
            match ev {
                EditorEvent::Continue => {}
                EditorEvent::Back => {
                    self.focus_editor = false;
                    return true;
                }
                EditorEvent::Quit => {
                    self.focus_editor = false;
                    return false;
                }
            }
        }
    }
}

pub fn run(root: &Path, follow: bool) -> ExitCode {
    let Some(mut tree) = Tree::new(root) else {
        eprintln!("winfacl: {}: cannot open", root.display());
        return ExitCode::FAILURE;
    };
    if tree.node(tree.root).is_dir {
        if let Err(e) = tree.expand(tree.root) {
            eprintln!("winfacl: {}: {}", root.display(), e);
        }
    }

    let Ok(mut term) = Term::new() else {
        eprintln!("winfacl: cannot initialise the terminal");
        return ExitCode::FAILURE;
    };

    let mut b = Browser {
        flat: tree.flatten(),
        tree,
        sel: 0,
        top: 0,
        follow,
        focus_editor: false,
        model: None,
        model_path: PathBuf::new(),
        model_err: String::new(),
        editor: EditorState::new(true),
    };
    b.load_selection();

    loop {
        let _ = term.terminal.draw(|f| b.render(f));
        let Some(k) = term.next_key() else {
            return ExitCode::SUCCESS;
        };
        let code = match k {
            Key::Resize => continue,
            Key::Press(c, _) => c,
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => b.move_sel(&mut term, -1),
            KeyCode::Down | KeyCode::Char('j') => b.move_sel(&mut term, 1),
            KeyCode::PageUp => b.move_sel(&mut term, -10),
            KeyCode::PageDown => b.move_sel(&mut term, 10),
            KeyCode::Home => b.move_sel(&mut term, isize::MIN / 2),
            KeyCode::End => b.move_sel(&mut term, isize::MAX / 2),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('+') => b.expand_sel(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('-') => b.collapse_or_up(&mut term),
            KeyCode::Enter => {
                let Some(&id) = b.flat.get(b.sel) else {
                    continue;
                };
                if b.tree.node(id).is_dir {
                    if b.tree.node(id).expanded {
                        b.tree.collapse(id);
                        b.reflatten();
                    } else {
                        b.expand_sel();
                    }
                } else if !b.enter_editor(&mut term) {
                    return ExitCode::SUCCESS;
                }
            }
            KeyCode::Tab | KeyCode::Char('e') => {
                if !b.enter_editor(&mut term) {
                    return ExitCode::SUCCESS;
                }
            }
            KeyCode::Char('r') => {
                let Some(&id) = b.flat.get(b.sel) else {
                    continue;
                };
                if b.tree.node(id).is_dir && b.tree.node(id).loaded {
                    let _ = b.tree.refresh(id);
                    b.reflatten();
                }
            }
            KeyCode::F(1) | KeyCode::Char('?') => {
                let mut bg = b.snapshot(&mut term);
                dialogs::help(&mut term, &mut bg);
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                let mut bg = b.snapshot(&mut term);
                if b.may_leave(&mut term, &mut bg) {
                    return ExitCode::SUCCESS;
                }
            }
            _ => {}
        }
    }
}
