#!/bin/bash
# tools/oneios-tool/tests/prelink/compare-build.sh   `oneios-tool prelink build` (the whole build in one process,
# its steps partly at the same time: what the phone and `oneios-tool serve` run) against kernel/prelink/build.sh
# (one step after the other), and against `serve`'s `prelink` request: the same kernel.kc, hash list and records,
# and the same work files, byte for byte. In the records that carry their folder's path, the path is taken out
# first; the record's two commits are compared apart (build.sh asks git, `serve` is given them or writes null).
#
# Needs the real inputs (Apple's kernelcache: never commit what this writes):
#   KERNEL=.../xnu.macho  [REFERENCE=.../reference.macho]
#   KERNELCACHE (default out/kernel-inputs/24A446/kernelcache.macho)
#   XNUPRELINK  (default external/XnuPrelink, else external/Droid-VM-UniXNU/kits/XnuPrelink, of this checkout or
#               of ONEIOS_MAIN)
#   ONEIOS_TOOL (default this checkout's release build), WORK (default out/rustport-prelink-build), JOBS (the
#               threads of `prelink build`, default: all), RUNS (how often each is timed, default 1)
# Prints each way's wall and CPU time, then one line per file, then `ALL THE SAME` (exit 0) or `DIFFERENT: N`.
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
main=${ONEIOS_MAIN:-$root}
tool=${ONEIOS_TOOL:-$root/tools/oneios-tool/target/release/oneios-tool}
cache=${KERNELCACHE:-$main/out/kernel-inputs/24A446/kernelcache.macho}
kernel=${KERNEL:?KERNEL=.../xnu.macho}
reference=${REFERENCE:-}
work=${WORK:-$root/out/rustport-prelink-build}
xp=${XNUPRELINK:-}
for d in "$root/external/XnuPrelink" "$root/external/Droid-VM-UniXNU/kits/XnuPrelink" "$main/external/XnuPrelink" "$main/external/Droid-VM-UniXNU/kits/XnuPrelink"; do
  [ -n "$xp" ] || { [ -f "$d/fileset_match.py" ] && xp=$d; }
done
[ -x "$tool" ] || { echo "no tool at $tool: build it or set ONEIOS_TOOL" >&2; exit 2; }
for f in "$cache" "$kernel" "$xp/fileset_match.py"; do [ -f "$f" ] || { echo "missing: $f" >&2; exit 2; }; done
data=$root/kernel/prelink
build=24A446
profile=$root/kernel/unixnu/profiles/os/ios-27.0.1-$build/profile.toml
more=$(ls "$data"/lead/reviewed-*-"$build"-*.json "$data"/user/reviewed-*-"$build"-*.json 2>/dev/null)
rm -rf "$work"; mkdir -p "$work"
TIMEFORMAT='%R %U %S'
timed() { # LABEL cmd...: the best wall time of RUNS runs
  local label=$1 best= r; shift
  for r in $(seq "${RUNS:-1}"); do
    { time "$@" > "$work/$label.out" 2>&1; } 2> "$work/.t"; local st=$?
    [ $st = 0 ] || { echo "$label FAILED (status $st)"; tail -n 5 "$work/$label.out"; exit 1; }
    read -r real user sys < "$work/.t"
    [ -z "$best" ] || awk -v a="$real" -v b="${best%% *}" 'BEGIN { exit !(a < b) }' && best="$real $user $sys"
  done
  read -r real user sys <<< "$best"
  printf '%-34s wall %6.2f s  user %6.2f s  sys %5.2f s  CPU %3.0f%%\n' "$label" "$real" "$user" "$sys" "$(awk -v r="$real" -v u="$user" -v s="$sys" 'BEGIN { print (u + s) * 100 / r }')"
}
sh_way() { rm -rf "$work/sh"; env ONEIOS_TOOL="$tool" KERNEL="$kernel" KERNELCACHE="$cache" XNUPRELINK="$xp" ${reference:+REFERENCE="$reference"} OUT="$work/sh" "$root/kernel/prelink/build.sh"; }
args=(--kernel "$kernel" --kernelcache "$cache" --profile "$profile" --reviewed "$data/reviewed-$build-v54.json" --pairs "$data/pairs-$build.json"
  --stubs "$data/stubs-$build.json" --xnuprelink "$xp" ${reference:+--reference "$reference"} --repo "$root")
