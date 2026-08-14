//! UI-free POSIX.1e ACL model: an editable entry list with canonical
//! ordering, mask handling, validation and effective-access evaluation.
//! Port of the C `acl_model.c`; disk I/O lives in `xattr.rs`.

pub const P_R: u8 = 0x4;
pub const P_W: u8 = 0x2;
pub const P_X: u8 = 0x1;
pub const P_ALL: u8 = P_R | P_W | P_X;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Access,
    Default,
}

/// Declaration order is the canonical POSIX.1e ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tag {
    UserObj,
    User,
    GroupObj,
    Group,
    Mask,
    Other,
}

impl Tag {
    pub fn is_named(self) -> bool {
        matches!(self, Tag::User | Tag::Group)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub tag: Tag,
    /// uid for `Tag::User`, gid for `Tag::Group`, 0 otherwise.
    pub id: u32,
    pub perms: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryList(pub Vec<Entry>);

impl EntryList {
    pub fn new() -> Self {
        EntryList(Vec::new())
    }

    pub fn find(&self, kind: Kind, tag: Tag, id: u32) -> Option<usize> {
        self.0.iter().position(|e| {
            e.kind == kind && e.tag == tag && (!tag.is_named() || e.id == id)
        })
    }

    /// Insert or replace; base tags and (tag,id) pairs are unique per kind.
    pub fn set(&mut self, kind: Kind, tag: Tag, id: u32, perms: u8) -> usize {
        let perms = perms & P_ALL;
        if let Some(i) = self.find(kind, tag, id) {
            self.0[i].perms = perms;
            return i;
        }
        self.0.push(Entry {
            kind,
            tag,
            id: if tag.is_named() { id } else { 0 },
            perms,
        });
        self.0.len() - 1
    }

    pub fn remove_at(&mut self, idx: usize) {
        if idx < self.0.len() {
            self.0.remove(idx);
        }
    }

    /// Canonical POSIX order: access before default; user_obj, users (by
    /// id), group_obj, groups (by id), mask, other.
    pub fn sort_canonical(&mut self) {
        self.0.sort_by(|a, b| {
            (a.kind, a.tag, a.id).cmp(&(b.kind, b.tag, b.id))
        });
    }

    pub fn count_named(&self, kind: Kind) -> usize {
        self.0
            .iter()
            .filter(|e| e.kind == kind && e.tag.is_named())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_find_remove() {
        let mut l = EntryList::new();
        l.set(Kind::Access, Tag::UserObj, 0, P_ALL);
        l.set(Kind::Access, Tag::User, 1000, P_R | P_W);
        l.set(Kind::Access, Tag::User, 1001, P_R);
        assert_eq!(l.0.len(), 3);

        // set on an existing principal replaces, never duplicates
        let i = l.set(Kind::Access, Tag::User, 1000, P_R);
        assert_eq!(l.0.len(), 3);
        assert_eq!(l.0[i].perms, P_R);

        // base tags ignore id for identity
        l.set(Kind::Access, Tag::UserObj, 999, P_R);
        assert_eq!(l.0.len(), 3);

        // find distinguishes kinds and named ids
        assert!(l.find(Kind::Access, Tag::User, 1001).is_some());
        assert!(l.find(Kind::Default, Tag::User, 1001).is_none());
        assert!(l.find(Kind::Access, Tag::User, 4242).is_none());

        // perms are clamped to rwx bits
        let i = l.set(Kind::Access, Tag::User, 1001, 0xFF);
        assert_eq!(l.0[i].perms, P_ALL);

        let i = l.find(Kind::Access, Tag::User, 1000).unwrap();
        l.remove_at(i);
        assert_eq!(l.0.len(), 2);
        assert!(l.find(Kind::Access, Tag::User, 1000).is_none());
    }

    #[test]
    fn canonical_order() {
        let mut l = EntryList::new();
        l.set(Kind::Default, Tag::Other, 0, P_R);
        l.set(Kind::Access, Tag::Other, 0, P_R);
        l.set(Kind::Access, Tag::Mask, 0, P_ALL);
        l.set(Kind::Access, Tag::Group, 2000, P_R);
        l.set(Kind::Access, Tag::Group, 100, P_R);
        l.set(Kind::Access, Tag::GroupObj, 0, P_R);
        l.set(Kind::Access, Tag::User, 1001, P_R);
        l.set(Kind::Access, Tag::User, 42, P_R);
        l.set(Kind::Access, Tag::UserObj, 0, P_ALL);
        l.set(Kind::Default, Tag::UserObj, 0, P_ALL);
        l.sort_canonical();

        let seq: Vec<(Kind, Tag, u32)> =
            l.0.iter().map(|e| (e.kind, e.tag, e.id)).collect();
        assert_eq!(
            seq,
            vec![
                (Kind::Access, Tag::UserObj, 0),
                (Kind::Access, Tag::User, 42),
                (Kind::Access, Tag::User, 1001),
                (Kind::Access, Tag::GroupObj, 0),
                (Kind::Access, Tag::Group, 100),
                (Kind::Access, Tag::Group, 2000),
                (Kind::Access, Tag::Mask, 0),
                (Kind::Access, Tag::Other, 0),
                (Kind::Default, Tag::UserObj, 0),
                (Kind::Default, Tag::Other, 0),
            ]
        );
    }

    #[test]
    fn count_named_counts_per_kind() {
        let mut l = EntryList::new();
        l.set(Kind::Access, Tag::User, 1, P_R);
        l.set(Kind::Access, Tag::Group, 2, P_R);
        l.set(Kind::Access, Tag::UserObj, 0, P_R);
        l.set(Kind::Default, Tag::User, 1, P_R);
        assert_eq!(l.count_named(Kind::Access), 2);
        assert_eq!(l.count_named(Kind::Default), 1);
    }
}

// ---- mask, validation, effective access (task 3) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Owner,
    NamedUser,
    GroupClass,
    Other,
}

#[derive(Debug, Clone)]
pub struct Effective {
    pub granted: u8,
    pub matched: MatchKind,
    pub pre_mask: u8,
    pub mask: u8,
    pub mask_applied: bool,
    pub trace: String,
}

impl EntryList {
    /// Union of the group class (group_obj + named users + named groups),
    /// exactly what setfacl's mask recalculation uses.
    pub fn calc_mask(&self, kind: Kind) -> u8 {
        self.0
            .iter()
            .filter(|e| {
                e.kind == kind
                    && matches!(e.tag, Tag::User | Tag::Group | Tag::GroupObj)
            })
            .fold(0, |m, e| m | e.perms)
            & P_ALL
    }

    /// Insert/refresh the mask for `kind`: always upsert when named
    /// entries exist; refresh (but never invent) one otherwise.
    pub fn apply_auto_mask(&mut self, kind: Kind) {
        if !self.0.iter().any(|e| e.kind == kind) {
            return;
        }
        let mask = self.calc_mask(kind);
        if self.count_named(kind) == 0 {
            if let Some(i) = self.find(kind, Tag::Mask, 0) {
                self.0[i].perms = mask;
            }
            return;
        }
        self.set(kind, Tag::Mask, 0, mask);
    }

    fn count_tag(&self, kind: Kind, tag: Tag) -> usize {
        self.0.iter().filter(|e| e.kind == kind && e.tag == tag).count()
    }

    /// POSIX validity: base entries present exactly once, at most one
    /// mask, mask required with named entries, no duplicates, defaults
    /// only on directories.
    pub fn validate(&self, is_dir: bool) -> Result<(), String> {
        if !is_dir && self.0.iter().any(|e| e.kind == Kind::Default) {
            return Err("default ACL entries are only valid on directories".into());
        }

        for (i, a) in self.0.iter().enumerate() {
            for b in &self.0[i + 1..] {
                if a.kind == b.kind
                    && a.tag == b.tag
                    && (!a.tag.is_named() || a.id == b.id)
                {
                    return Err(format!(
                        "duplicate {} entry",
                        kind_name(a.kind)
                    ));
                }
            }
        }

        for kind in [Kind::Access, Kind::Default] {
            if kind == Kind::Default && !self.0.iter().any(|e| e.kind == kind) {
                continue; // no default ACL at all is fine
            }
            for (tag, what) in [
                (Tag::UserObj, "owner"),
                (Tag::GroupObj, "owning-group"),
                (Tag::Other, "Everyone"),
            ] {
                if self.count_tag(kind, tag) != 1 {
                    return Err(format!(
                        "{} ACL needs exactly one {} entry",
                        kind_name(kind),
                        what
                    ));
                }
            }
            if self.count_tag(kind, Tag::Mask) > 1 {
                return Err(format!(
                    "{} ACL has more than one mask entry",
                    kind_name(kind)
                ));
            }
            if self.count_named(kind) > 0 && self.count_tag(kind, Tag::Mask) == 0 {
                return Err(format!(
                    "{} ACL has named entries but no mask (enable auto-mask)",
                    kind_name(kind)
                ));
            }
        }
        Ok(())
    }

    fn access(&self, tag: Tag, id: u32) -> Option<&Entry> {
        self.find(Kind::Access, tag, id).map(|i| &self.0[i])
    }

    /// POSIX.1e access-check order: owner (unmasked), named user
    /// (masked), group class union (masked), other (unmasked).
    pub fn effective(&self, uid: u32, owner: u32, group: u32, gids: &[u32]) -> Effective {
        let mask_e = self.access(Tag::Mask, 0);
        let mask = mask_e.map_or(P_ALL, |e| e.perms);

        // 1. The file owner is matched first and never clipped by the mask.
        if uid == owner {
            let p = self.access(Tag::UserObj, 0).map_or(0, |e| e.perms);
            return Effective {
                granted: p,
                matched: MatchKind::Owner,
                pre_mask: p,
                mask,
                mask_applied: false,
                trace: format!(
                    "{} is the file owner, so the owner entry (user::{}) \
                     matches first.\nThe mask never applies to the owner \
                     entry.\nEffective: {}",
                    super::names::uid_name(uid),
                    perm_string(p),
                    perm_string(p)
                ),
            };
        }

        // 2. A named user entry for this uid wins over every group entry.
        if let Some(e) = self.access(Tag::User, uid) {
            let granted = e.perms & mask;
            let clip = if mask_e.is_some() {
                format!(
                    "Mask {} clips it ({} bits removed).",
                    perm_string(mask),
                    if e.perms & !mask != 0 { "some" } else { "no" }
                )
            } else {
                "There is no mask entry, so nothing is clipped.".into()
            };
            return Effective {
                granted,
                matched: MatchKind::NamedUser,
                pre_mask: e.perms,
                mask,
                mask_applied: true,
                trace: format!(
                    "Named user entry user:{}:{} matches; named user entries \
                     take precedence over all group entries.\n{}\nEffective: {}",
                    super::names::uid_name(uid),
                    perm_string(e.perms),
                    clip,
                    perm_string(granted)
                ),
            };
        }

        // 3. Group class: the owning group plus every matching named
        //    group; a bit is granted if ANY matching entry carries it.
        let mut uni = 0u8;
        let mut matched_any = false;
        let mut names = Vec::new();
        if let Some(e) = self.access(Tag::GroupObj, 0) {
            if gids.contains(&group) {
                uni |= e.perms;
                matched_any = true;
                names.push(format!(
                    "group::{} (owning group {})",
                    perm_string(e.perms),
                    super::names::gid_name(group)
                ));
            }
        }
        for e in self.0.iter().filter(|e| {
            e.kind == Kind::Access && e.tag == Tag::Group && gids.contains(&e.id)
        }) {
            uni |= e.perms;
            matched_any = true;
            names.push(format!(
                "group:{}:{}",
                super::names::gid_name(e.id),
                perm_string(e.perms)
            ));
        }
        if matched_any {
            let granted = uni & mask;
            let clip = if mask_e.is_some() {
                format!("Mask {} clips it.", perm_string(mask))
            } else {
                "There is no mask entry, so nothing is clipped.".into()
            };
            return Effective {
                granted,
                matched: MatchKind::GroupClass,
                pre_mask: uni,
                mask,
                mask_applied: true,
                trace: format!(
                    "No owner or named-user match; the group class applies.\n\
                     Matching entries: {} -> combined {}\n{}\nEffective: {}",
                    names.join(", "),
                    perm_string(uni),
                    clip,
                    perm_string(granted)
                ),
            };
        }

        // 4. Everyone else. Never clipped by the mask.
        let p = self.access(Tag::Other, 0).map_or(0, |e| e.perms);
        Effective {
            granted: p,
            matched: MatchKind::Other,
            pre_mask: p,
            mask,
            mask_applied: false,
            trace: format!(
                "Not the owner, no named-user entry, and none of the user's \
                 groups match.\nThe Everyone entry (other::{}) applies; the \
                 mask never clips it.\nEffective: {}",
                perm_string(p),
                perm_string(p)
            ),
        }
    }
}

pub fn perm_string(perms: u8) -> String {
    let mut s = String::with_capacity(3);
    s.push(if perms & P_R != 0 { 'r' } else { '-' });
    s.push(if perms & P_W != 0 { 'w' } else { '-' });
    s.push(if perms & P_X != 0 { 'x' } else { '-' });
    s
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Access => "access",
        Kind::Default => "default",
    }
}

#[cfg(test)]
mod tests_mask {
    use super::*;

