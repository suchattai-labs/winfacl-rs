//! uid/gid <-> name helpers with numeric fallback for orphan ids.
//! Port of the C `wf_names.c`.

use uzers::{get_group_by_gid, get_group_by_name, get_user_by_name, get_user_by_uid};

/// Name for a uid; falls back to the decimal id when unknown. Never fails.
pub fn uid_name(uid: u32) -> String {
    match get_user_by_uid(uid) {
        Some(u) => u.name().to_string_lossy().into_owned(),
        None => uid.to_string(),
    }
}

/// Name for a gid; falls back to the decimal id when unknown. Never fails.
pub fn gid_name(gid: u32) -> String {
    match get_group_by_gid(gid) {
        Some(g) => g.name().to_string_lossy().into_owned(),
        None => gid.to_string(),
    }
}

fn lookup(s: &str, by_name: impl Fn(&str) -> Option<u32>) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    // A bare numeric id is accepted even without a database entry --
    // POSIX ACLs may legitimately reference deleted users.
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().ok();
    }
    by_name(s)
}

pub fn lookup_user(s: &str) -> Option<u32> {
    lookup(s, |n| get_user_by_name(n).map(|u| u.uid()))
}

pub fn lookup_group(s: &str) -> Option<u32> {
    lookup(s, |n| get_group_by_name(n).map(|g| g.gid()))
}

/// Full group set for a uid: primary gid plus supplementary groups.
pub fn user_groups(uid: u32) -> Vec<u32> {
    let Some(user) = get_user_by_uid(uid) else {
        return Vec::new();
    };
    let mut gids = vec![user.primary_group_id()];
    let name = user.name().to_string_lossy().into_owned();
    if let Some(groups) = uzers::get_user_groups(&name, user.primary_group_id()) {
        for g in groups {
            if !gids.contains(&g.gid()) {
                gids.push(g.gid());
            }
        }
    }
    gids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_lookup() {
        assert_eq!(lookup_user("root"), Some(0));
        // numerics resolve even when orphaned
        assert_eq!(lookup_user("4242424"), Some(4242424));
        assert_eq!(lookup_user("no-such-user-winfacl"), None);
        assert_eq!(lookup_user(""), None);
        assert_eq!(lookup_group("root"), Some(0));
        assert_eq!(lookup_group("nope-winfacl-xyz"), None);

        assert_eq!(uid_name(0), "root");
        // an unknown id degrades to its number rather than failing
        assert_eq!(uid_name(4242424), "4242424");

        assert!(!user_groups(0).is_empty());
    }
}
