//! `Model`: the load/stage/apply lifecycle over one filesystem object,
//! plus the recursive-apply walk. Port of the C `wf_model_*` functions.

use super::model::{format_getfacl, EntryList, Kind, Tag};
use super::xattr;
use rustix::io::Errno;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    Ok,
    /// Filesystem has no ACL support: degraded, mode-bits-only view.
    NotSup,
    Denied,
    NoEnt,
    Error,
}

#[derive(Debug)]
pub struct Model {
    pub path: PathBuf,
    pub target: Option<PathBuf>,
    pub is_symlink: bool,
    pub is_dir: bool,
    pub mode: u32,

    pub status: LoadStatus,
    pub load_errno: Option<Errno>,
    pub acl_supported: bool,
    pub has_default_acl: bool,

    pub owner: u32,
    pub group: u32,
    pub staged_owner: u32,
    pub staged_group: u32,

    pub current: EntryList,
    pub staged: EntryList,

    pub auto_mask: bool,
    pub recursive: bool,
}

#[derive(Debug, Clone)]
pub struct ApplyError {
    pub path: PathBuf,
    pub errno: Errno,
    pub what: &'static str,
}

#[derive(Debug, Default)]
pub struct Report {
    pub objects: usize,
    pub dirs: usize,
    pub failures: usize,
    pub errors: Vec<ApplyError>,
}

impl Model {
    /// Load `path`. Always returns a Model; check `status`. NotSup
    /// degrades to a read-only mode-bit view like the C version.
    pub fn load(path: &Path, follow: bool) -> Model {
        use rustix::fs::{lstat, stat};

        let mut m = Model {
            path: path.to_path_buf(),
            target: None,
            is_symlink: false,
            is_dir: false,
            mode: 0,
            status: LoadStatus::Ok,
            load_errno: None,
            acl_supported: true,
            has_default_acl: false,
            owner: 0,
            group: 0,
            staged_owner: 0,
            staged_group: 0,
            current: EntryList::new(),
            staged: EntryList::new(),
            auto_mask: true,
            recursive: false,
        };

        let fail = |m: &mut Model, e: Errno| {
            m.load_errno = Some(e);
            m.status = match e {
                Errno::NOTSUP => LoadStatus::NotSup,
                Errno::ACCESS | Errno::PERM => LoadStatus::Denied,
                Errno::NOENT | Errno::NOTDIR => LoadStatus::NoEnt,
                _ => LoadStatus::Error,
            };
        };

        let lst = match lstat(path) {
            Ok(st) => st,
            Err(e) => {
                fail(&mut m, e);
                return m;
            }
        };
        if (lst.st_mode & 0o170000) == 0o120000 {
            m.is_symlink = true;
            m.target = std::fs::read_link(path).ok();
        }
        // POSIX.1e ACLs do not exist on symlinks; we always operate on
        // the resolved object, follow only steers what we report.
        let _ = follow;
        let st = match stat(path) {
            Ok(st) => st,
            Err(e) => {
                fail(&mut m, e);
                return m;
            }
        };
        m.is_dir = (st.st_mode & 0o170000) == 0o040000;
        m.mode = st.st_mode as u32 & 0o7777;
        m.owner = st.st_uid;
        m.group = st.st_gid;
        m.staged_owner = st.st_uid;
        m.staged_group = st.st_gid;

        match xattr::read_acl(path, Kind::Access, true) {
            Ok(Some(entries)) => m.current.0.extend(entries),
            Ok(None) => {
                // no extended ACL: the mode bits are the whole story
                m.current = mode_to_entries(m.mode);
            }
            Err(Errno::NOTSUP) => {
                m.acl_supported = false;
                m.load_errno = Some(Errno::NOTSUP);
                m.status = LoadStatus::NotSup;
                m.current = mode_to_entries(m.mode);
            }
            Err(e) => {
                fail(&mut m, e);
                return m;
            }
        }

        if m.is_dir && m.acl_supported {
            if let Ok(Some(entries)) = xattr::read_acl(path, Kind::Default, true) {
                m.has_default_acl = !entries.is_empty();
                m.current.0.extend(entries);
            }
        }

        m.current.sort_canonical();
        m.staged = m.current.clone();
        m
    }

    pub fn dirty(&self) -> bool {
        if self.staged_owner != self.owner || self.staged_group != self.group {
            return true;
        }
        let mut a = self.current.clone();
        let mut b = self.staged.clone();
        a.sort_canonical();
        b.sort_canonical();
        a != b
    }

    pub fn revert(&mut self) {
        self.staged = self.current.clone();
        self.staged_owner = self.owner;
        self.staged_group = self.group;
        self.recursive = false;
    }

    pub fn copy_access_to_default(&mut self) {
        if !self.is_dir {
            return;
        }
        self.remove_default();
        let access: Vec<_> = self
            .staged
            .0
            .iter()
            .filter(|e| e.kind == Kind::Access)
            .copied()
            .collect();
        for e in access {
            self.staged.set(Kind::Default, e.tag, e.id, e.perms);
        }
        self.staged.sort_canonical();
    }

