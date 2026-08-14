//! Modal dialogs: message, confirm, entry editor, principal picker,
//! effective access, apply report, help. Each runs a small event loop,
//! repainting the caller-supplied background underneath itself.

use super::term::{popup_rect, Key, Term};
use crate::acl::facade::{Model, Report};
use crate::acl::model::{perm_string, Kind, Tag, P_ALL, P_R, P_W, P_X};
use crate::acl::names;
use crossterm::event::KeyCode;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub type Background<'a> = &'a mut dyn FnMut(&mut Frame);

fn dialog_block(title: &str) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .title_style(Style::new().bold())
        .borders(Borders::ALL)
}

fn draw_over(term: &mut Term, bg: Background, draw: &mut dyn FnMut(&mut Frame)) {
    let _ = term.terminal.draw(|f| {
        bg(f);
        draw(f);
    });
}

/// Modal message box; any key dismisses it.
pub fn msgbox(term: &mut Term, bg: Background, title: &str, body: &str) {
    loop {
        let (t, b) = (title.to_string(), body.to_string());
        draw_over(term, bg, &mut |f| {
            let lines = b.lines().count() as u16;
            let area = popup_rect(f.area(), 60, lines + 4);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(format!("{b}\n\nPress any key to close"))
                    .wrap(Wrap { trim: false })
                    .block(dialog_block(&t)),
                area,
            );
        });
        match term.next_key() {
            None | Some(Key::Press(..)) => return,
            Some(Key::Resize) => continue,
        }
    }
}

/// Modal yes/no; `false` is the default and the Escape answer.
pub fn confirm(term: &mut Term, bg: Background, title: &str, body: &str) -> bool {
    let mut yes = false;
    loop {
        let (t, b) = (title.to_string(), body.to_string());
        draw_over(term, bg, &mut |f| {
            let lines = b.lines().count() as u16;
            let area = popup_rect(f.area(), 60, lines + 5);
            f.render_widget(Clear, area);
            let btn = |label: &str, focused: bool| {
                if focused {
                    format!("[({label})]")
                } else {
                    format!("[ {label} ]")
                }
            };
            let text = format!("{b}\n\n  {}   {}", btn("Yes", yes), btn("No", !yes));
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .block(dialog_block(&t)),
                area,
            );
        });
        match term.next_key() {
            None => return false,
            Some(Key::Resize) => continue,
            Some(k) => match k.code() {
                Some(KeyCode::Left | KeyCode::Right | KeyCode::Tab) => yes = !yes,
                Some(KeyCode::Enter) => return yes,
                Some(KeyCode::Esc) => return false,
                _ if k.is_char('y') || k.is_char('Y') => return true,
                _ if k.is_char('n') || k.is_char('N') => return false,
                _ => {}
            },
        }
    }
}

/// Filterable user/group picker. Returns the chosen (name, id).
pub fn pick_principal(term: &mut Term, bg: Background, groups: bool) -> Option<(String, u32)> {
    // Safety: iterating the passwd/group database is not thread-safe in
    // glibc; winfacl is single-threaded.
    let mut items: Vec<(String, u32)> = if groups {
        unsafe { uzers::all_groups() }
            .map(|g| (g.name().to_string_lossy().into_owned(), g.gid()))
            .collect()
    } else {
        unsafe { uzers::all_users() }
            .map(|u| (u.name().to_string_lossy().into_owned(), u.uid()))
            .collect()
    };
    items.sort_by(|a, b| a.0.cmp(&b.0));

    let mut filter = String::new();
    let mut sel: usize = 0;
    loop {
        let shown: Vec<&(String, u32)> =
            items.iter().filter(|(n, _)| n.contains(&filter)).collect();
        if sel >= shown.len() {
            sel = shown.len().saturating_sub(1);
        }
        let title = if groups {
            "Pick a group"
        } else {
            "Pick a user"
        };
        let f2 = filter.clone();
        let rows: Vec<String> = shown
            .iter()
            .enumerate()
            .map(|(i, (n, id))| format!("{} {:<28} {:>8}", if i == sel { ">" } else { " " }, n, id))
            .collect();
        draw_over(term, bg, &mut |f| {
            let area = popup_rect(f.area(), 46, 18);
            f.render_widget(Clear, area);
            let mut text = format!("Filter: {f2}_\n\n");
            let h = area.height.saturating_sub(5) as usize;
            let top = sel.saturating_sub(h.saturating_sub(1));
            for r in rows.iter().skip(top).take(h) {
                text.push_str(r);
                text.push('\n');
            }
            f.render_widget(Paragraph::new(text).block(dialog_block(title)), area);
        });
        match term.next_key()? {
            Key::Resize => continue,
            k => match k.code() {
                Some(KeyCode::Esc) => return None,
                Some(KeyCode::Enter) => {
                    return shown.get(sel).map(|&(n, id)| (n.clone(), *id));
                }
                Some(KeyCode::Up) => sel = sel.saturating_sub(1),
                Some(KeyCode::Down) => {
                    if sel + 1 < shown.len() {
                        sel += 1;
                    }
                }
                Some(KeyCode::Backspace) => {
                    filter.pop();
                }
                Some(KeyCode::Char(c)) => {
                    filter.push(c);
                    sel = 0;
                }
                _ => {}
            },
        }
    }
}

