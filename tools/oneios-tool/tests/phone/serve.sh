# `oneios-tool serve` on the stand-in phone's stage (sourced by run.sh): the local forms the OneOS app uses give
# what the adb forms give of the same stage.
#   serve_checks_closed   no session: `sums` against the stage's image.sha256 and against `phone sums`; `log-list`
#                         and `log` against `phone log`; `cpu` (this machine's /sys, as the adb form's script reads
#                         it on the stand-in); a write outside the stage is refused; a download into the stage
#   serve_checks_open     a session is open: the heavy requests are refused with `session`, the readings run
# The requests go in with --request (the one-shot form): one JSON line each on standard output.
# Uses: TOOL, W, P (the stand-in phone), S (its stage), FAKE_PROC; ok, bad (lib.sh).

# serve REQUEST-JSON...: the lines of one run, in $W/serve.out; the status is serve's.
serve() {
  local args=() r
  for r in "$@"; do args+=(--request "$r"); done
  # the stand-in's /proc is the host's, or the stand-in's own (fake-rewrite.sh): the session check reads it
  local root=; [ "$FAKE_PROC" = stand-in ] && root=$P/.fake
  "$TOOL" serve --stage "$S" ${root:+--sys-root "$root"} --thermal off --battery-min 0 "${args[@]}" > "$W/serve.out" 2> "$W/serve.err"
}
# field LINE-PATTERN KEY: the value of "KEY":VALUE in the first line of serve.out that matches
field() { grep -E -- "$1" "$W/serve.out" | head -n 1 | sed -E "s/.*\"$2\":(\"([^\"]*)\"|([^,}]*)).*/\\2\\3/"; }

