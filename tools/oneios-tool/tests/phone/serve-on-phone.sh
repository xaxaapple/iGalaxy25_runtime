#!/system/bin/sh
# Measure `oneios-tool serve` on the phone itself (README "serve"; **not run on the phone yet**: written on a box
# without one). Run it as root with One UI up and no iOS session, from a PC or a Mac:
#
#   adb push oneios-tool-android /data/local/tmp/oneios-tool        # README "For the phone itself": the build
#   adb push tools/oneios-tool/tests/phone/serve-on-phone.sh /data/local/tmp/serve-on-phone.sh
#   adb shell su -c 'sh /data/local/tmp/serve-on-phone.sh' 2>&1 | tee serve-on-phone.txt
#   adb shell su -c 'sh /data/local/tmp/serve-on-phone.sh prelink' 2>&1 | tee serve-on-phone-prelink.txt   # see "prelink" below
#
# What it does: each request once with all threads and, where the work splits, once with one thread (`--jobs 1`),
# with the shell's `time`; of each run the last event and the `bye` line are printed. It reads the staged files
# (image/, runtime/, data/data.img, the session's logs) and writes only under /data/local/tmp/oneios/work, which
# it removes at the end. It reads no folder of /sys or /proc (the tool reads named files only), starts nothing
# but the tool, and the tool refuses the heavy requests while an iOS session is open. The tool runs at nice 10
# and stops at the heat and battery limits; a `refused` line with `heat` or `battery` means: wait, and run again.
#
#   sh serve-on-phone.sh            the readings, the stage's sums, the hash list, the APFS copy, iOS's log, a cancel
#   sh serve-on-phone.sh prelink    the prelink; its inputs must be in /data/local/tmp/oneios/work-prelink first:
#       adb push out/kernel/current-hosted/xnu.macho out/kernel-inputs/24A446/kernelcache.macho \
#           kernel/unixnu/profiles/os/ios-27.0.1-24A446/profile.toml kernel/prelink/reviewed-24A446-v54.json \
#           kernel/prelink/pairs-24A446.json kernel/prelink/stubs-24A446.json /data/local/tmp/prelink-in/
#       adb push kernel/prelink/lead kernel/prelink/user /data/local/tmp/prelink-in/
#       adb push external/XnuPrelink /data/local/tmp/prelink-in/XnuPrelink     # fileset_match.py, kc_image.py, fileset_shape.py, fixtures/
#       adb shell su -c 'rm -rf /data/local/tmp/oneios/work-prelink && mv /data/local/tmp/prelink-in /data/local/tmp/oneios/work-prelink'
#     (Apple's kernelcache: remove the folder afterwards; this script does when the prelink is done)
#   sh serve-on-phone.sh fetch URL  the IPSW's directory and its BuildManifest.plist over the phone's network
T=${T:-/data/local/tmp/oneios-tool}
S=${S:-/data/local/tmp/oneios}   # (S=DIR, NOT_ROOT_OK=1: a dry run on a PC with a stand-in stage)
W=$S/work
[ "$(id -u)" = 0 ] || [ -n "${NOT_ROOT_OK:-}" ] || { echo "serve-on-phone.sh: run it as root (adb shell su -c 'sh $0')"; exit 2; }
[ -x "$T" ] || { echo "serve-on-phone.sh: no $T (adb push oneios-tool-android $T; chmod 755)"; exit 2; }
[ -d "$S" ] || { echo "serve-on-phone.sh: no stage at $S"; exit 2; }

# q LABEL OPTIONS... REQUEST: one request with the shell's time; its last event and bye
q() {
  label=$1; shift
  echo "== $label"
  time "$T" serve --stage "$S" "$@" > "$W.out" 2> "$W.err"
  echo "   status $?"
  grep -E '"ev":"(ok|refused|failed|cancelled|bye|heat|space)"' "$W.out" | cut -c1-600 | sed 's/^/   /'
  [ ! -s "$W.err" ] || sed 's/^/   stderr: /' "$W.err"
}
# twice LABEL REQUEST: with all threads, then with one
# twice LABEL REQUEST: with one thread, with all, and with one again (the first run also reads the files into the
# page cache: the second and third runs are the ones to compare)
twice() { q "$1 (--jobs 1, first: reads the files from the disk)" --jobs 1 --request "$2"; q "$1 (all threads)" --request "$2"; q "$1 (--jobs 1)" --jobs 1 --request "$2"; }
finish() { rm -rf "$W" "$W.out" "$W.err"; }

