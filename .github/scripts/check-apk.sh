#!/bin/bash
# The OneOS APK that a OneiOS release carries holds no package set, no image and no firmware: nothing of Apple's,
# Samsung's, Google's or OPPO's (docs/releases.md, "What is never in a release").
#   .github/scripts/check-apk.sh APK PACKAGES_DIR
# PACKAGES_DIR is the fork's app-android/packages: its <set>.sha256 lists name every file of every Android package
# set (the OEM files: jars, apks, images, Samsung's camera tar). The build packs them only with
# -Poneos.packSets=true; the release build never passes it. The APK keeps its files content-addressed
# (assets/oneos/files/<sha256>) and its own lists (assets/oneos/sets/*.sha256, scripts.sha256 for the session
# scripts) name them; this script names each entry by those lists. Every entry is unpacked and refused when:
#   - its SHA-256 is a set file's (OneOSGuest.apk excepted: the fork's own in-guest app, app-android/guest);
#   - its name, or the name its hash has in the APK's lists, is a firmware, image or OTA name (Apple's, Android's
#     sparse and raw images, Samsung's Odin and lz4 files, Google's payload.bin and .new.dat, OPPO's
#     .ozip/.ofp/.ops, an image's .apex/.odex/.vdex/.oat, a .tar);
#   - its content is a Mach-O (a CAFEBABE with a Java class's version, such as kotlinx-coroutines'
#     DebugProbesKt.bin, is not one), an IMG4/IM4P/IM4M, an AEA archive, a dyld cache, an Android sparse image, or
#     (from 1 MiB) an ext4, erofs or APFS volume or a .dmg;
#   - it is over 64 MiB (the largest real entry, the debug guest_manager.apk, was 45.5 MB on 2026-11-15).
# Prints the APK's assets/ entries with their sizes and names. Exit 0: clean; 3: refused (the lines above say why);
# 2: usage.
set -euo pipefail
[ $# = 2 ] && [ -f "$1" ] && [ -d "$2" ] || { echo "usage: check-apk.sh APK PACKAGES_DIR" >&2; exit 2; }
apk=$1 packages=$2
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
sets=("$packages"/*.sha256)
[ -f "${sets[0]}" ] || { echo "check-apk: no set lists (*.sha256) in $packages" >&2; exit 2; }
cat "${sets[@]}" | awk '!/^#/ && NF == 2 && $2 != "OneOSGuest.apk" { print $1 }' | sort -u > "$W/set-hashes"
[ -s "$W/set-hashes" ] || { echo "check-apk: the set lists in $packages name no file" >&2; exit 2; }
mkdir "$W/x"
unzip -q "$apk" -d "$W/x"
# sha256 -> "name (list)" from the APK's own lists
: > "$W/names"
for l in "$W"/x/assets/oneos/sets/*.sha256; do
  [ -f "$l" ] || continue
  awk -v s="$(basename "$l" .sha256)" '!/^#/ && NF == 2 { print $1, $2 " (" s ")" }' "$l" >> "$W/names"
done
max=$((64 * 1024 * 1024))
bad=0
refuse() { echo "check-apk: refused $1: $2" >&2; bad=1; }
hex() { od -An -tx1 -j "$2" -N "$3" "$1" | tr -d ' \n'; }
name_rule() { # <entry> <lower-case name>
  case "$2" in
    kernelcache*|*cryptex*|*.ipsw|*.dmg|*.aea|*.im4p|*.img4|*.im4m|*.kc|*.trustcache|buildmanifest.plist|restore.plist|*.shsh*|dyld_shared_cache*|*.macho)
      refuse "$1" "an Apple firmware name ($2)" ;;
    *.img|*.simg|*.erofs|*.ext4|*.sparse|super*.img*|*.new.dat|*.new.dat.br|*.transfer.list|*.patch.dat|payload.bin|payload_properties.txt)
      refuse "$1" "an Android image or OTA name ($2)" ;;
    *.tar.md5|*.pit|*.lz4|sboot*.bin|param.bin|cm.bin|modem.bin|up_param.bin)
      refuse "$1" "a Samsung firmware name ($2)" ;;
    *.ozip|*.ofp|*.ops)
      refuse "$1" "an OPPO firmware name ($2)" ;;
    *.apex|*.capex|*.odex|*.vdex|*.oat|*.tar|*.tar.*)
      refuse "$1" "a system image's file or a package-set archive ($2)" ;;
  esac
}
echo "assets of $(basename "$apk") (bytes, path, the name of a content-addressed file):"
( cd "$W/x" && find . -type f | sed 's|^\./||' | LC_ALL=C sort ) > "$W/entries"
while IFS= read -r e; do
  f=$W/x/$e
  size=$(wc -c < "$f" | tr -d ' ')
  sum=$(sha256sum "$f" | cut -d' ' -f1)
  known=$(awk -v h="$sum" '$1 == h { $1 = ""; sub(/^ /, ""); print }' "$W/names" | paste -sd, -)
  case "$e" in assets/*) printf '  %10s  %s%s\n' "$size" "$e" "${known:+  = $known}" ;; esac
  name_rule "$e" "$(basename "$e" | tr '[:upper:]' '[:lower:]')"
  if [ -n "$known" ]; then
    while IFS= read -r k; do name_rule "$e" "$(printf '%s' "${k%% (*}" | tr '[:upper:]' '[:lower:]')"; done < <(tr , '\n' <<< "$known")
  fi
  [ "$size" -le "$max" ] || refuse "$e" "$size bytes, over 64 MiB"
  case "$(hex "$f" 0 4)" in
    feedface|feedfacf|cefaedfe|cffaedfe|cafebabf|bebafeca) refuse "$e" "a Mach-O" ;;
    cafebabe)
      # A fat Mach-O counts its architectures here (a few); a Java class has its version (45 or more).
      [ $((16#$(hex "$f" 4 4))) -ge 45 ] || refuse "$e" "a Mach-O (fat)" ;;
    41454131) refuse "$e" "an AEA archive" ;;
    3aff26ed) refuse "$e" "an Android sparse image" ;;
  esac
  if head -c 32 "$f" | LC_ALL=C grep -aq 'IM4P\|IMG4\|IM4M'; then refuse "$e" "an IMG4 container"; fi
  if [ "$(hex "$f" 0 7)" = 64796c645f7631 ]; then refuse "$e" "a dyld shared cache"; fi
  if [ "$size" -ge 1048576 ]; then
    [ "$(hex "$f" 1080 2)" != 53ef ] || refuse "$e" "an ext4 volume"
    [ "$(hex "$f" 1024 4)" != e0f5e1e2 ] || refuse "$e" "an erofs volume"
    [ "$(hex "$f" 32 4)" != 4e585342 ] || refuse "$e" "an APFS container"
    [ "$(tail -c 512 "$f" | od -An -tx1 -N4 | tr -d " \n")" != 6b6f6c79 ] || refuse "$e" "a .dmg disk image"
  fi
  if grep -qx "$sum" "$W/set-hashes"; then
    refuse "$e" "a package set's file ($sum, $(grep -l "^$sum " "${sets[@]}" | xargs -n1 basename | paste -sd, -))"
  fi
done < "$W/entries"
if [ "$bad" = 1 ]; then
  echo "check-apk: $(basename "$apk") refused: a OneiOS release carries no package set, image or firmware (docs/releases.md)" >&2
  exit 3
fi
echo "check-apk: $(basename "$apk"): $(wc -l < "$W/entries" | tr -d ' ') entries, no package set, image or firmware ($(wc -l < "$W/set-hashes" | tr -d ' ') set files checked)"