serve_checks_closed() {
  local n
  # every line is the tool's, the first is hello and the last is bye with nothing left behind
  serve '{"id":"s","cmd":"sums","dir":"'"$S"'/image"}'; local st=$?
  n=$(grep -c . "$S/image.sha256")
  [ $st = 0 ] && [ "$(grep -vc '^{"ev":' "$W/serve.out")" = 0 ] && [ ! -s "$W/serve.err" ] && head -n 1 "$W/serve.out" | grep -q '^{"ev":"hello","proto":1,' \
    && tail -n 1 "$W/serve.out" | grep -Eq '^\{"ev":"bye","why":"requests","requests":1,"cancelled":0,("threads":1,)?"children":0\}$' \
    && ok "serve: one request, every line begins with {\"ev\":, hello first, bye last with one thread and no child" || bad "serve: the lines" "status $st: $(tail -n 2 "$W/serve.out" | tr '\n' '|')$(cat "$W/serve.err")"
  [ "$(field '"ev":"ok","id":"s"' files)" = "$n" ] && [ "$(grep -c '"ev":"row","id":"s","kind":"file".*"ok":true' "$W/serve.out")" = "$n" ] \
    && ok "serve sums: the $n files of image.sha256, each as listed" || bad "serve sums" "$(grep '"id":"s"' "$W/serve.out" | tail -n 1)"
  # the same sums as the adb form's `phone sums` (sha256sum on the phone) and as sha256sum here
  local same=1 path sum
  while read -r sum path; do
    [ "$(field "\"kind\":\"file\",\"path\":\"$path\"" sha256)" = "$sum" ] && [ "$(sha256sum < "$S/image/$path" | cut -d' ' -f1)" = "$sum" ] || same=0
  done < "$S/image.sha256"
  "$TOOL" phone sums > "$W/sums.adb" 2>&1
  [ $same = 1 ] && grep -q "^$(field '"path":"kernel.kc"' sha256)  .*image/kernel.kc$" "$W/sums.adb" \
    && ok "serve sums: each row's SHA-256 is sha256sum's, and kernel.kc's is \`phone sums\`'s" || bad "serve sums against sha256sum and phone sums" "$(grep kernel.kc "$W/sums.adb" | head -n 1)"
  # a changed file is refused with its name; the stage is as it was afterwards
  cp "$S/image/kernel.kc" "$W/kernel.kc.keep"; echo changed >> "$S/image/kernel.kc"
  serve '{"id":"s","cmd":"sums","dir":"'"$S"'/image"}'; st=$?
  cp "$W/kernel.kc.keep" "$S/image/kernel.kc"
  [ $st = 2 ] && grep -q '"ev":"refused","id":"s","cmd":"sums","code":"mismatch","text":"1 of '"$n"' files are not as .*kernel.kc (another SHA-256)' "$W/serve.out" \
    && ok "serve sums: a changed kernel.kc is refused (mismatch, status 2), named" || bad "serve sums of a changed file" "status $st: $(grep '"id":"s"' "$W/serve.out" | tail -n 1)"

  # the logs: the names and sizes `phone log --list` gives, and a log's lines as `phone log` prints them
  [ ! -e "$S/switch.log" ] || cp "$S/switch.log" "$W/switch.log.keep"
  printf '[    1.000] xnuhost I one\n[    2.000] guest     oneios-gpu-report: two\n[    3.000] guest     panic(cpu 0 caller 0x1): three\n' > "$S/switch.log"
  serve '{"id":"l","cmd":"log-list"}' '{"id":"g","cmd":"log","file":"switch","filters":["gpu","panic"]}' '{"id":"t","cmd":"log","file":"switch","tail":1}'
  "$TOOL" phone log --list > "$W/list.adb" 2>&1
  local size; size=$(stat -c %s "$S/switch.log")
  grep -Eq "^switch +$size bytes" "$W/list.adb" && [ "$(field '"kind":"log","name":"switch"' bytes)" = "$size" ] \
    && [ "$(grep -c '"ev":"row","id":"l","kind":"log"' "$W/serve.out")" -ge 7 ] \
    && ok "serve log-list: the seven named logs, switch.log's size as \`phone log --list\`'s" || bad "serve log-list" "$(grep '"name":"switch"' "$W/serve.out"; grep '^switch' "$W/list.adb")"
  "$TOOL" phone log --file switch --gpu --panic > "$W/log.adb" 2>/dev/null
  grep '"ev":"line","id":"g"' "$W/serve.out" | sed -E 's/.*"text":"(.*)"\}$/\1/' > "$W/log.serve"
  [ -s "$W/log.adb" ] && cmp -s "$W/log.adb" "$W/log.serve" && [ "$(field '"ev":"ok","id":"g"' matched)" = 2 ] \
    && ok "serve log: the filtered lines are \`phone log --gpu --panic\`'s, as line events" || bad "serve log against phone log" "$(diff "$W/log.adb" "$W/log.serve" | head -n 3 | tr '\n' '|')"
  [ "$(field '"ev":"ok","id":"t"' lines)" = 1 ] && grep -q '"ev":"line","id":"t","stream":"out","text":"\[    3.000\] guest     panic(cpu 0 caller 0x1): three"' "$W/serve.out" \
    && ok "serve log: tail 1 is the last line" || bad "serve log tail"
  rm -f "$S/switch.log"; [ ! -e "$W/switch.log.keep" ] || mv "$W/switch.log.keep" "$S/switch.log"

  # the CPU check: the local form reads the files the adb form's script names. On the stand-in the script's /sys
  # is the stand-in's own (empty), so the two are compared where both read the same files: this machine's /sys
  # for the local form, and the script itself run here for the adb form (Linux; a Mac has no /sys)
  if [ -r /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq ] || [ -r /sys/devices/system/cpu/cpu0/cpu_capacity ]; then
    "$TOOL" serve --stage "$S" --request '{"id":"c","cmd":"cpu"}' > "$W/serve.out" 2> "$W/serve.err"
    grep '"ev":"line","id":"c"' "$W/serve.out" | sed -E 's/.*"text":"(.*)"\}$/\1/; s/  now .*//' | grep '^cpu[0-9]' > "$W/cpu.serve"
    n=$(grep -c . "$W/cpu.serve")
    local have=0 c
    for c in 0 1 2 3 4 5 6 7; do [ -e /sys/devices/system/cpu/cpu$c/cpufreq/cpuinfo_max_freq ] || [ -e /sys/devices/system/cpu/cpu$c/cpu_capacity ] && have=$((have + 1)); done
    [ "$n" = "$have" ] && [ "$(grep -c '"ev":"row","id":"c","kind":"cpu"' "$W/serve.out")" = "$have" ] && grep -q "governor $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null)" "$W/cpu.serve" \
      && ok "serve cpu: the $have CPUs this machine names (cpu0 to cpu7), each a line and a row, cpu0's governor as its file says" || bad "serve cpu" "$n lines for $have CPUs: $(head -n 2 "$W/cpu.serve" | tr '\n' '|')$(cat "$W/serve.err")"
  else echo "skipped serve cpu (this machine has no /sys/devices/system/cpu/cpu0 files)"; fi

  # nothing is written outside the stage
  serve '{"id":"o","cmd":"hashes","image":"'"$S"'/image/kernel.kc","out":"'"$W"'/outside.hashes"}'; st=$?
  [ $st = 2 ] && grep -q '"code":"outside"' "$W/serve.out" && [ ! -e "$W/outside.hashes" ] && [ ! -e "$W/outside.hashes.part" ] \
    && ok "serve: a write outside the stage is refused (outside, status 2), nothing written" || bad "serve: a write outside the stage" "status $st"
  serve '{"id":"h","cmd":"hashes","image":"'"$S"'/image/kernel.kc","out":"'"$S"'/work/kernel.kc.hashes"}'; st=$?
  "$TOOL" image hashes "$S/image/kernel.kc" "$W/kernel.kc.hashes" > /dev/null 2>&1
  [ $st = 0 ] && cmp -s "$S/work/kernel.kc.hashes" "$W/kernel.kc.hashes" && ok "serve hashes: the list \`image hashes\` writes, in the stage" || bad "serve hashes" "status $st"

  # a download into the stage (from a file here: the same pieces, state file and SHA-256 check as from a server,
  # which crates/oneios-tool/tests/serve.rs has): the file, byte for byte, and no part and no state file beside it;
  # `hello` names the two requests that hold a download and let it go on
  local sum; sum=$(sha256sum < "$S/image/kernel.kc" | cut -d' ' -f1)
  serve '{"id":"d","cmd":"download","url":"'"$S"'/image/kernel.kc","out":"'"$S"'/work/dl/kernel.kc","sha256":"'"$sum"'","resume":true,"connections":2}'; st=$?
  [ $st = 0 ] && cmp -s "$S/image/kernel.kc" "$S/work/dl/kernel.kc" && [ "$(ls "$S/work/dl" | tr '\n' ' ')" = "kernel.kc " ] \
    && [ "$(field '"ev":"ok","id":"d"' sha256)" = "$sum" ] && [ "$(field '"ev":"ok","id":"d"' resumed_from)" = 0 ] \
    && head -n 1 "$W/serve.out" | grep -q '"cmds":\["ping","health","cancel","pause","resume",' \
    && ok "serve download: kernel.kc into the stage with its SHA-256, no part and no state file left; hello names pause and resume" || bad "serve download" "status $st: $(grep '"id":"d"' "$W/serve.out" | tail -n 1 | cut -c1-200) $(ls "$S/work/dl" 2>/dev/null | tr '\n' ' ')"
  # another SHA-256 than the file's: refused, and nothing is kept (not the part either, whatever "resume" says)
  serve '{"id":"d","cmd":"download","url":"'"$S"'/image/kernel.kc","out":"'"$S"'/work/dl/other.kc","sha256":"'"$(printf x | sha256sum | cut -d' ' -f1)"'","resume":true}'; st=$?
  [ $st = 2 ] && grep -q '"ev":"refused","id":"d","cmd":"download","code":"mismatch"' "$W/serve.out" && [ "$(ls "$S/work/dl" | tr '\n' ' ')" = "kernel.kc " ] \
    && ok "serve download: a file with another SHA-256 is refused (mismatch, status 2), nothing of it is kept" || bad "serve download of a wrong file" "status $st: $(ls "$S/work/dl" | tr '\n' ' ')"
  rm -rf "$S/work"
}

