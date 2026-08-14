//! The Advanced-Security-Settings-style permission editor. Renders into
//! any rect so the browser can embed it; `run` wraps it standalone.
//! Port of the C `ui.c`, including the access+default row merging.

use super::dialogs;
use super::term::{Key, Term};
use crate::acl::facade::{count_tree, Model, Report};
use crate::acl::model::{perm_string, Entry, Kind, Tag};
use crate::acl::names;
use crossterm::event::KeyCode;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::process::ExitCode;

const BTNS: [&str; 7] = ["Add", "Edit", "Remove", "Effective Access", "Apply", "OK", "Cancel"];
const B_ADD: usize = 0;
const B_EDIT: usize = 1;
const B_REMOVE: usize = 2;
const B_EFF: usize = 3;
const B_APPLY: usize = 4;
const B_OK: usize = 5;
const B_CANCEL: usize = 6;

/// A display row merges an access entry with the identical default
/// entry, the way the Windows dialog shows one ACE covering the folder
/// and everything inside it.
#[derive(Clone, Copy)]
struct Row {
    aidx: Option<usize>,
    didx: Option<usize>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum EditorEvent {
    Continue,
    Back,
    Quit,
}

pub struct EditorState {
    rows: Vec<Row>,
    pub sel: usize,
    top: usize,
    focus_buttons: bool,
    btn: usize,
    status: String,
    status_error: bool,
    pub embedded: bool,
}

impl EditorState {
    pub fn new(embedded: bool) -> EditorState {
        EditorState {
            rows: Vec::new(),
            sel: 0,
            top: 0,
            focus_buttons: false,
            btn: 0,
            status: String::new(),
            status_error: false,
            embedded,
        }
    }

    pub fn rows_build(&mut self, m: &Model) {
        self.rows.clear();
        let l = &m.staged;
        let mut used = vec![false; l.0.len()];
        for (i, e) in l.0.iter().enumerate() {
            if e.kind != Kind::Access {
                continue;
            }
            match l.find(Kind::Default, e.tag, e.id) {
                Some(d) if l.0[d].perms == e.perms => {
                    used[d] = true;
                    self.rows.push(Row { aidx: Some(i), didx: Some(d) });
                }
                _ => self.rows.push(Row { aidx: Some(i), didx: None }),
            }
        }
        for (i, e) in l.0.iter().enumerate() {
            if e.kind == Kind::Default && !used[i] {
                self.rows.push(Row { aidx: None, didx: Some(i) });
            }
        }
        if self.sel >= self.rows.len() {
            self.sel = self.rows.len().saturating_sub(1);
        }
    }