/// Add (edit_idx None) or edit an entry. Returns true if staged changed.
pub fn entry_dialog(
    term: &mut Term,
    bg: Background,
    m: &mut Model,
    edit_idx: Option<usize>,
) -> bool {
    // Form state
    let mut is_group = false;
    let mut named = true;
    let mut name = String::new();
    let mut perms: u8 = P_R;
    let mut kind = Kind::Access;
    let mut focus = 0usize; // 0 name, 1..=3 rwx, 4 kind, 5 ok, 6 cancel
    let mut err = String::new();

    let mut orig: Option<(Kind, Tag, u32)> = None;
    if let Some(i) = edit_idx {
        let e = m.staged.0[i];
        orig = Some((e.kind, e.tag, e.id));
        kind = e.kind;
        perms = e.perms;
        match e.tag {
            Tag::User => {
                is_group = false;
                name = names::uid_name(e.id);
            }
            Tag::Group => {
                is_group = true;
                name = names::gid_name(e.id);
            }
            _ => {
                // base entries: principal is fixed, only perms editable
                named = false;
                name = match e.tag {
                    Tag::UserObj => "(owner)".into(),
                    Tag::GroupObj => "(owning group)".into(),
                    Tag::Other => "(Everyone)".into(),
                    Tag::Mask => "(mask)".into(),
                    _ => String::new(),
                };
            }
        }
    }

    loop {
        let title = if edit_idx.is_some() {
            "Edit entry"
        } else {
            "Add entry"
        };
        let chk = |on: bool, label: &str, foc: bool| {
            format!(
                "{}[{}] {label}{}",
                if foc { ">" } else { " " },
                if on { "x" } else { " " },
                if foc { "<" } else { " " }
            )
        };
        let body = format!(
            "Principal ({}): {}{}\n   (F2 opens the picker; type a name or numeric id)\n\n\
             Permissions:  {}  {}  {}\n\n\
             Applies to:  {}{}{}\n\n  {}   {}\n\n{}",
            if is_group { "group" } else { "user" },
            name,
            if focus == 0 { "_" } else { " " },
            chk(perms & P_R != 0, "read", focus == 1),
            chk(perms & P_W != 0, "write", focus == 2),
            chk(perms & P_X != 0, "execute", focus == 3),
            if focus == 4 { ">" } else { " " },
            match kind {
                Kind::Access => "this object (access ACL)",
                Kind::Default => "new children (default ACL)",
            },
            if focus == 4 { "< (space toggles)" } else { "" },
            if focus == 5 { "[(OK)]" } else { "[ OK ]" },
            if focus == 6 {
                "[(Cancel)]"
            } else {
                "[ Cancel ]"
            },
            err
        );
        draw_over(term, bg, &mut |f| {
            let area = popup_rect(f.area(), 64, 15);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(body.clone())
                    .wrap(Wrap { trim: false })
                    .block(dialog_block(title)),
                area,
            );
        });

        let Some(k) = term.next_key() else {
            return false;
        };
        match k {
            Key::Resize => continue,
            k => match k.code() {
                Some(KeyCode::Esc) => return false,
                Some(KeyCode::Tab) | Some(KeyCode::Down) => focus = (focus + 1) % 7,
                Some(KeyCode::BackTab) | Some(KeyCode::Up) => focus = (focus + 6) % 7,
                Some(KeyCode::F(2)) if named && focus == 0 => {
                    if let Some((n, _)) = pick_principal(term, bg, is_group) {
                        name = n;
                    }
                }
                Some(KeyCode::Char(' ')) => match focus {
                    1 => perms ^= P_R,
                    2 => perms ^= P_W,
                    3 => perms ^= P_X,
                    4 => {
                        kind = if kind == Kind::Access {
                            Kind::Default
                        } else {
                            Kind::Access
                        };
                    }
                    _ => {
                        if focus == 0 && named {
                            name.push(' ');
                        }
                    }
                },
                Some(KeyCode::Char('/')) if focus == 0 && named => {
                    is_group = !is_group;
                }
                Some(KeyCode::Backspace) if focus == 0 && named => {
                    name.pop();
                }
                Some(KeyCode::Char(c)) if focus == 0 && named => name.push(c),
                Some(KeyCode::Enter) => {
                    if focus == 6 {
                        return false;
                    }
                    // OK (or Enter anywhere else): commit
                    if kind == Kind::Default && !m.is_dir {
                        err = "Default entries need a directory.".into();
                        continue;
                    }
                    if let Some((k0, t0, i0)) = orig {
                        if !named {
                            // base entry: only the perms change
                            if let Some(i) = m.staged.find(k0, t0, i0) {
                                m.staged.0[i].perms = perms & P_ALL;
                                return true;
                            }
                            return false;
                        }
                        // named principal may have been retyped: replace
                        if let Some(i) = m.staged.find(k0, t0, i0) {
                            m.staged.remove_at(i);
                        }
                    }
                    let id = if is_group {
                        names::lookup_group(name.trim())
                    } else {
                        names::lookup_user(name.trim())
                    };
                    let Some(id) = id else {
                        err = format!(
                            "Unknown {}: {}",
                            if is_group { "group" } else { "user" },
                            name.trim()
                        );
                        continue;
                    };
                    let tag = if is_group { Tag::Group } else { Tag::User };
                    m.staged.set(kind, tag, id, perms);
                    if m.auto_mask {
                        m.staged.apply_auto_mask(kind);
                    }
                    m.staged.sort_canonical();
                    return true;
                }
                _ => {}
            },
        }
    }
}

