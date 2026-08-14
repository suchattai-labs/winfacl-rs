# winfacl-rs

A terminal POSIX ACL manager in the style of the Windows **Advanced
Security Settings** dialog / **SetACL Studio**: an expandable filesystem
tree on the left, a live permission editor on the right. Rust port of
[winfacl](https://github.com/suchattai-labs/winfacl) with identical
behavior and a fully static release binary.

```
winfacl                     # browse the whole filesystem from /
winfacl /srv                # browse, rooted at /srv
winfacl /srv/share/file     # edit one file, classic full-screen dialog
winfacl -d /srv/share       # dump in getfacl(1) form (script-friendly)
```

## Features

- Two-panel browser: lazy filesystem tree, live ACL preview as the
  cursor moves, dirty-prompt before leaving staged changes behind
- Access **and default** ACL editing, automatic mask recalculation
  matching `setfacl`, recursive apply with per-path failure reporting
- Effective-access evaluation for any user, mask clipping shown per entry
- `-d/--dump` output byte-identical to `getfacl` (including
  `#effective:` annotations) — verified in CI against the real tools
- Zero C dependencies: ACLs go through the kernel's
  `system.posix_acl_*` xattr ABI directly, so the release binary is
  **fully static** (musl) and runs on any Linux distro

## Install

Grab a static binary from the releases page (`winfacl-linux-x86_64` or
`winfacl-linux-aarch64`), `chmod +x`, done. Or build from source:

```
cargo build --release
# static:
cargo build --release --target x86_64-unknown-linux-musl
```

## Keys

Browser: `j/k`/arrows move · `→`/`Enter` expand · `←` collapse/up ·
`r` re-read dir · `Tab`/`e` edit · `q` quit.
Editor: `a`/`e`/`r` add/edit/remove · `f` effective access ·
`d`/`D` copy access→default / drop default ACL · `m` auto-mask ·
`R` recursive apply · `s`/`o` apply / apply+leave · `u` revert ·
`?` help · `q` back/quit.

## Testing

`cargo test` covers the ACL model, xattr wire format (round-tripped
against `setfacl`/`getfacl`), and the tree. `tests/integration.sh`
cross-checks dump output against `getfacl` and the C winfacl as an
oracle, and smoke-tests the TUI under a pty. CI runs all of it.
