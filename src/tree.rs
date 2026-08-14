//! Lazy filesystem tree behind the browser's left panel. Arena-indexed
//! (`NodeId`), children loaded on first expand, dirs-first alpha sort.
//! Port of the C `tree_model.c`.

use std::path::{Path, PathBuf};

pub type NodeId = usize;

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub expanded: bool,
    pub loaded: bool,
    pub load_failed: bool,
    pub load_error: Option<std::io::Error>,
}

#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: NodeId,
}

impl Tree {
    /// Root at `path` (need not be a directory). `None` if it can't be
    /// lstat'ed.
    pub fn new(path: &Path) -> Option<Tree> {
        let lmeta = std::fs::symlink_metadata(path).ok()?;
        let is_symlink = lmeta.file_type().is_symlink();
        let is_dir = if is_symlink {
            std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
        } else {
            lmeta.is_dir()
        };
        Some(Tree {
            nodes: vec![Node {
                name: path.to_string_lossy().into_owned(),
                parent: None,
                children: Vec::new(),
                is_dir,
                is_symlink,
                expanded: false,
                loaded: false,
                load_failed: false,
                load_error: None,
            }],
            root: 0,
        })
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    fn load_children(&mut self, id: NodeId) -> Result<(), std::io::Error> {
        let path = self.path(id);
        let rd = match std::fs::read_dir(&path) {
            Ok(rd) => rd,
            Err(e) => {
                let n = &mut self.nodes[id];
                n.load_failed = true;
                n.load_error = Some(std::io::Error::new(e.kind(), e.to_string()));
                return Err(e);
            }
        };

        let old: Vec<NodeId> = std::mem::take(&mut self.nodes[id].children);
        let mut unused: Vec<Option<NodeId>> = old.iter().copied().map(Some).collect();
        let mut kids: Vec<NodeId> = Vec::new();

        for dent in rd.flatten() {
            let name = dent.file_name().to_string_lossy().into_owned();
            // Reuse the old node if this name survived, keeping its
            // loaded subtree and expansion state.
            let reused = unused
                .iter_mut()
                .find(|slot| slot.map(|i| self.nodes[i].name == name).unwrap_or(false));
            if let Some(slot) = reused {
                kids.push(slot.take().unwrap());
                continue;
            }
            let ft = match dent.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            let is_symlink = ft.is_symlink();
            let is_dir = if is_symlink {
                std::fs::metadata(dent.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            } else {
                ft.is_dir()
            };
            self.nodes.push(Node {
                name,
                parent: Some(id),
                children: Vec::new(),
                is_dir,
                is_symlink,
                expanded: false,
                loaded: false,
                load_failed: false,
                load_error: None,
            });
            kids.push(self.nodes.len() - 1);
        }

        kids.sort_by(|&a, &b| {
            let (na, nb) = (&self.nodes[a], &self.nodes[b]);
            nb.is_dir
                .cmp(&na.is_dir)
                .then_with(|| na.name.cmp(&nb.name))
        });

        let n = &mut self.nodes[id];
        n.children = kids;
        n.loaded = true;
        n.load_failed = false;
        n.load_error = None;
        Ok(())
    }

    /// Expand a directory, reading children on first use. `Err` leaves
    /// the node marked failed but retryable; non-dirs always fail.
    pub fn expand(&mut self, id: NodeId) -> Result<(), std::io::Error> {
        if !self.nodes[id].is_dir {
            return Err(std::io::Error::from_raw_os_error(20)); // ENOTDIR
        }
        if !self.nodes[id].loaded {
            self.load_children(id)?;
        }
        self.nodes[id].expanded = true;
        Ok(())
    }

    pub fn collapse(&mut self, id: NodeId) {
        self.nodes[id].expanded = false;
    }

    /// Re-read children from disk, keeping loaded/expanded state of
    /// grandchildren that still exist by name.
    pub fn refresh(&mut self, id: NodeId) -> Result<(), std::io::Error> {
        self.load_children(id)
    }

    /// Absolute path of a node.
    pub fn path(&self, id: NodeId) -> PathBuf {
        match self.nodes[id].parent {
            None => PathBuf::from(&self.nodes[id].name),
            Some(p) => self.path(p).join(&self.nodes[id].name),
        }
    }

    /// Depth below the root (root itself is 0).
    pub fn depth(&self, id: NodeId) -> usize {
        match self.nodes[id].parent {
            None => 0,
            Some(p) => self.depth(p) + 1,
        }
    }

    /// Visible nodes: root first, depth-first through expanded dirs.
    pub fn flatten(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.flatten_into(self.root, &mut out);
        out
    }

    fn flatten_into(&self, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        if self.nodes[id].expanded {
            for &c in &self.nodes[id].children {
                self.flatten_into(c, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Fixture:
    ///   beta/inner.txt, alpha/, zz.txt, aa.txt, link -> aa.txt
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir(p.join("beta")).unwrap();
        std::fs::create_dir(p.join("alpha")).unwrap();
        std::fs::write(p.join("beta/inner.txt"), b"").unwrap();
        std::fs::write(p.join("zz.txt"), b"").unwrap();
        std::fs::write(p.join("aa.txt"), b"").unwrap();
        std::os::unix::fs::symlink("aa.txt", p.join("link")).unwrap();
        dir
    }

    fn names(t: &Tree, ids: &[NodeId]) -> Vec<String> {
        ids.iter().map(|&i| t.node(i).name.clone()).collect()
    }

    #[test]
    fn new_and_root() {
        let fix = fixture();
        let t = Tree::new(fix.path()).unwrap();
        let r = t.node(t.root);
        assert!(r.is_dir);
        assert!(!r.loaded);
        assert_eq!(t.depth(t.root), 0);
        assert_eq!(t.path(t.root), fix.path());
        assert!(Tree::new(Path::new("/no/such/wftree/path")).is_none());
    }

    #[test]
    fn expand_sorts_dirs_first() {
        let fix = fixture();
        let mut t = Tree::new(fix.path()).unwrap();
        t.expand(t.root).unwrap();
        let r = t.node(t.root);
        assert!(r.loaded && r.expanded);
        assert_eq!(
            names(&t, &t.node(t.root).children),
            vec!["alpha", "beta", "aa.txt", "link", "zz.txt"]
        );
        let kids = t.node(t.root).children.clone();
        assert!(t.node(kids[0]).is_dir);
        assert!(t.node(kids[1]).is_dir);
        assert!(!t.node(kids[2]).is_dir);
        assert!(t.node(kids[3]).is_symlink);
        assert_eq!(t.node(kids[2]).parent, Some(t.root));
    }

    #[test]
    fn expand_file_fails_but_is_retryable_dirs_recover() {
        let fix = fixture();
        let mut t = Tree::new(fix.path()).unwrap();
        t.expand(t.root).unwrap();
        let kids = t.node(t.root).children.clone();

        // aa.txt is not a directory
        assert!(t.expand(kids[2]).is_err());

        // unreadable dir fails, marks the node, then recovers on retry
        let alpha = fix.path().join("alpha");
        std::fs::set_permissions(&alpha, std::fs::Permissions::from_mode(0o000)).unwrap();
        if uzers::get_current_uid() != 0 {
            assert!(t.expand(kids[0]).is_err());
            assert!(t.node(kids[0]).load_failed);
            assert!(t.node(kids[0]).children.is_empty());
        }
        std::fs::set_permissions(&alpha, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(t.expand(kids[0]).is_ok());
        assert!(!t.node(kids[0]).load_failed);
    }

    #[test]
    fn path_and_depth_nested() {
        let fix = fixture();
        let mut t = Tree::new(fix.path()).unwrap();
        t.expand(t.root).unwrap();
        let beta = t.node(t.root).children[1];
        t.expand(beta).unwrap();
        let inner = t.node(beta).children[0];
        assert_eq!(t.node(inner).name, "inner.txt");
        assert_eq!(t.depth(inner), 2);
        assert_eq!(t.path(inner), fix.path().join("beta/inner.txt"));
    }

    #[test]
    fn flatten_respects_expansion() {
        let fix = fixture();
        let mut t = Tree::new(fix.path()).unwrap();

        assert_eq!(t.flatten().len(), 1); // collapsed root

        t.expand(t.root).unwrap();
        assert_eq!(t.flatten().len(), 6);

        let beta = t.node(t.root).children[1];
        t.expand(beta).unwrap();
        let flat = t.flatten();
        assert_eq!(flat.len(), 7);
        // depth-first: inner.txt right after beta
        assert_eq!(names(&t, &flat)[2..5], ["beta", "inner.txt", "aa.txt"]);

        t.collapse(beta);
        assert_eq!(t.flatten().len(), 6);
        assert!(t.node(beta).loaded); // collapse keeps the subtree loaded
        t.expand(beta).unwrap(); // re-expand needs no reload
        assert_eq!(t.flatten().len(), 7);
    }

    #[test]
    fn refresh_picks_up_changes_and_preserves_state() {
        let fix = fixture();
        let mut t = Tree::new(fix.path()).unwrap();
        t.expand(t.root).unwrap();
        assert_eq!(t.node(t.root).children.len(), 5);

        // expand beta so we can prove refresh preserves its state
        let beta = t.node(t.root).children[1];
        t.expand(beta).unwrap();

        std::fs::write(fix.path().join("new.txt"), b"").unwrap();
        t.refresh(t.root).unwrap();
        assert_eq!(t.node(t.root).children.len(), 6);
        let beta = t.node(t.root).children[1];
        assert_eq!(t.node(beta).name, "beta");
        assert!(t.node(beta).expanded && t.node(beta).loaded);

        std::fs::remove_file(fix.path().join("new.txt")).unwrap();
        t.refresh(t.root).unwrap();
        assert_eq!(t.node(t.root).children.len(), 5);
    }

    #[test]
    fn root_can_be_a_file() {
        let fix = fixture();
        let f = fix.path().join("aa.txt");
        let mut t = Tree::new(&f).unwrap();
        assert!(!t.node(t.root).is_dir);
        assert!(t.expand(t.root).is_err());
        assert_eq!(t.path(t.root), f);
    }
}