for f in $more; do args+=(--more-reviewed "$f"); done
one_way() { rm -rf "$work/one"; "$tool" ${JOBS:+--jobs "$JOBS"} prelink build "${args[@]}" --out "$work/one"; }
one_serial() { rm -rf "$work/one1"; "$tool" --jobs 1 prelink build "${args[@]}" --out "$work/one1"; }
json() { printf '"%s"' "$1"; }
serve_way() {
  rm -rf "$work/serve"
  local list=; for f in $more; do list="$list${list:+,}$(json "$f")"; done
  "$tool" serve --stage "$work" --thermal off --battery-min 0 --request "{\"id\":\"1\",\"cmd\":\"prelink\",\"kernel\":$(json "$kernel"),\"kernelcache\":$(json "$cache"),\"profile\":$(json "$profile"),\"reviewed\":$(json "$data/reviewed-$build-v54.json"),\"more_reviewed\":[$list],\"pairs\":$(json "$data/pairs-$build.json"),\"stubs\":$(json "$data/stubs-$build.json"),\"xnuprelink\":$(json "$xp")${reference:+,\"reference\":$(json "$reference")},\"out\":$(json "$work/serve"),\"keep_work\":true${JOBS:+,\"jobs\":$JOBS}}"
}
timed "build.sh (step after step)" sh_way
timed "prelink build --jobs 1" one_serial
timed "prelink build${JOBS:+ --jobs $JOBS}" one_way
timed "serve: prelink" serve_way
grep -q '"ev":"ok","id":"1","cmd":"prelink"' "$work/serve: prelink.out" || { echo "serve's request did not end with ok"; tail -n 3 "$work/serve: prelink.out"; exit 1; }

# FILE as it is compared: the way's own folder out of the text (as a whole path, and as the record writes the reviewed
# file's: from the checkout), and the record's two commits
norm() { sed -e "s|$work/$1|OUT|g" -e "s|${work#"$root"/}/$1|OUT|g" -e 's/"oneios": [^,]*,/"oneios": COMMIT,/' -e 's/"xnuprelink": [^,]*,/"xnuprelink": COMMIT,/' "$2"; }
different=0
for way in one one1 serve; do
  n=0
  for f in "$work"/sh/kernel.kc "$work"/sh/kernel.kc.hashes "$work"/sh/kernel.kc.*.json "$work"/sh/work/*; do
    rel=${f#"$work/sh/"}
    case $rel in work/match.key|work/xnu.macho.*) continue ;; esac
    other=$work/$way/$rel
    if [ ! -f "$other" ]; then echo "$way: $rel is missing"; different=$((different + 1)); continue; fi
    case $rel in
    *.json) cmp -s <(norm sh "$f") <(norm "$way" "$other") || { echo "$way: $rel DIFFERS"; diff <(norm sh "$f") <(norm "$way" "$other") | head -n 4; different=$((different + 1)); continue; } ;;
    *) cmp -s "$f" "$other" || { echo "$way: $rel DIFFERS"; different=$((different + 1)); continue; } ;;
    esac
    n=$((n + 1))
  done
  extra=$(cd "$work/$way" && ls work | while read -r name; do [ -e "$work/sh/work/$name" ] || echo "$name"; done | tr '\n' ' ')
  echo "$way: $n files as build.sh's${extra:+; only here: $extra}"
done
echo "kernel.kc: $(sha256sum < "$work/sh/kernel.kc" | cut -d' ' -f1) (build.sh), $(sha256sum < "$work/one/kernel.kc" | cut -d' ' -f1) (prelink build), $(sha256sum < "$work/serve/kernel.kc" | cut -d' ' -f1) (serve)"
grep -h '"oneios"\|"xnuprelink"' "$work/sh/kernel.kc.build.json" "$work/serve/kernel.kc.build.json" | sed 's/^ */  record: /' | sort -u
[ -n "${KEEP:-}" ] || rm -rf "$work"
[ $different = 0 ] && echo "ALL THE SAME" || { echo "DIFFERENT: $different"; exit 1; }