    fn base(is_dir: bool) -> EntryList {
        let _ = is_dir;
        let mut l = EntryList::new();
        l.set(Kind::Access, Tag::UserObj, 0, P_R | P_W);
        l.set(Kind::Access, Tag::GroupObj, 0, P_R);
        l.set(Kind::Access, Tag::Other, 0, P_R);
        l
    }

    #[test]
    fn mask_calc_unions_group_class() {
        let mut l = base(false);
        l.set(Kind::Access, Tag::User, 1000, P_R | P_W);
        l.set(Kind::Access, Tag::Group, 2000, P_X);
        // group_obj r | named user rw | named group x = rwx
        assert_eq!(l.calc_mask(Kind::Access), P_ALL);
        // owner and other never contribute
        let l2 = base(false);
        assert_eq!(l2.calc_mask(Kind::Access), P_R);
    }

    #[test]
    fn auto_mask_upserts_only_with_named() {
        let mut l = base(false);
        l.apply_auto_mask(Kind::Access);
        // no named entries, no pre-existing mask: none invented
        assert!(l.find(Kind::Access, Tag::Mask, 0).is_none());

        // existing mask without named entries is refreshed, not removed
        l.set(Kind::Access, Tag::Mask, 0, 0);
        l.apply_auto_mask(Kind::Access);
        let i = l.find(Kind::Access, Tag::Mask, 0).unwrap();
        assert_eq!(l.0[i].perms, P_R);

        // named entry appears: mask upserted to the union
        l.set(Kind::Access, Tag::User, 1000, P_ALL);
        l.apply_auto_mask(Kind::Access);
        let i = l.find(Kind::Access, Tag::Mask, 0).unwrap();
        assert_eq!(l.0[i].perms, P_ALL);

        // empty kind is left completely alone
        let mut e = EntryList::new();
        e.apply_auto_mask(Kind::Default);
        assert!(e.0.is_empty());
    }

