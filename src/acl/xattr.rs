//! Raw POSIX ACL I/O through the `system.posix_acl_access` /
//! `system.posix_acl_default` extended attributes -- the same stable
//! kernel ABI libacl uses, minus the C dependency.
//!
//! Wire format (little-endian): u32 version (= 2), then per entry
//! u16 tag, u16 perms, u32 qualifier (0xFFFF_FFFF when not named).

use super::model::{Entry, Kind, Tag};
use rustix::io::Errno;

pub const ACL_EA_VERSION: u32 = 2;
pub const UNDEFINED_ID: u32 = 0xFFFF_FFFF;

const TAG_USER_OBJ: u16 = 0x01;
const TAG_USER: u16 = 0x02;
const TAG_GROUP_OBJ: u16 = 0x04;
const TAG_GROUP: u16 = 0x08;
const TAG_MASK: u16 = 0x10;
const TAG_OTHER: u16 = 0x20;

fn attr_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Access => "system.posix_acl_access",
        Kind::Default => "system.posix_acl_default",
    }
}

/// Decode the xattr blob into entries of `kind`.
pub fn decode(blob: &[u8], kind: Kind) -> Result<Vec<Entry>, Errno> {
    if blob.len() < 4 || !(blob.len() - 4).is_multiple_of(8) {
        return Err(Errno::INVAL);
    }
    let version = u32::from_le_bytes(blob[0..4].try_into().unwrap());
    if version != ACL_EA_VERSION {
        return Err(Errno::INVAL);
    }
    let mut out = Vec::with_capacity((blob.len() - 4) / 8);
    for chunk in blob[4..].chunks_exact(8) {
        let tag_raw = u16::from_le_bytes(chunk[0..2].try_into().unwrap());
        let perms = (u16::from_le_bytes(chunk[2..4].try_into().unwrap()) & 0x7) as u8;
        let id = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let tag = match tag_raw {
            TAG_USER_OBJ => Tag::UserObj,
            TAG_USER => Tag::User,
            TAG_GROUP_OBJ => Tag::GroupObj,
            TAG_GROUP => Tag::Group,
            TAG_MASK => Tag::Mask,
            TAG_OTHER => Tag::Other,
            _ => continue, // unknown tag: ignore rather than fail
        };
        out.push(Entry {
            kind,
            tag,
            id: if tag.is_named() { id } else { 0 },
            perms,
        });
    }
    Ok(out)
}

/// Encode every entry of `kind` into the xattr wire format.
pub fn encode(entries: &[Entry], kind: Kind) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&ACL_EA_VERSION.to_le_bytes());
    for e in entries.iter().filter(|e| e.kind == kind) {
        let tag = match e.tag {
            Tag::UserObj => TAG_USER_OBJ,
            Tag::User => TAG_USER,
            Tag::GroupObj => TAG_GROUP_OBJ,
            Tag::Group => TAG_GROUP,
            Tag::Mask => TAG_MASK,
            Tag::Other => TAG_OTHER,
        };
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&u16::from(e.perms & 0x7).to_le_bytes());
        let id = if e.tag.is_named() { e.id } else { UNDEFINED_ID };
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

/// Read one ACL. `Ok(None)` means the attribute does not exist (ENODATA)
/// -- the object has only its mode bits. `follow` controls symlink
/// resolution.
pub fn read_acl(
    path: &std::path::Path,
    kind: Kind,
    follow: bool,
) -> Result<Option<Vec<Entry>>, Errno> {
    let name = attr_name(kind);
    let get = if follow {
        rustix::fs::getxattr
    } else {
        rustix::fs::lgetxattr
    };
    let mut buf = vec![0u8; 4 + 8 * 64];
    loop {
        match get(path, name, &mut buf) {
            Ok(n) => {
                buf.truncate(n);
                return decode(&buf, kind).map(Some);
            }
            Err(Errno::NODATA) => return Ok(None),
            Err(Errno::RANGE) => buf.resize(buf.len() * 2, 0),
            Err(e) => return Err(e),
        }
    }
}

/// Write every entry of `kind` from `entries` as the object's ACL.
pub fn write_acl(path: &std::path::Path, kind: Kind, entries: &[Entry]) -> Result<(), Errno> {
    let blob = encode(entries, kind);
    rustix::fs::setxattr(
        path,
        attr_name(kind),
        &blob,
        rustix::fs::XattrFlags::empty(),
    )
}

