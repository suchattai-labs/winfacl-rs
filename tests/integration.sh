#!/bin/sh
#
# integration.sh -- cross-check the winfacl-rs binary against the
# reference tools (getfacl/setfacl) and, when present, the C winfacl.
#
# Run from the repo root after `cargo build`. CI does exactly that.

set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/debug/winfacl"
CREF="${WINFACL_C:-$HOME/projects/winfacl/bin/winfacl}"

pass=0
fail=0

ok()  { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf '  FAIL %s\n' "$1"; }

check_eq() {
    if [ "$2" = "$3" ]; then
        ok "$1"
    else
        bad "$1"
        printf '       expected: %s\n' "$2"
        printf '       actual:   %s\n' "$3"
    fi
}

check_contains() {
    case "$2" in
    *"$3"*) ok "$1" ;;
    *)
        bad "$1"
        printf '       missing: %s\n' "$3"
        ;;
    esac
}

TMP=$(mktemp -d /tmp/winfacl-rs-it-XXXXXX) || exit 1
trap 'chmod -R u+rwx "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

# ------------------------------------------------------------------
echo "1. dump matches getfacl"
F="$TMP/dump"
: > "$F"
chmod 644 "$F"
setfacl -m u:root:rwx -m g:daemon:r "$F"
MINE=$("$BIN" -d "$F" | grep -v '^#' | grep -v '^$' | sed 's/\t.*//' | sort | tr '\n' ' ')
THEIRS=$(getfacl -c "$F" 2>/dev/null | grep -v '^#' | grep -v '^$' | sed 's/\t.*//' | sort | tr '\n' ' ')
check_eq "entry sets identical" "$THEIRS" "$MINE"

# ------------------------------------------------------------------
echo "2. effective annotations match getfacl exactly"
setfacl -n -m m::r-- "$F"
MINE=$("$BIN" -d "$F" | grep -v '^#')
THEIRS=$(getfacl -cp "$F" 2>/dev/null | grep -v '^$')
check_eq "annotated dump identical" "$THEIRS" "$MINE"
check_contains "effective annotation present" "$("$BIN" -d "$F")" "#effective:"

# ------------------------------------------------------------------
echo "3. dump matches the C winfacl (oracle)"
if [ -x "$CREF" ]; then
    D="$TMP/oracle-dir"
    mkdir -p "$D"
    setfacl -m u:root:rwx -d -m u:root:rwx -d -m g:daemon:r-x "$D"
    check_eq "byte-identical to C winfacl -d" "$("$CREF" -d "$D")" "$("$BIN" -d "$D")"
else
    echo "  skip (no C winfacl at $CREF)"
fi

# ------------------------------------------------------------------
echo "4. default ACL round trip"
D="$TMP/dir"
mkdir -p "$D"
setfacl -d -m u:root:rwx "$D"
check_contains "default entries dumped" "$("$BIN" -d "$D")" "default:user:root:rwx"

# ------------------------------------------------------------------
echo "5. error handling"
if "$BIN" -d /no/such/winfacl/path >/dev/null 2>&1; then
    bad "nonexistent path should exit non-zero"
else
    ok "nonexistent path exits non-zero"
fi
if "$BIN" -d >/dev/null 2>&1; then
    bad "dump without a path should exit non-zero"
else
    ok "dump without a path exits non-zero"
fi
"$BIN" --version >/dev/null 2>&1 && ok "--version works" || bad "--version broken"

# ------------------------------------------------------------------
echo "6. TUI smoke test under a pty"
smoke() {
    # $1 = path, $2 = keystrokes, $3 = expected string, $4 = label
    out=$(printf '%b' "$2" | timeout 20 script -qec \
        "stty rows 30 cols 110; TERM=xterm $BIN '$1'" /dev/null 2>&1)
    st=$?
    if [ $st -ne 0 ]; then
        bad "$4 (exit $st)"
        return
    fi
    case "$out" in
    *"$3"*) ok "$4" ;;
    *)
        bad "$4 (screen did not render)"
        printf '%s\n' "$out" | head -3
        ;;
    esac
}
SD="$TMP/smoke"
mkdir -p "$SD/sub"
: > "$SD/file1"
setfacl -m u:root:rwx -m g:daemon:rx "$SD/file1"
smoke "$SD/file1" 'q' "Advanced Security Settings" \
      "file arg opens the classic editor"
smoke "$SD/file1" '?xq' "winfacl Help" "help screen opens and closes"
smoke "$SD" 'q' "Filesystem" "directory arg opens the two-panel browser"
smoke "$SD" 'q' "Advanced Security Settings" \
      "browser preview shows the editor panel"
smoke "$SD" '\tqq' "Filesystem" \
      "Tab enters the editor, q returns to the tree, q quits"
smoke "$SD" 'jjq' "Filesystem" "tree navigation survives cursor movement"
smoke "$SD" '\033[<0;5;3M\033[<0;5;3mq' "Filesystem" \
      "mouse click in the tree is handled"
smoke "$SD" '\033[<64;5;3M\033[<65;5;3Mq' "Filesystem" \
      "scroll wheel is handled"

# ------------------------------------------------------------------
printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