    pub fn remove_default(&mut self) {
        self.staged.0.retain(|e| e.kind != Kind::Default);
    }

    fn apply_one(list: &EntryList, path: &Path, is_dir: bool, rep: &mut Report) -> bool {
        let mut ok = true;
        if let Err(e) = xattr::write_acl(path, Kind::Access, &list.0) {
            rep.add(path, e, "set access acl");
            ok = false;
        }
        if is_dir {
            if list.0.iter().any(|e| e.kind == Kind::Default) {
                if let Err(e) = xattr::write_acl(path, Kind::Default, &list.0) {
                    rep.add(path, e, "set default acl");
                    ok = false;
                }
            } else if let Err(e) = xattr::remove_default(path) {
                if e != Errno::NOTSUP && e != Errno::NOENT {
                    rep.add(path, e, "remove default acl");
                    ok = false;
                }
            }
        }
        ok
    }

    /// Write the staged ACL (recursively when armed). Auto-mask and sort
    /// happen here, matching C. Per-path failures land in the report and
    /// never abort a recursive walk. Returns Err(()) if anything failed.
    pub fn apply(&mut self, rep: &mut Report) -> Result<(), ()> {
        if !self.acl_supported {
            rep.add(&self.path, Errno::NOTSUP, "filesystem has no ACL support");
            return Err(());
        }
        // Auto-mask first: it can only ever make a set validate that
        // would otherwise fail on "named entries but no mask".
        if self.auto_mask {
            self.staged.apply_auto_mask(Kind::Access);
            if self.is_dir {
                self.staged.apply_auto_mask(Kind::Default);
            }
        }
        if self.staged.validate(self.is_dir).is_err() {
            rep.add(&self.path, Errno::INVAL, "validation");
            return Err(());
        }
        self.staged.sort_canonical();

        if self.staged_owner != self.owner || self.staged_group != self.group {
            let u = (self.staged_owner != self.owner).then_some(self.staged_owner);
            let g = (self.staged_group != self.group).then_some(self.staged_group);
            match rustix::fs::chown(
                &self.path,
                u.map(|x| unsafe { rustix::fs::Uid::from_raw(x) }),
                g.map(|x| unsafe { rustix::fs::Gid::from_raw(x) }),
            ) {
                Ok(()) => {
                    self.owner = self.staged_owner;
                    self.group = self.staged_group;
                }
                Err(e) => rep.add(&self.path, e, "chown"),
            }
        }

        if self.recursive && self.is_dir {
            for entry in walkdir::WalkDir::new(&self.path).follow_links(false) {
                match entry {
                    Ok(ent) => {
                        let ft = ent.file_type();
                        if ft.is_symlink() {
                            continue; // never follow or modify symlinks
                        }
                        if Self::apply_one(&self.staged, ent.path(), ft.is_dir(), rep) {
                            rep.objects += 1;
                            if ft.is_dir() {
                                rep.dirs += 1;
                            }
                        }
                    }
                    Err(err) => {
                        let errno = err
                            .io_error()
                            .and_then(|e| e.raw_os_error())
                            .map_or(Errno::IO, Errno::from_raw_os_error);
                        let p = err
                            .path()
                            .map_or_else(|| self.path.clone(), Path::to_path_buf);
                        rep.add(&p, errno, "read directory");
                    }
                }
            }
        } else if Self::apply_one(&self.staged, &self.path, self.is_dir, rep) {
            rep.objects += 1;
            if self.is_dir {
                rep.dirs += 1;
            }
        }

        if rep.failures > 0 {
            return Err(());
        }
        self.current = self.staged.clone();
        self.has_default_acl = self.current.0.iter().any(|e| e.kind == Kind::Default);
        Ok(())
    }

    pub fn format_getfacl(&self) -> String {
        format_getfacl(
            &self.path.to_string_lossy(),
            self.staged_owner,
            self.staged_group,
            &self.staged,
        )
    }
}

impl Report {
    fn add(&mut self, path: &Path, errno: Errno, what: &'static str) {
        self.failures += 1;
        // cap stored detail so a huge tree cannot exhaust memory
        if self.errors.len() < 1000 {
            self.errors.push(ApplyError {
                path: path.to_path_buf(),
                errno,
                what,
            });
        }
    }
}

/// Count objects a recursive apply would touch (symlinks excluded),
/// stopping at `limit`.
pub fn count_tree(path: &Path, limit: usize) -> usize {
    let mut n = 0;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        if let Ok(ent) = entry {
            if ent.file_type().is_symlink() {
                continue;
            }
            n += 1;
            if limit > 0 && n >= limit {
                break;
            }
        }
    }
    n
}