    #[test]
    fn validate_rules() {
        // valid base
        assert!(base(false).validate(false).is_ok());

        // missing owner entry
        let mut l = base(false);
        let i = l.find(Kind::Access, Tag::UserObj, 0).unwrap();
        l.remove_at(i);
        assert!(l.validate(false).is_err());

        // named entries without a mask
        let mut l = base(false);
        l.set(Kind::Access, Tag::User, 1000, P_R);
        assert!(l.validate(false).is_err());
        l.set(Kind::Access, Tag::Mask, 0, P_ALL);
        assert!(l.validate(false).is_ok());

        // default entries on a file are rejected, ok on a dir (complete set)
        let mut l = base(true);
        l.set(Kind::Default, Tag::UserObj, 0, P_ALL);
        l.set(Kind::Default, Tag::GroupObj, 0, P_R);
        l.set(Kind::Default, Tag::Other, 0, P_R);
        assert!(l.validate(true).is_ok());
        assert!(l.validate(false).is_err());

        // an incomplete default set on a dir is invalid
        let mut l = base(true);
        l.set(Kind::Default, Tag::UserObj, 0, P_ALL);
        assert!(l.validate(true).is_err());

        // duplicates are impossible via set(); simulate via raw push
        let mut l = base(false);
        l.0.push(Entry { kind: Kind::Access, tag: Tag::Other, id: 0, perms: P_R });
        assert!(l.validate(false).is_err());
    }

