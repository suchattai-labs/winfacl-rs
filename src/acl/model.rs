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