case ${1:-all} in
all)
  rm -rf "$W"; mkdir -p "$W"
  q "hello, health, cpu, the logs' list" --request '{"id":"1","cmd":"health"}' --request '{"id":"2","cmd":"cpu"}' --request '{"id":"3","cmd":"log-list"}'
  grep '"ev":"hello"' "$W.out" | cut -c1-400 | sed 's/^/   /'
  grep -c '"ev":"row","id":"2","kind":"cpu"' "$W.out" | sed 's/^/   cpu rows: /'
  q "the last session's log: its stages and panics" --request '{"id":"1","cmd":"log","markers":true,"filters":["panic"],"lines":40}'
  twice "sums of image/ against image.sha256" '{"id":"1","cmd":"sums","dir":"'"$S"'/image"}'
  twice "sums of runtime/ against runtime.sha256" '{"id":"1","cmd":"sums","dir":"'"$S"'/runtime"}'
  twice "the chunk hash list of image/os.img" '{"id":"1","cmd":"hashes","image":"'"$S"'/image/os.img","out":"'"$W"'/os.img.hashes"}'
  cmp "$W/os.img.hashes" "$S/image/os.img.hashes" && echo "   the list is the staged os.img.hashes, byte for byte" || echo "   THE LIST DIFFERS from the staged os.img.hashes"
  q "verify image/ against its manifest.json (every chunk)" --request '{"id":"1","cmd":"verify","set":"'"$S"'/image"}'
  rm -rf "$W/usr-a" "$W/usr-1"
  q "apfs-copy of os.img /usr (--jobs 1, first: reads the image from the disk)" --jobs 1 --request '{"id":"1","cmd":"apfs-copy","image":"'"$S"'/image/os.img","src":"/usr","dst":"'"$W"'/usr-0"}'; rm -rf "$W/usr-0"
  q "apfs-copy of os.img /usr (all threads)" --request '{"id":"1","cmd":"apfs-copy","image":"'"$S"'/image/os.img","src":"/usr","dst":"'"$W"'/usr-a"}'
  q "apfs-copy of os.img /usr (--jobs 1)" --jobs 1 --request '{"id":"1","cmd":"apfs-copy","image":"'"$S"'/image/os.img","src":"/usr","dst":"'"$W"'/usr-1"}'
  # the two copies: every file's SHA-256 (the folder is ours, on /data)
  a=$(cd "$W/usr-a" && find . -type f | sort | xargs sha256sum | sha256sum); b=$(cd "$W/usr-1" && find . -type f | sort | xargs sha256sum | sha256sum)
  [ "$a" = "$b" ] && echo "   the two copies hold the same files ($a)" || echo "   THE TWO COPIES DIFFER"
  rm -rf "$W/usr-a" "$W/usr-1"
  if [ -f "$S/data/data.img" ]; then
    q "ioslog boots of data/data.img" --request '{"id":"1","cmd":"ioslog-boots","source":"'"$S"'/data/data.img"}'
    q "ioslog show of the last boot, into a file" --request '{"id":"1","cmd":"ioslog-show","source":"'"$S"'/data/data.img","boot":"last","out":"'"$W"'/ioslog.txt"}'
  else echo "== no $S/data/data.img: ioslog not measured"; fi
  # a cancel in the middle: SIGTERM to the process while it copies; the part must be gone
  echo "== SIGTERM in the middle of an apfs-copy"
  "$T" serve --stage "$S" --request '{"id":"1","cmd":"apfs-copy","image":"'"$S"'/image/os.img","src":"/System","dst":"'"$W"'/system"}' > "$W.out" 2> "$W.err" &
  pid=$!; sleep 2; kill -TERM $pid; wait $pid; echo "   status $? (5: the end cut a request short)"
  grep -E '"ev":"(cancelled|bye)"' "$W.out" | cut -c1-300 | sed 's/^/   /'
  [ ! -e "$W/system" ] && [ ! -e "$W/system.part" ] && echo "   nothing of the copy is left" || echo "   THE COPY LEFT FILES: $(ls "$W")"
  finish ;;
prelink)
  I=$S/work-prelink
  [ -f "$I/kernelcache.macho" ] && [ -f "$I/xnu.macho" ] && [ -f "$I/XnuPrelink/fileset_match.py" ] || { echo "serve-on-phone.sh: the prelink's inputs are not in $I (this script's header has the adb lines)"; exit 2; }
  mkdir -p "$W"
  more=; for f in "$I"/lead/reviewed-*-24A446-*.json "$I"/user/reviewed-*-24A446-*.json; do [ -f "$f" ] && more="$more${more:+,}\"$f\""; done
  r='"kernel":"'"$I"'/xnu.macho","kernelcache":"'"$I"'/kernelcache.macho","profile":"'"$I"'/profile.toml","reviewed":"'"$I"'/reviewed-24A446-v54.json","more_reviewed":['"$more"'],"pairs":"'"$I"'/pairs-24A446.json","stubs":"'"$I"'/stubs-24A446.json","xnuprelink":"'"$I"'/XnuPrelink"'
  q "prelink (all threads)" --request '{"id":"1","cmd":"prelink",'"$r"',"out":"'"$W"'/kc-a"}'
  q "prelink (--jobs 1)" --jobs 1 --request '{"id":"1","cmd":"prelink",'"$r"',"out":"'"$W"'/kc-1"}'
  sha256sum "$W/kc-a/kernel.kc" "$W/kc-1/kernel.kc" "$S/image/kernel.kc" | sed 's/^/   /'
  echo "   (the two must be the same; the staged one is the same when it was made from this kernel and these records)"
  rm -rf "$I"; finish ;;
fetch)
  url=${2:?serve-on-phone.sh fetch URL: the IPSW URL of the profile}
  mkdir -p "$W"
  q "the IPSW's directory" --request '{"id":"1","cmd":"zip-ls","source":"'"$url"'","match":"kernelcache|BuildManifest"}'
  grep '"ev":"row"' "$W.out" | cut -c1-300 | sed 's/^/   /'
  q "BuildManifest.plist out of the IPSW" --request '{"id":"1","cmd":"zip-get","source":"'"$url"'","outdir":"'"$W"'/ipsw","members":[{"member":"BuildManifest.plist"}]}'
  finish ;;
*) echo "serve-on-phone.sh [all | prelink | fetch URL]"; exit 2 ;;
esac
