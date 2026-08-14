# winfacl-rs — design

Full Rust port of [winfacl](https://github.com/suchattai-labs/winfacl)
(C/ncurses), approved 2026-08-14. The C implementation is the behavioral
reference; where this document is silent, the C behavior is the spec.

## Goals

- Feature parity: two-panel browser (filesystem tree left, permission
  editor right), classic single-file editor, `-d` dump mode with
  getfacl-compatible output including `#effective:` annotations,
  recursive apply with per-path failure reporting, staged-changes model
  with dirty prompts.
- Portable release artifact: fully static musl binaries (x86_64 +
  aarch64), zero runtime dependencies, built by GitHub Actions.
- Same testing philosophy: pure-logic model layer unit-tested headless;
  integration suite cross-checked against getfacl/setfacl **and** the C
  `winfacl -d` as an oracle.

## Non-goals

- NFSv4 / rich ACLs, macOS/BSD support (Linux POSIX.1e only).
- Feature growth beyond the C version in the first release.

## Architecture

Mirrors the C layout; each unit is testable without a terminal.

| Module | Purpose | C counterpart |
|--------|---------|---------------|
| `src/acl/xattr.rs` | Raw ACL I/O: read/write `system.posix_acl_access` / `system.posix_acl_default` xattrs via `rustix`; the on-disk binary format (version 2, little-endian: u32 header, then per-entry u16 tag, u16 perms, u32 qualifier) | libacl (replaced) |
| `src/acl/model.rs` | Pure logic: entry list, canonical ordering, mask auto-calc, validation, effective access, staging/dirty tracking, getfacl formatting | `acl_model.c` |
| `src/acl/names.rs` | uid/gid ↔ name with numeric-orphan fallback (`uzers`) | `wf_names.c` |
| `src/tree.rs` | Lazy filesystem tree: expand/collapse/refresh/flatten, dirs-first sort, symlink + unreadable markers | `tree_model.c` |
| `src/ui/editor.rs` | Advanced-Security-Settings-style editor as a ratatui widget usable standalone or embedded | `ui.c` |
| `src/ui/browser.rs` | Two-panel shell, live preview on cursor move, dirty prompt before leaving an object | `browser.c` |
| `src/ui/dialogs.rs` | Add/edit entry, principal picker, effective access, confirm/message, apply report, help | `dialogs.c` |
| `src/main.rs` | clap CLI: file → editor, dir/no-arg → browser rooted there (default `/`), `-d`, `-n`, exit codes 0/1/2 | `main.c` |

Key data type (mirrors `wf_ace`): `Entry { kind: Access|Default, tag:
UserObj|User|GroupObj|Group|Mask|Other, id: u32, perms: u8 }` with a
`Model` holding `disk` and `staged` entry lists, owner/group/mode from
stat, `auto_mask`, `recursive`, and load status (Ok / NoAcl / Denied /
Missing). ACL-less filesystems degrade to read-only mode-bit display,
as in C.

## Crates

ratatui + crossterm (TUI; Unicode-width-safe rendering), rustix (xattr
+ stat syscalls, pure Rust), uzers (passwd/group), clap (CLI), walkdir
(recursive apply). Dev: tempfile. No C dependencies anywhere — this is
what makes the static musl build trivial.

## Error handling

- Recursive apply collects per-path errors into a report dialog;
  failures never abort the walk (C parity).
- Unreadable directories in the tree stay expandable, marked `!`, and
  recover on retry.
- Terminal restore on panic via a panic hook wrapping crossterm's
  raw-mode guard.

## Testing

- `cargo test`: port the scenario coverage of the C suites (197 model +
  65 tree checks) — same fixtures built under tempdirs at runtime.
- `tests/integration.sh`: adapted from the C repo; cross-checks apply
  results against getfacl/setfacl, dump output against `getfacl` and
  against C `winfacl -d` where available, pty smoke tests via
  `script(1)` for editor, browser, help, and navigation.
- CI runs both on every push.

## CI / Release (GitHub Actions)

- `ci.yml`: on push/PR — fmt check, clippy, cargo test, integration.sh
  (ubuntu-latest has getfacl/setfacl via the acl package).
- `release.yml`: on `v*` tags — build `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl` (cross via `cross` or musl-cross
  toolchain), strip, attach both binaries to a GitHub Release.

## Repo

`suchattai-labs/winfacl-rs`, private-by-default to match winfacl.