/// Remove the default ACL; missing attribute is success.
pub fn remove_default(path: &std::path::Path) -> Result<(), Errno> {
    match rustix::fs::removexattr(path, attr_name(Kind::Default)) {
        Ok(()) | Err(Errno::NODATA) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::model::{EntryList, P_ALL, P_R, P_W, P_X};
    use std::process::Command;

    fn getfacl(path: &std::path::Path) -> String {
        let out = Command::new("getfacl")
            .arg("-c")
            .arg(path)
            .output()
            .expect("getfacl runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn setfacl(path: &std::path::Path, spec: &str) {
        let st = Command::new("setfacl")
            .args(["-m", spec])
            .arg(path)
            .status()
            .expect("setfacl runs");
        assert!(st.success());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let mut l = EntryList::new();
        l.set(Kind::Access, Tag::UserObj, 0, P_R | P_W);
        l.set(Kind::Access, Tag::User, 4242, P_ALL);
        l.set(Kind::Access, Tag::GroupObj, 0, P_R);
        l.set(Kind::Access, Tag::Group, 999, P_X);
        l.set(Kind::Access, Tag::Mask, 0, P_ALL);
        l.set(Kind::Access, Tag::Other, 0, 0);
        l.set(Kind::Default, Tag::UserObj, 0, P_ALL); // must be excluded

        let blob = encode(&l.0, Kind::Access);
        assert_eq!(blob.len(), 4 + 6 * 8);
        assert_eq!(&blob[0..4], &2u32.to_le_bytes());

        let back = decode(&blob, Kind::Access).unwrap();
        assert_eq!(back.len(), 6);
        let mut expect = l.clone();
        expect.0.retain(|e| e.kind == Kind::Access);
        assert_eq!(back, expect.0);
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode(&[1, 0, 0, 0], Kind::Access).is_err()); // bad version
        assert!(decode(&[2, 0, 0, 0, 9], Kind::Access).is_err()); // torn entry
    }

    #[test]
    fn read_what_setfacl_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        setfacl(&f, "u:root:rwx");
        setfacl(&f, "g:root:r");

        let entries = read_acl(&f, Kind::Access, true).unwrap().unwrap();
        let l = EntryList(entries);
        assert!(l.find(Kind::Access, Tag::User, 0).is_some());
        assert!(l.find(Kind::Access, Tag::Group, 0).is_some());
        assert!(l.find(Kind::Access, Tag::Mask, 0).is_some());
        assert!(l.find(Kind::Access, Tag::UserObj, 0).is_some());
    }

    #[test]
    fn getfacl_reads_what_we_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();

        let mut l = EntryList::new();
        l.set(Kind::Access, Tag::UserObj, 0, P_R | P_W);
        l.set(Kind::Access, Tag::User, 0, P_ALL);
        l.set(Kind::Access, Tag::GroupObj, 0, P_R);
        l.set(Kind::Access, Tag::Mask, 0, P_ALL);
        l.set(Kind::Access, Tag::Other, 0, P_R);
        l.sort_canonical();
        write_acl(&f, Kind::Access, &l.0).unwrap();

        let txt = getfacl(&f);
        assert!(txt.contains("user:root:rwx"), "got: {txt}");
        assert!(txt.contains("mask::rwx"), "got: {txt}");
    }

    #[test]
    fn plain_file_has_no_acl_xattr() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        assert!(read_acl(&f, Kind::Access, true).unwrap().is_none());
        assert!(read_acl(dir.path(), Kind::Default, true).unwrap().is_none());
    }

    #[test]
    fn default_acl_roundtrip_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("d");
        std::fs::create_dir(&d).unwrap();

        let mut l = EntryList::new();
        l.set(Kind::Default, Tag::UserObj, 0, P_ALL);
        l.set(Kind::Default, Tag::User, 0, P_ALL);
        l.set(Kind::Default, Tag::GroupObj, 0, P_R);
        l.set(Kind::Default, Tag::Mask, 0, P_ALL);
        l.set(Kind::Default, Tag::Other, 0, P_R);
        l.sort_canonical();
        write_acl(&d, Kind::Default, &l.0).unwrap();

        let txt = getfacl(&d);
        assert!(txt.contains("default:user:root:rwx"), "got: {txt}");

        remove_default(&d).unwrap();
        assert!(read_acl(&d, Kind::Default, true).unwrap().is_none());
        // removing an already-absent default ACL still succeeds
        remove_default(&d).unwrap();
    }
}