    #[test]
    fn effective_owner_unmasked() {
        let mut l = base(false);
        l.set(Kind::Access, Tag::User, 500, P_R);
        l.set(Kind::Access, Tag::Mask, 0, 0); // mask clears everything
        let e = l.effective(1000, 1000, 100, &[100]);
        assert_eq!(e.matched, MatchKind::Owner);
        assert_eq!(e.granted, P_R | P_W); // owner entry, mask ignored
        assert!(!e.mask_applied);
    }

    #[test]
    fn effective_named_user_masked() {
        let mut l = base(false);
        l.set(Kind::Access, Tag::User, 500, P_ALL);
        l.set(Kind::Access, Tag::Mask, 0, P_R);
        let e = l.effective(500, 1000, 100, &[999]);
        assert_eq!(e.matched, MatchKind::NamedUser);
        assert_eq!(e.pre_mask, P_ALL);
        assert_eq!(e.granted, P_R);
        assert!(e.mask_applied);
    }

    #[test]
    fn effective_group_class_union() {
        let mut l = base(false);
        l.set(Kind::Access, Tag::Group, 200, P_W);
        l.set(Kind::Access, Tag::Group, 300, P_X);
        l.set(Kind::Access, Tag::Mask, 0, P_ALL);
        // member of owning group (r) + 200 (w) + 300 (x): union rwx
        let e = l.effective(500, 1000, 100, &[100, 200, 300]);
        assert_eq!(e.matched, MatchKind::GroupClass);
        assert_eq!(e.granted, P_ALL);
        // member of only 200: w
        let e = l.effective(500, 1000, 100, &[200]);
        assert_eq!(e.granted, P_W);
    }

    #[test]
    fn effective_other_ignores_mask() {
        let mut l = base(false);
        l.set(Kind::Access, Tag::Mask, 0, 0);
        let e = l.effective(500, 1000, 100, &[999]);
        assert_eq!(e.matched, MatchKind::Other);
        assert_eq!(e.granted, P_R);
        assert!(!e.mask_applied);
    }
}