pub fn mode_to_entries(mode: u32) -> EntryList {
    let mut l = EntryList::new();
    let bits = |s: u32| -> u8 { ((mode >> s) & 0x7) as u8 };
    l.set(Kind::Access, Tag::UserObj, 0, bits(6));
    l.set(Kind::Access, Tag::GroupObj, 0, bits(3));
    l.set(Kind::Access, Tag::Other, 0, bits(0));
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::model::{P_ALL, P_R, P_W, P_X};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn sh(cmd: &str) -> String {
        let out = Command::new("sh").args(["-c", cmd]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn load_nonexistent() {
        let m = Model::load(Path::new("/no/such/winfacl/path"), true);
        assert_eq!(m.status, LoadStatus::NoEnt);
    }

    #[test]
    fn load_plain_file_synthesizes_mode_entries() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o640)).unwrap();

        let m = Model::load(&f, true);
        assert_eq!(m.status, LoadStatus::Ok);
        assert!(m.acl_supported);
        assert!(!m.is_dir);
        assert_eq!(m.staged.0.len(), 3);
        let i = m.staged.find(Kind::Access, Tag::UserObj, 0).unwrap();
        assert_eq!(m.staged.0[i].perms, P_R | P_W);
        let i = m.staged.find(Kind::Access, Tag::GroupObj, 0).unwrap();
        assert_eq!(m.staged.0[i].perms, P_R);
        let i = m.staged.find(Kind::Access, Tag::Other, 0).unwrap();
        assert_eq!(m.staged.0[i].perms, 0);
        assert!(!m.dirty());
    }

    #[test]
    fn roundtrip_file_add_named_user() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();

        let mut m = Model::load(&f, true);
        m.staged.set(Kind::Access, Tag::User, 0, P_ALL);
        assert!(m.dirty());
        let mut rep = Report::default();
        m.apply(&mut rep).unwrap();
        assert_eq!(rep.objects, 1);
        assert!(!m.dirty());

        // getfacl agrees, mask auto-created
        let txt = sh(&format!("getfacl -c '{}'", f.display()));
        assert!(txt.contains("user:root:rwx"), "got: {txt}");
        assert!(txt.contains("mask::"), "got: {txt}");

        // a fresh load sees the same set
        let m2 = Model::load(&f, true);
        assert!(m2.staged.find(Kind::Access, Tag::User, 0).is_some());
        assert!(m2.staged.find(Kind::Access, Tag::Mask, 0).is_some());
    }

    #[test]
    fn roundtrip_dir_default_acl() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("d");
        std::fs::create_dir(&d).unwrap();

        let mut m = Model::load(&d, true);
        assert!(m.is_dir);
        m.staged.set(Kind::Access, Tag::User, 0, P_ALL);
        m.copy_access_to_default();
        let mut rep = Report::default();
        m.apply(&mut rep).unwrap();

        let txt = sh(&format!("getfacl -c '{}'", d.display()));
        assert!(txt.contains("default:user:root:rwx"), "got: {txt}");

        // children inherit
        let child = d.join("kid");
        std::fs::write(&child, b"x").unwrap();
        let txt = sh(&format!("getfacl -c '{}'", child.display()));
        assert!(txt.contains("user:root:rwx"), "got: {txt}");

        // removing the default ACL sticks
        let mut m = Model::load(&d, true);
        assert!(m.has_default_acl);
        m.remove_default();
        let mut rep = Report::default();
        m.apply(&mut rep).unwrap();
        let m2 = Model::load(&d, true);
        assert!(!m2.has_default_acl);
    }

    #[test]
    fn revert_discards_staged() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        let mut m = Model::load(&f, true);
        m.staged.set(Kind::Access, Tag::User, 42, P_R);
        assert!(m.dirty());
        m.revert();
        assert!(!m.dirty());
    }

    #[test]
    fn recursive_apply_walks_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("f1"), b"x").unwrap();
        std::fs::write(root.join("a/f2"), b"x").unwrap();
        std::fs::write(root.join("a/b/f3"), b"x").unwrap();
        std::os::unix::fs::symlink("f1", root.join("ln")).unwrap();

        assert_eq!(count_tree(&root, 0), 6); // root, a, b, f1, f2, f3

        let mut m = Model::load(&root, true);
        m.staged.set(Kind::Access, Tag::User, 0, P_R | P_X);
        m.recursive = true;
        let mut rep = Report::default();
        m.apply(&mut rep).unwrap();
        assert_eq!(rep.objects, 6); // symlink skipped
        assert_eq!(rep.dirs, 3);

        for p in ["f1", "a/f2", "a/b/f3"] {
            let txt = sh(&format!("getfacl -c '{}'", root.join(p).display()));
            assert!(txt.contains("user:root:r-x"), "{p}: {txt}");
        }
        // the symlink target f1 got it via the walk, but the link itself
        // was never followed as a separate object (objects == 6 proves it)
    }

    #[test]
    fn recursive_apply_continues_past_unreadable() {
        if uzers::get_current_uid() == 0 {
            return; // root ignores permission bits; scenario needs a mortal
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("locked/inner")).unwrap();
        std::fs::write(root.join("ok"), b"x").unwrap();
        let locked = root.join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut m = Model::load(&root, true);
        m.staged.set(Kind::Access, Tag::User, 0, P_R);
        m.recursive = true;
        let mut rep = Report::default();
        let r = m.apply(&mut rep);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(r.is_err());
        assert!(rep.failures > 0);
        assert!(rep.objects >= 2); // root + ok were still processed
    }
}
