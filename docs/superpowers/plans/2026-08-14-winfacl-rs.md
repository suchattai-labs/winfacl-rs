# winfacl-rs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Feature-parity Rust port of the C winfacl (two-panel POSIX ACL TUI) shipping as static musl binaries via GitHub Actions.

**Architecture:** Pure-logic ACL model + lazy fs tree (both headless-tested), ratatui UI on top, raw xattr syscalls via rustix instead of libacl. The C repo at `~/projects/winfacl` is the behavioral reference; each task cites the C functions whose behavior it ports, and the C binary is a test oracle.

**Tech Stack:** Rust stable, ratatui, crossterm, rustix, uzers, clap, walkdir, tempfile (dev).

**Spec:** `docs/superpowers/specs/2026-08-14-winfacl-rs-design.md`

## Global Constraints

- No C dependencies anywhere (blocks static musl build).
- `-d` output byte-identical to C `winfacl -d` / `getfacl` (minus getfacl's leading-slash warning).
- CLI contract: file arg → editor, dir/no-arg → browser (default root `/`), `-n/--no-follow`, `-d/--dump`, `-h`, `-V`; exit 0 ok / 1 runtime / 2 usage.
- Every logic module carries unit tests; TDD per task; commit per task.
- Keymap identical to C version (see help text in `dialogs.c:wf_dialog_help`).

---

### Task 1: Scaffold + CI skeleton
**Files:** Create `Cargo.toml`, `.gitignore`, `rustfmt.toml`, `.github/workflows/ci.yml`, `src/main.rs` (hello stub).
- [ ] `cargo init`, add deps (ratatui, crossterm, rustix `features=["fs","xattr"]`, uzers, clap `features=["derive"]`, walkdir; dev: tempfile)
- [ ] ci.yml: push/PR → `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `sudo apt-get install acl && sh tests/integration.sh` (integration.sh added Task 8; guard with `[ -f ]` until then)
- [ ] `cargo build` passes; commit "chore: scaffold"

### Task 2: ACL core types + entry list
**Files:** Create `src/acl/mod.rs`, `src/acl/model.rs`. Reference: `acl_model.h`, `acl_model.c` (types, `wf_acelist_*`).
**Produces:** `Kind{Access,Default}`, `Tag{UserObj,User,GroupObj,Group,Mask,Other}` (ord = canonical order), `Perms(u8)` bitflags r=4 w=2 x=1, `Entry{kind,tag,id:u32,perms}`, `EntryList(Vec<Entry>)` with `set/find/remove/sort_canonical/count_named`.
- [ ] Tests first: canonical ordering (user<named users(by id)<group<named groups<mask<other; access before default), set-replaces-same-principal, find, remove. Port scenarios from `test_model.c:test_set_find_remove/test_canonical_order`.
- [ ] Implement; tests pass; commit.

### Task 3: Mask calc, auto-mask, validation, effective access
**Files:** Modify `src/acl/model.rs`. Reference: `acl_model.c` (`wf_calc_mask`, `wf_apply_auto_mask`, validation, `wf_effective_access`).
**Produces:** `EntryList::calc_mask(kind)->Perms` (union of named users+groups+group_obj), `apply_auto_mask(kind)` (upsert mask iff named entries exist), `validate()->Result<(),String>` (required base entries, mask required with named, no default on files handled at model level, duplicate principals), `effective(uid,gids)->Effective{granted,limited_by_mask,matched}` — owner unmasked, named user & all matching groups masked (union of matching groups), other unmasked.
- [ ] Port test scenarios: `test_mask_calc`, `test_auto_mask`, `test_validate`, `test_effective_*` (owner, named-user-masked, group-class union, other-ignores-mask).
- [ ] Implement; commit.

### Task 4: getfacl formatting
**Files:** Modify `src/acl/model.rs`; create `src/acl/names.rs`. Reference: `wf_names.c`, `acl_model.c` (`wf_format_getfacl`, perm strings).
**Produces:** `names::uid_name(u32)->String` / `gid_name` (numeric fallback), `lookup_user/group(&str)->Option<u32>` (accepts bare numerics), `user_groups(uid)->Vec<u32>`; `Model::format_getfacl()->String`: `# file:`/`# owner:`/`# group:` header, entries in canonical order, `\t#effective:rwx` only when mask clips.
- [ ] Tests: perm string "r--" etc; header + entry lines + effective annotation cases; orphan uid prints number. Port `test_formatting`, `test_getfacl_format`, `test_name_lookup`.
- [ ] Implement; commit.

### Task 5: xattr ACL I/O
**Files:** Create `src/acl/xattr.rs`. Format: u32 LE version=2 header; entries of u16 tag (1=USER_OBJ,2=USER,4=GROUP_OBJ,8=GROUP,16=MASK,32=OTHER), u16 perms, u32 qualifier (0xFFFFFFFF for non-named). Attrs `system.posix_acl_access`, `system.posix_acl_default`.
**Produces:** `read_acl(path,kind,follow)->Result<Option<Vec<Entry>>,Errno>` (None = no xattr → synthesize from mode bits happens in model), `write_acl(path,kind,&[Entry])`, `remove_default(path)`.
- [ ] Tests (real fs in tempdir): encode/decode roundtrip; write then `getfacl` via std::process::Command compare; read what `setfacl` wrote; ENODATA → None; ENOTSUP detection.
- [ ] Implement with rustix::fs::{getxattr,setxattr,lgetxattr,...}; commit.

### Task 6: Model load/apply + recursive walk
**Files:** Modify `src/acl/model.rs`; create `src/acl/apply.rs`. Reference: `acl_model.c` (`wf_model_load`, `wf_model_apply`, `wf_walk`, `wf_count_tree`, report).
**Produces:** `Model::load(path,follow)->Model` with `status: LoadStatus{Ok,NoEnt,Denied,NotSup,Other(Errno)}`, mode-bit synthesis when no xattr, `is_dir/is_symlink/owner/group/mode`, `staged` vs `disk`, `dirty()`, `revert()`, `copy_access_to_default()`, `remove_default()`; `apply::apply(model,&mut Report)->Result` — single or recursive (walkdir, skip symlinks, collect per-path errors, never abort), `count_tree(path,cap)`.
- [ ] Tests: load roundtrip vs setfacl fixtures, dirty tracking, revert, recursive apply touches all + unreadable subtree reported not fatal (port `test_roundtrip_*`, `test_recursive_apply`, `test_load_nonexistent`).
- [ ] Implement; commit.

### Task 7: fs tree
**Files:** Create `src/tree.rs`. Reference: `tree_model.c` + `test_tree.c` (all 9 scenarios).
**Produces:** `Node{name,is_dir,is_symlink,expanded,loaded,load_failed,load_errno,children}` in an arena `Tree{nodes:Vec<Node>,root:NodeId}` (indices, not pointers); `expand/collapse/refresh/path(id)->PathBuf/depth/flatten()->Vec<NodeId>`; dirs-first alpha sort; lazy load; unreadable recovery; refresh preserves expanded grandchildren by name.
- [ ] Port all `test_tree.c` scenarios as Rust tests; implement; commit.

### Task 8: CLI + dump mode + integration harness
**Files:** Create `tests/integration.sh` (adapt from C repo), modify `src/main.rs`.
**Produces:** clap CLI per Global Constraints; `-d` prints `Model::format_getfacl`; routing: dir/no-arg → `ui::browser::run` (stub returning unimplemented error for now), file → `ui::editor::run` (stub).
- [ ] integration.sh sections 1–9 of C version adapted: dump vs getfacl (+ vs C winfacl -d if present), apply-path tests driven through a `wfapply`-equivalent `--apply-spec` hidden test flag OR reuse cargo-test coverage and keep script sections: dump match, effective annotation, error handling. pty smoke deferred to Task 11.
- [ ] Dump output matches getfacl on the aclplay2 fixtures; commit.

### Task 9: TUI infra + dialogs
**Files:** Create `src/ui/mod.rs`, `src/ui/term.rs` (raw-mode guard + panic hook), `src/ui/dialogs.rs`. Reference: `dialogs.c`.
**Produces:** modal engine over ratatui: `msgbox(title,body)`, `confirm(title,body)->bool`, `input` line editor, `pick_principal(kind)->Option<String>` (filterable list from uzers), `entry_dialog(model,edit_idx)->bool`, `effective_dialog(model)`, `report_dialog(&Report)`, `help()`. Dialogs run their own small event loops centered on the full frame.
- [ ] Editor-independent; smoke-tested in Task 11's pty run. Unit-test pure helpers (line wrap). Commit.

### Task 10: Editor widget
**Files:** Create `src/ui/editor.rs`. Reference: `ui.c` (row merge logic `rows_build`, columns, buttons, keymap, embedded semantics).
**Produces:** `EditorState{model,rows,sel,top,focus,btn}` with `rows_build` (merge access+default rows), `render(frame,area,active)`, `handle_key(key)->EditorEvent{Continue,Back,Quit}`; standalone `run(model)` wrapper. Embedded: q/Esc → Back; OK applies then Back.
- [ ] Keymap parity: j/k/arrows/PgUp/PgDn/Home/End/Tab/BTab, a e r f s o c, d D m R u, ?/F1, Enter. Commit.

### Task 11: Browser + pty smoke tests
**Files:** Create `src/ui/browser.rs`; extend `tests/integration.sh` section 10. Reference: `browser.c`.
**Produces:** two-panel layout (tree 35% clamp 24–44 cols), live preview on cursor move, dirty prompt before leaving (revert on discard), Tab/e/Enter-on-file → editor, expand/collapse/refresh keys, load-error panel, resize handling (ratatui makes this nearly free).
- [ ] pty smoke: file→editor renders "Advanced Security Settings"; dir→"Filesystem"; Tab-q-q flow; jjq navigation; ?-x-q help. All via `script(1)` as in C. Full integration.sh green. Commit.

### Task 12: Release workflow + docs + publish
**Files:** Create `.github/workflows/release.yml`, `README.md`; finalize.
- [ ] release.yml: on `v*` tag → x86_64-musl (native musl-tools) and aarch64-musl (via `cross`), strip, upload to GitHub Release.
- [ ] Local proof: `cargo build --release --target x86_64-unknown-linux-musl`, `ldd` says "statically linked", binary runs the browser on aclplay2.
- [ ] README (port of C README + install-from-release section). `gh repo create suchattai-labs/winfacl-rs --private`, push, verify CI green, tag `v0.9.0`, verify release artifacts.

## Self-Review Notes
- Spec coverage: all spec sections map to tasks (xattr→5, model→2-4+6, tree→7, UI→9-11, CI/release→1+12, dump parity→4+8).
- Type names used consistently: `Entry`, `EntryList`, `Model`, `Report`, `LoadStatus`, `Tree`/`NodeId`, `EditorState`, `EditorEvent`.
- No placeholder steps: content lives either inline or in a cited C function that is the authoritative behavior.