    fn primary<'m>(&self, m: &'m Model, r: &Row) -> &'m Entry {
        &m.staged.0[r.aidx.or(r.didx).unwrap()]
    }

    fn principal(&self, m: &Model, r: &Row) -> String {
        let e = self.primary(m, r);
        match e.tag {
            Tag::UserObj => format!("{} (owner)", names::uid_name(m.staged_owner)),
            Tag::User => names::uid_name(e.id),
            Tag::GroupObj => {
                format!("{} (owning group)", names::gid_name(m.staged_group))
            }
            Tag::Group => format!("{} (group)", names::gid_name(e.id)),
            Tag::Mask => "MASK (permission ceiling)".into(),
            Tag::Other => "Everyone".into(),
        }
    }

    fn applies(&self, m: &Model, r: &Row) -> &'static str {
        if !m.is_dir {
            return "This file only";
        }
        match (r.aidx, r.didx) {
            (Some(_), Some(_)) => "This folder, subfolders and files",
            (Some(_), None) => "This folder only",
            _ => "Subfolders and files only",
        }
    }

    fn access_summary(perms: u8, is_dir: bool) -> &'static str {
        match perms & 7 {
            7 => "Full control",
            6 => {
                if is_dir {
                    "Modify (no traverse)"
                } else {
                    "Modify"
                }
            }
            5 => "Read & execute",
            4 => "Read",
            2 => "Write",
            0 => "None",
            _ => "Special",
        }
    }

    pub fn set_status(&mut self, err: bool, s: impl Into<String>) {
        self.status = s.into();
        self.status_error = err;
    }

    /// Render into `area`. `active`: whether the editor has key focus.
    pub fn render(&mut self, f: &mut Frame, area: Rect, m: &Model, active: bool) {
        let lines = self.lines(area, m, active);
        f.render_widget(Paragraph::new(lines), area);
        if self.embedded {
            f.render_widget(
                Block::default().borders(Borders::LEFT),
                Rect { width: 1, ..area },
            );
        }
    }

    /// The editor screen as owned lines -- also used to snapshot the
    /// background that dialogs repaint beneath themselves.
    pub fn lines(&mut self, area: Rect, m: &Model, active: bool) -> Vec<Line<'static>> {
        let title_style = Style::new().bold().fg(Color::White).bg(Color::Blue);
        let banner = Style::new().bold().fg(Color::Yellow).bg(Color::Red);
        let header = Style::new().bold().fg(Color::Black).bg(Color::Gray);
        let selected = if active {
            Style::new().bold().fg(Color::Black).bg(Color::Cyan)
        } else {
            header
        };

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::styled(
            format!(
                " Advanced Security Settings{:>w$}",
                if self.embedded { "q Back " } else { "F1 Help  Esc Cancel " },
                w = (area.width as usize).saturating_sub(28)
            ),
            title_style,
        ));
        lines.push(Line::raw(""));
        lines.push(Line::raw(format!("  Name:   {}", m.path.display())));
        lines.push(Line::raw(format!(
            "  Owner:  {} (uid {})      Group: {} (gid {})",
            names::uid_name(m.staged_owner),
            m.staged_owner,
            names::gid_name(m.staged_group),
            m.staged_group
        )));
        lines.push(Line::raw(format!(
            "  Type:   {}{}   Mode: {:04o}   Auto-mask: {}",
            if m.is_dir { "Directory" } else { "File" },
            match (&m.is_symlink, &m.target) {
                (true, Some(t)) => format!(" (via symlink -> {})", t.display()),
                (true, None) => " (via symlink)".into(),
                _ => String::new(),
            },
            m.mode,
            if m.auto_mask { "on" } else { "off" }
        )));

        if !m.acl_supported {
            lines.push(Line::styled(
                " READ-ONLY: this filesystem does not support POSIX ACLs. \
                 Showing mode bits only. ",
                banner,
            ));
        } else if m.recursive {
            lines.push(Line::styled(
                " Recursive apply is ARMED: Apply will rewrite every child object. ",
                banner,
            ));
        }

        lines.push(Line::raw("  Permission entries:"));
        let w = area.width as usize;
        let narrow = w < 76;
        let head = if narrow {
            format!("  {:<6}{:<26}{:<20}", "Type", "Principal", "Access")
        } else {
            format!(
                "  {:<6}{:<26}{:<22}{:<28}{}",
                "Type", "Principal", "Access", "Applies to", "Inherited from"
            )
        };
        lines.push(Line::styled(format!("{head:<w$}"), header));

        let fixed = lines.len() + 3; // header block + separator + buttons + status
        let list_h = (area.height as usize).saturating_sub(fixed).max(3);
        if self.sel < self.top {
            self.top = self.sel;
        }
        if self.sel >= self.top + list_h {
            self.top = self.sel - list_h + 1;
        }

        for (k, row) in self.rows.iter().enumerate().skip(self.top).take(list_h) {
            let e = self.primary(m, row);
            let acc = format!(
                "{:<12} {}",
                if e.tag == Tag::Mask {
                    "(ceiling)"
                } else {
                    Self::access_summary(e.perms, m.is_dir)
                },
                perm_string(e.perms)
            );
            let body = if narrow {
                format!("  {:<6}{:<26}{:<20}", "Allow", self.principal(m, row), acc)
            } else {
                format!(
                    "  {:<6}{:<26}{:<22}{:<28}{}",
                    "Allow",
                    self.principal(m, row),
                    acc,
                    self.applies(m, row),
                    if row.aidx.is_some() { "None" } else { "Default (this folder)" }
                )
            };
            let style = if k == self.sel && !self.focus_buttons {
                selected
            } else if k == self.sel {
                header
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("{body:<w$}"), style));
        }
        if self.rows.is_empty() {
            lines.push(Line::raw("    (no entries)"));
        }
        for _ in self.rows.len().saturating_sub(self.top).min(list_h)..list_h {
            lines.push(Line::raw(""));
        }

        lines.push(Line::raw(format!("{:-<w$}", "")));
        let mut btns = String::from("  ");
        for (i, b) in BTNS.iter().enumerate() {
            let hot = self.focus_buttons && self.btn == i;
            btns.push_str(&format!("[{}{b}{}] ", if hot { "(" } else { " " }, if hot { ")" } else { " " }));
            if i == B_EFF {
                btns.push(' ');
            }
        }
        lines.push(Line::raw(btns));

        if self.status.is_empty() {
            let mut hint = String::from(
                "  a Add  e Edit  r Remove  f Effective  s Apply  o OK  d Copy->default  m Mask  ? Help",
            );
            if m.dirty() {
                hint.push_str("   [MODIFIED]");
            }
            lines.push(Line::styled(hint, Style::new().dim()));
        } else {
            let style = if self.status_error { banner } else { Style::default() };
            lines.push(Line::styled(format!("  {}", self.status), style));
        }

        lines
    }

    fn writable(&mut self, term: &mut Term, bg: dialogs::Background, m: &Model) -> bool {
        if !m.acl_supported {
            dialogs::msgbox(
                term,
                bg,
                "Read-only",
                "This filesystem does not support POSIX ACLs, so winfacl is \
                 showing the mode bits read-only.",
            );
            return false;
        }
        true
    }

    fn action_remove(&mut self, term: &mut Term, bg: dialogs::Background, m: &mut Model) {
        if !self.writable(term, bg, m) || self.rows.is_empty() {
            return;
        }
        let row = self.rows[self.sel];
        let e = *self.primary(m, &row);

        if matches!(e.tag, Tag::UserObj | Tag::GroupObj | Tag::Other) && e.kind == Kind::Access {
            self.set_status(
                true,
                "The owner, owning-group and Everyone entries are required by \
                 POSIX and cannot be removed.",
            );
            return;
        }
        if e.tag == Tag::Mask && m.staged.count_named(e.kind) > 0 {
            self.set_status(true, "The mask is required while named entries exist.");
            return;
        }
        let prin = self.principal(m, &row);
        if !dialogs::confirm(term, bg, "Remove entry", &format!("Remove the entry for {prin}?")) {
            return;
        }
        let (mut i1, i2) = match (row.aidx, row.didx) {
            (Some(a), Some(d)) => (a.max(d), Some(a.min(d))),
            (Some(a), None) => (a, None),
            (None, Some(d)) => (d, None),
            _ => return,
        };
        m.staged.remove_at(i1);
        if let Some(i2) = i2 {
            if i2 < i1 {
                i1 = i2;
            }
            m.staged.remove_at(i1);
        }
        if m.auto_mask {
            m.staged.apply_auto_mask(Kind::Access);
            if m.is_dir {
                m.staged.apply_auto_mask(Kind::Default);
            }
        }
        m.staged.sort_canonical();
        self.rows_build(m);
        self.set_status(false, "Entry removed (not yet applied).");
    }

    /// Returns false if the user backed out.
    fn action_apply(&mut self, term: &mut Term, bg: dialogs::Background, m: &mut Model) -> bool {
        if !self.writable(term, bg, m) {
            return false;
        }
        if !m.dirty() && !m.recursive {
            self.set_status(false, "Nothing to apply.");
            return false;
        }
        if let Err(e) = m.staged.validate(m.is_dir) {
            dialogs::msgbox(term, bg, "Cannot apply", &format!("The ACL is not valid:\n\n{e}"));
            return false;
        }
        if m.recursive && m.is_dir {
            let n = count_tree(&m.path, 200_001);
            if !dialogs::confirm(
                term,
                bg,
                "Confirm recursive apply",
                &format!(
                    "This will replace the permissions on {n}{} objects under\n{}.\n\n\
                     Child ACLs will be overwritten, not merged. Continue?",
                    if n > 200_000 { "+" } else { "" },
                    m.path.display()
                ),
            ) {
                return false;
            }
        }
        let mut rep = Report::default();
        let ok = m.apply(&mut rep).is_ok();
        dialogs::report_dialog(term, bg, &rep);
        if ok {
            self.set_status(false, format!("Applied to {} object(s).", rep.objects));
        } else {
            self.set_status(true, format!("Applied with {} failure(s).", rep.failures));
        }
        m.recursive = false;
        self.rows_build(m);
        ok
    }

    fn action_quit(&mut self, term: &mut Term, bg: dialogs::Background, m: &Model) -> EditorEvent {
        if self.embedded {
            // Back to the tree; staged changes stay on the model and the
            // browser prompts before the cursor leaves a dirty object.
            return EditorEvent::Back;
        }
        if m.dirty()
            && !dialogs::confirm(
                term,
                bg,
                "Discard changes?",
                "There are unsaved permission changes.\n\nDiscard them and quit?",
            )
        {
            return EditorEvent::Continue;
        }
        EditorEvent::Quit
    }

    fn button(
        &mut self,
        b: usize,
        term: &mut Term,
        bg: dialogs::Background,
        m: &mut Model,
    ) -> EditorEvent {
        match b {
            B_ADD => {
                if self.writable(term, bg, m) && dialogs::entry_dialog(term, bg, m, None) {
                    self.rows_build(m);
                    self.set_status(false, "Entry added (not yet applied).");
                }
            }
            B_EDIT => {
                if !self.rows.is_empty() && self.writable(term, bg, m) {
                    let idx = {
                        let r = self.rows[self.sel];
                        r.aidx.or(r.didx).unwrap()
                    };
                    if dialogs::entry_dialog(term, bg, m, Some(idx)) {
                        self.rows_build(m);
                        self.set_status(false, "Entry updated (not yet applied).");
                    }
                }
            }
            B_REMOVE => self.action_remove(term, bg, m),
            B_EFF => dialogs::effective_dialog(term, bg, m),
            B_APPLY => {
                self.action_apply(term, bg, m);
            }
            B_OK => {
                if !m.dirty() || self.action_apply(term, bg, m) {
                    return if self.embedded { EditorEvent::Back } else { EditorEvent::Quit };
                }
            }
            B_CANCEL => return self.action_quit(term, bg, m),
            _ => {}
        }
        EditorEvent::Continue
    }

    /// One key of the editor event loop.
    pub fn handle_key(
        &mut self,
        k: Key,
        term: &mut Term,
        bg: dialogs::Background,
        m: &mut Model,
    ) -> EditorEvent {
        self.status.clear();
        let code = match k {
            Key::Resize => return EditorEvent::Continue,
            Key::Press(c, _) => c,
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus_buttons {
                    self.focus_buttons = false;
                } else {
                    self.sel = self.sel.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.focus_buttons {
                    if self.sel + 1 < self.rows.len() {
                        self.sel += 1;
                    } else {
                        self.focus_buttons = true;
                        self.btn = 0;
                    }
                }
            }
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => {
                self.sel = (self.sel + 10).min(self.rows.len().saturating_sub(1));
            }
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = self.rows.len().saturating_sub(1),
            KeyCode::Tab => {
                if !self.focus_buttons {
                    self.focus_buttons = true;
                    self.btn = 0;
                } else if self.btn + 1 < BTNS.len() {
                    self.btn += 1;
                } else if self.embedded {
                    self.focus_buttons = false;
                    return EditorEvent::Back; // Tab off the end returns to the tree
                } else {
                    self.focus_buttons = false;
                }
            }
            KeyCode::BackTab => {
                if self.focus_buttons && self.btn > 0 {
                    self.btn -= 1;
                } else if self.focus_buttons {
                    self.focus_buttons = false;
                } else {
                    self.focus_buttons = true;
                    self.btn = BTNS.len() - 1;
                }
            }
            KeyCode::Left => {
                if self.focus_buttons {
                    self.btn = self.btn.saturating_sub(1);
                }
            }
            KeyCode::Right => {
                if self.focus_buttons && self.btn + 1 < BTNS.len() {
                    self.btn += 1;
                }
            }
            KeyCode::Enter => {
                let b = if self.focus_buttons { self.btn } else { B_EDIT };
                return self.button(b, term, bg, m);
            }
            KeyCode::Char('a') | KeyCode::Char('A') => return self.button(B_ADD, term, bg, m),
            KeyCode::Char('e') | KeyCode::Char('E') => return self.button(B_EDIT, term, bg, m),
            KeyCode::Char('f') | KeyCode::Char('F') => return self.button(B_EFF, term, bg, m),
            KeyCode::Char('s') => return self.button(B_APPLY, term, bg, m),
            KeyCode::Char('o') => return self.button(B_OK, term, bg, m),
            KeyCode::Char('r') | KeyCode::Delete => return self.button(B_REMOVE, term, bg, m),
            KeyCode::Char('m') => {
                m.auto_mask = !m.auto_mask;
                if m.auto_mask {
                    m.staged.apply_auto_mask(Kind::Access);
                    if m.is_dir {
                        m.staged.apply_auto_mask(Kind::Default);
                    }
                    m.staged.sort_canonical();
                    self.rows_build(m);
                }
                let s = format!(
                    "Automatic mask recalculation {}.",
                    if m.auto_mask { "enabled" } else { "disabled" }
                );
                self.set_status(false, s);
            }
            KeyCode::Char('R') => {
                if !m.is_dir {
                    self.set_status(true, "Recursive apply only applies to directories.");
                } else {
                    m.recursive = !m.recursive;
                    let s = format!(
                        "Recursive apply {}.",
                        if m.recursive { "armed" } else { "disarmed" }
                    );
                    self.set_status(false, s);
                }
            }
            KeyCode::Char('d') => {
                if self.writable(term, bg, m) {
                    if !m.is_dir {
                        self.set_status(true, "Default ACLs exist only on directories.");
                    } else {
                        m.copy_access_to_default();
                        if m.auto_mask {
                            m.staged.apply_auto_mask(Kind::Default);
                        }
                        m.staged.sort_canonical();
                        self.rows_build(m);
                        self.set_status(false, "Access ACL copied to the default ACL.");
                    }
                }
            }
            KeyCode::Char('D') => {
                if self.writable(term, bg, m) {
                    if !m.is_dir {
                        self.set_status(true, "Default ACLs exist only on directories.");
                    } else if dialogs::confirm(
                        term,
                        bg,
                        "Remove default ACL",
                        "Remove every default (inheritable) entry from this folder?",
                    ) {
                        m.remove_default();
                        self.rows_build(m);
                        self.set_status(false, "Default ACL removed (not yet applied).");
                    }
                }
            }
            KeyCode::Char('u') => {
                m.revert();
                self.rows_build(m);
                self.set_status(false, "Reverted to the on-disk ACL.");
            }
            KeyCode::F(1) | KeyCode::Char('?') | KeyCode::Char('h') => dialogs::help(term, bg),
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('c') => {
                return self.action_quit(term, bg, m)
            }
            _ => {}
        }
        EditorEvent::Continue
    }
}

/// Standalone editor over the whole screen.
pub fn run(mut m: Model) -> ExitCode {
    let Ok(mut term) = Term::new() else {
        eprintln!("winfacl: cannot initialise the terminal");
        return ExitCode::FAILURE;
    };
    let mut ed = EditorState::new(false);
    ed.rows_build(&m);
    if !m.acl_supported {
        ed.set_status(true, "Read-only: no ACL support on this filesystem.");
    }

    loop {
        let _ = term.terminal.draw(|f| {
            let area = f.area();
            ed.render(f, area, &m, true);
        });
        let Some(k) = term.next_key() else { return ExitCode::SUCCESS };

        // Dialogs opened while handling this key repaint a snapshot of
        // the frame we just drew beneath themselves.
        let snap = ed.lines(term.terminal.get_frame().area(), &m, false);
        let mut bg = move |f: &mut Frame| {
            f.render_widget(Paragraph::new(snap.clone()), f.area());
        };
        match ed.handle_key(k, &mut term, &mut bg, &mut m) {
            EditorEvent::Continue => {}
            EditorEvent::Back | EditorEvent::Quit => return ExitCode::SUCCESS,
        }
    }
}