/// Effective-access dialog: evaluate any user (+ optional group override).
pub fn effective_dialog(term: &mut Term, bg: Background, m: &Model) {
    let mut user = String::new();
    let mut result = String::new();
    loop {
        let body =
            format!("User: {user}_   (F2 picks a user, Enter evaluates, Esc closes)\n\n{result}");
        draw_over(term, bg, &mut |f| {
            let area = popup_rect(f.area(), 70, 16);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(body.clone())
                    .wrap(Wrap { trim: false })
                    .block(dialog_block("Effective Access")),
                area,
            );
        });
        let Some(k) = term.next_key() else { return };
        match k {
            Key::Resize => continue,
            k => match k.code() {
                Some(KeyCode::Esc) => return,
                Some(KeyCode::F(2)) => {
                    if let Some((n, _)) = pick_principal(term, bg, false) {
                        user = n;
                    }
                }
                Some(KeyCode::Backspace) => {
                    user.pop();
                }
                Some(KeyCode::Char(c)) => user.push(c),
                Some(KeyCode::Enter) => {
                    let Some(uid) = names::lookup_user(user.trim()) else {
                        result = format!("Unknown user: {}", user.trim());
                        continue;
                    };
                    let gids = names::user_groups(uid);
                    let eff = m
                        .staged
                        .effective(uid, m.staged_owner, m.staged_group, &gids);
                    result = format!(
                        "Effective permissions: {}\n\n{}",
                        perm_string(eff.granted),
                        eff.trace
                    );
                }
                _ => {}
            },
        }
    }
}

/// Apply-report viewer.
pub fn report_dialog(term: &mut Term, bg: Background, rep: &Report) {
    let mut lines = vec![format!(
        "Applied to {} object{} ({} director{}), {} failure{}.",
        rep.objects,
        if rep.objects == 1 { "" } else { "s" },
        rep.dirs,
        if rep.dirs == 1 { "y" } else { "ies" },
        rep.failures,
        if rep.failures == 1 { "" } else { "s" }
    )];
    if !rep.errors.is_empty() {
        lines.push(String::new());
        for e in rep.errors.iter().take(200) {
            lines.push(format!("{:<18} {} : {}", e.what, e.path.display(), e.errno));
        }
    }
    msgbox(term, bg, "Apply report", &lines.join("\n"));
}

pub fn help(term: &mut Term, bg: Background) {
    msgbox(
        term,
        bg,
        "winfacl Help",
        "Browser (directory mode)\n\
         \x20 Up/Down, j/k     move through the filesystem tree\n\
         \x20 Right/l/Enter    expand a directory   Left/h  collapse / up\n\
         \x20 Tab or e         edit the selected object's permissions\n\
         \x20 r                re-read the selected directory\n\
         \x20 q or Esc         quit (from the editor: back to the tree)\n\
         \n\
         Editor\n\
         \x20 Up/Down, j/k     move through the permission entries\n\
         \x20 a  Add entry     e  Edit      r/Del  Remove\n\
         \x20 f  Effective Access           u  Revert to on-disk ACL\n\
         \x20 d  Copy access ACL to default D  Remove default ACL\n\
         \x20 m  Toggle auto-mask           R  Toggle recursive apply\n\
         \x20 s  Apply         o  OK (apply and leave)\n\
         \n\
         About POSIX ACLs\n\
         \x20 POSIX.1e has no deny entries: every entry is an Allow entry.\n\
         \x20 The mask clips named users, named groups and the owning\n\
         \x20 group -- but never the owner or Everyone. A directory's\n\
         \x20 default ACL is the template new children inherit.",
    );
}
