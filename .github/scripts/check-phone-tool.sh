#!/bin/bash
# CI and release: the phone's oneios-tool that runtime/package.sh wrote (docs/releases.md, "oneios-tool for the phone").
#   .github/scripts/check-phone-tool.sh VERSION       reads out/release (OUT_DIR)
# - out/release/oneios-tool-VERSION-android-arm64 and its .sha256: the line checks, and names that file;
# - the same bytes as bin/oneios-tool of out/release/oneios-runtime-VERSION-arm64.tar.zst, listed in its runtime.json
#   with mode 0755;
# - an arm64 position-independent executable for /system/bin/linker64, stripped (no .symtab, no DWARF .debug_info/.debug_line/...; the allocated .debug_gdb_scripts stays).
# The package's own checks (`oneios-tool package`, elf-check.sh) ran on it already in runtime/package.sh.
set -euo pipefail
VERSION=${1:?usage: check-phone-tool.sh VERSION}
OUT=${OUT_DIR:-out/release}
TOOL=oneios-tool-$VERSION-android-arm64
PKG=oneios-runtime-$VERSION-arm64.tar.zst
fail() { echo "::error::$*" >&2; exit 1; }
[ -f "$OUT/$TOOL" ] && [ -f "$OUT/$TOOL.sha256" ] || fail "no $OUT/$TOOL or its .sha256"
[ "$(awk '{ print $2 }' "$OUT/$TOOL.sha256")" = "$TOOL" ] || fail "$TOOL.sha256 does not name $TOOL"
( cd "$OUT" && sha256sum -c "$TOOL.sha256" ) || fail "$TOOL does not match its .sha256"
d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT
zstd -dc "$OUT/$PKG" | tar -x -C "$d" runtime.json bin/oneios-tool || fail "$PKG has no bin/oneios-tool"
cmp -s "$OUT/$TOOL" "$d/bin/oneios-tool" || fail "$TOOL is not the package's bin/oneios-tool"
# runtime.json's entry, on one line whatever the manifest's layout: path, size, sha256 and mode in that order.
entry=$(tr -d '\n' < "$d/runtime.json" | tr -s ' ' | grep -o '{ *"path": "bin/oneios-tool"[^}]*}' || true)
sum=$(cut -d' ' -f1 "$OUT/$TOOL.sha256")
case "$entry" in
  *"\"sha256\": \"$sum\""*'"mode": "0755"'*) ;;
  *) fail "runtime.json does not list bin/oneios-tool with its SHA-256 and mode 0755 (${entry:-no entry})" ;;
esac
READELF=$(echo "${ANDROID_NDK_ROOT:?}"/toolchains/llvm/prebuilt/*/bin/llvm-readelf)
# Read once: `readelf | grep -q` fails under pipefail when grep's early exit gives readelf SIGPIPE.
h=$("$READELF" -h -l -S "$OUT/$TOOL")
grep -q 'Machine: *AArch64' <<< "$h" || fail "$TOOL is not arm64"
grep -q 'Type: *DYN' <<< "$h" || fail "$TOOL is not position-independent"
grep -q '/system/bin/linker64' <<< "$h" || fail "$TOOL does not name /system/bin/linker64"
if grep -q -E ' \.(symtab|debug_(info|line|abbrev|str|ranges|rnglists|loclists|aranges)) ' <<< "$h"; then fail "$TOOL is not stripped"; fi
echo "$TOOL: $(wc -c < "$OUT/$TOOL" | tr -d ' ') bytes, stripped, the package's bin/oneios-tool; $(cat "$OUT/$TOOL.sha256")"