serve_checks_open() {
  serve '{"id":"s","cmd":"sums","dir":"'"$S"'/image"}'; local st=$?
  [ $st = 2 ] && grep -q '"ev":"refused","id":"s","cmd":"sums","code":"session","text":"An iOS session is open: return to One UI first"' "$W/serve.out" \
    && ok "serve in a session: sums is refused (session, status 2; $FAKE_PROC /proc)" || bad "serve in a session: sums" "status $st: $(grep '"id":"s"' "$W/serve.out" | tail -n 1)"
  serve '{"id":"h","cmd":"health"}' '{"id":"l","cmd":"log","file":"xnuhost","markers":true}' '{"id":"c","cmd":"wait","seconds":0.05}'; st=$?
  [ $st = 0 ] && grep -q '"ev":"ok","id":"h","cmd":"health".*"session":true' "$W/serve.out" && grep -q '"ev":"ok","id":"l","cmd":"log"' "$W/serve.out" \
    && grep -q '"ev":"line","id":"l","stream":"out","text":".*xnuhost I xnuhost: session ABI' "$W/serve.out" \
    && ok "serve in a session: health says so, and the session's log is read beside it" || bad "serve in a session: the readings" "status $st: $(tail -n 3 "$W/serve.out" | tr '\n' '|')"
}
