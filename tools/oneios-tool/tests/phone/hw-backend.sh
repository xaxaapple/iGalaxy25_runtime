# Sourced by tests/phone/run.sh (with lib.sh's helpers, HERE, ROOT and W set): the bridges' two hardware backends
# (hw-backend=samsung|treble, docs/hardware.md 7e) on stand-in phones of their own. The bridges' phone tools
# (brightctl, ...; their host builds, made here with the build host's cc) run against each stand-in's sysfs, and for
# the HAL-based treble forms against a fake servicemanager with fake HALs on a binderfs (fake-hal, runtime/bridges/
# tests/fake_hal.c, in a user namespace; skipped with a line where the kernel has no binderfs):
#   s24            the S24 Ultra as the bridges see it: sys/class/backlight/panel0-backlight (max 51000, Samsung's
#                  doubled range), sys/class/sec/tsp (the touch IC's power node, name sec_touchscreen),
#                  sys/class/power_supply/{battery,usb}, sys/class/thermal/thermal_zone{0,1}, sys/class/sec/sec-ap-thermistor;
#                  .fake/hal/{health,thermal} (the same battery and temperatures for the HALs)
#   other          another phone: sys/class/backlight/lcd-backlight (max 2047), no sec class, no power_supply nodes;
#                  .fake/hal/{health,thermal} (its battery and temperatures only through the HALs)
#   haptics: s24/dev/input is empty (an evdev force-feedback device cannot be faked without the kernel); .fake/hal/vibrator
#                  serves the fake vibrator HAL, whose calls are .fake/hal/vibrator.log
#   odd            a phone whose backlight has a vendor's own name (vendor-bl): samsung's folder listing finds it, the
#                  treble backend (named paths only) does not
# Checked per backend: which node is used and its range, the line naming the backend and why, what is written and
# restored, what is left alone (the touch IC's node in treble), the one "missing" line and the fallback rule.
# The stand-ins are under a short folder of their own (mktemp -d /tmp/oneios-hw.XXXXXX), removed at the end.
hw_backend_checks() {
  local B=$ROOT/runtime/bridges out st
  if ! command -v cc > /dev/null 2>&1 || ! make -s -C "$B/brightness" build/host/brightctl > "$W/hw-build.txt" 2>&1; then
    echo "skipped hw-backend (the bridges' host tools do not build here: $(tail -n 1 "$W/hw-build.txt" 2>/dev/null))"; return
  fi
  local BRIGHTCTL=$B/brightness/build/host/brightctl
  # the stand-ins live under a short folder of their own: the bridges keep a sysfs path in 200 bytes, as the phone's are
  local T; T=$(mktemp -d /tmp/oneios-hw.XXXXXX) || { bad "hw-backend: no scratch folder"; return; }
  local S24=$T/s24 OTHER=$T/other ODD=$T/odd
  mkdir -p "$S24/sys/class/backlight/panel0-backlight" "$S24/sys/class/sec/tsp/input" "$OTHER/sys/class/backlight/lcd-backlight" \
    "$ODD/sys/class/backlight/vendor-bl"
  echo 51000 > "$S24/sys/class/backlight/panel0-backlight/max_brightness"; echo 9000 > "$S24/sys/class/backlight/panel0-backlight/brightness"
  echo sec_touchscreen > "$S24/sys/class/sec/tsp/input/name"; echo 0,0 > "$S24/sys/class/sec/tsp/enabled"
  echo 2047 > "$OTHER/sys/class/backlight/lcd-backlight/max_brightness"; echo 700 > "$OTHER/sys/class/backlight/lcd-backlight/brightness"
  echo 510 > "$ODD/sys/class/backlight/vendor-bl/max_brightness"; echo 100 > "$ODD/sys/class/backlight/vendor-bl/brightness"
  bl() { local p=$1; shift; env ASAN_OPTIONS=detect_leaks=0 ONEIOS_BRIGHT_DIR="$p/sys/class/backlight" ONEIOS_TSP_DIR="$p/sys/class/sec" ONEIOS_INPUT_DIR="$p/dev/input" "$@" "$BRIGHTCTL" "${BL_ARGS[@]}"; }

  # ---- backlight: samsung (the default) on the S24: unchanged, the doubled range halved, the touch IC woken
  BL_ARGS=(info)
  out=$(bl "$S24" 2>&1); st=$?
  [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^bridges: hw: backlight: backend samsung (the default)$' && printf '%s\n' "$out" | grep -q '^backend: samsung$' \
    && printf '%s\n' "$out" | grep -q 'panel: .*/panel0-backlight/brightness  max_brightness 51000, slider range 1..25500, One UI.s value 9000' \
    && [ "$(cat "$S24/sys/class/sec/tsp/enabled")" = 2,1 ] \
    && ok "hw-backend: backlight samsung (the default) on the S24: panel0-backlight, 1..25500 of 51000, the touch IC woken" || bad "hw-backend: backlight samsung on the S24" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  # ---- treble on the S24: the standard class name, the whole range, the touch IC's node not written
  echo 0,0 > "$S24/sys/class/sec/tsp/enabled"
  out=$(bl "$S24" ONEIOS_HW_BACKEND=treble 2>&1); st=$?
  [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^bridges: hw: backlight: backend treble (hw-backend=treble)$' \
    && printf '%s\n' "$out" | grep -q 'slider range 1..51000, One UI.s value 9000' && printf '%s\n' "$out" | grep -q 'touch: treble: no vendor-neutral power node' \
    && printf '%s\n' "$out" | grep -q 'light sensor: treble: missing: .*off (hw-backend-fallback=0)' && [ "$(cat "$S24/sys/class/sec/tsp/enabled")" = 0,0 ] \
    && ok "hw-backend: backlight treble on the S24: panel0-backlight by name, 1..51000, the touch IC and the light sensor left alone" || bad "hw-backend: backlight treble on the S24" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  # levels: written and One UI's value back, both backends
  BL_ARGS=(levels --ms 100 100)
  out=$(bl "$S24" ONEIOS_HW_BACKEND_BACKLIGHT=treble 2>&1); st=$?
  [ $st = 0 ] && printf '%s\n' "$out" | grep -q 'backend treble (hw-backend-backlight=treble)' && printf '%s\n' "$out" | grep -q '^slider 100% -> wanted 51000, the node was written 51000$' \
    && [ "$(cat "$S24/sys/class/backlight/panel0-backlight/brightness")" = 9000 ] \
    && ok "hw-backend: backlight treble levels: 51000 written, One UI's 9000 back" || bad "hw-backend: backlight treble levels" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  out=$(bl "$S24" 2>&1); st=$?
  [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^slider 100% -> wanted 25500, the node was written 25500$' && [ "$(cat "$S24/sys/class/backlight/panel0-backlight/brightness")" = 9000 ] \
    && ok "hw-backend: backlight samsung levels: 25500 written (the manual top), One UI's 9000 back" || bad "hw-backend: backlight samsung levels" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  # ---- another phone: both find lcd-backlight (samsung by listing the class, treble by its name)
  BL_ARGS=(info)
  for be in samsung treble; do
    out=$(bl "$OTHER" ONEIOS_HW_BACKEND=$be 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q "^backend: $be$" && printf '%s\n' "$out" | grep -q 'lcd-backlight/brightness  max_brightness 2047, slider range 1..2047, One UI.s value 700' \
      && ok "hw-backend: backlight $be on another phone: lcd-backlight, 1..2047" || bad "hw-backend: backlight $be on another phone" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  done
  # ---- a vendor's own name: treble says what is missing in one line and stays off; with the fallback, samsung's form
  out=$(bl "$ODD" ONEIOS_HW_BACKEND=treble 2>&1); st=$?
  [ $st = 1 ] && [ "$(printf '%s\n' "$out" | grep -c 'treble: missing:')" = 1 ] && printf '%s\n' "$out" | grep -q 'hw: backlight: treble: missing: no backlight class device .*; the bridge stays off (hw-backend-fallback=0)' \
    && [ "$(cat "$ODD/sys/class/backlight/vendor-bl/brightness")" = 100 ] \
    && ok "hw-backend: backlight treble, no standard name: one missing line, the bridge off, nothing written" || bad "hw-backend: backlight treble, no standard name" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  out=$(bl "$ODD" ONEIOS_HW_BACKEND=treble ONEIOS_HW_FALLBACK=1 2>&1); st=$?
  [ $st = 0 ] && printf '%s\n' "$out" | grep -q 'falling back to samsung (hw-backend-fallback=1)' && printf '%s\n' "$out" | grep -q '^backend: samsung$' \
    && printf '%s\n' "$out" | grep -q 'vendor-bl/brightness  max_brightness 510, slider range 1..255' \
    && ok "hw-backend: backlight treble with hw-backend-fallback=1: samsung's form finds vendor-bl" || bad "hw-backend: backlight fallback" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  out=$(bl "$S24" ONEIOS_HW_BACKEND=pixel 2>&1); st=$?
  [ $st = 2 ] && printf '%s\n' "$out" | grep -q 'bad ONEIOS_HW_BACKEND' && ok "hw-backend: a backend that does not exist is refused" || bad "hw-backend: a bad backend" "status $st: $out"

  # ---- the HAL-based treble forms: a fake servicemanager and HALs (fake-hal) on a binderfs in a user namespace
  local HAVE_HAL=1
  if ! grep -qw binder /proc/filesystems 2>/dev/null || ! unshare -Urm true 2>/dev/null; then
    HAVE_HAL=; echo "skipped hw-backend's HAL checks (no binderfs in this kernel, or no user namespaces)"
  elif ! make -s -C "$B" build/host/fake-hal > "$W/hw-build.txt" 2>&1; then
    HAVE_HAL=; bad "hw-backend: fake-hal does not build" "$(tail -n 2 "$W/hw-build.txt" | tr '\n' '|')"
  fi
  mkdir -p "$S24/.fake/hal" "$OTHER/.fake/hal"
  # with_hal PHONE CMD...: CMD with ONEIOS_BINDER set, beside fake-hal serving PHONE/.fake/hal (gone when CMD ends)
  with_hal() {
    local p=$1; shift
    mkdir -p "$T/binderfs"
    unshare -Urm bash -c 'mount -t binder binder "$1" || exit 90; export ONEIOS_BINDER=$1/binder; hal=$2; shift 2
      "$B/build/host/fake-hal" "$hal" > "$T/fake-hal.out" 2>&1 & f=$!
      for i in $(seq 50); do grep -q ready "$T/fake-hal.out" && break; sleep 0.05; done
      "$@"; st=$?; kill -9 $f; wait $f 2>/dev/null; exit $st' sh "$T/binderfs" "$p/.fake/hal" "$@"
  }
  export B T

  # ---- battery: samsung (the nodes) on the S24, treble (the health HAL) on the S24 and on another phone
  if [ -n "$HAVE_HAL" ] && make -s -C "$B/battery" build/host/batteryctl > "$W/hw-build.txt" 2>&1; then
    local BATCTL=$B/battery/build/host/batteryctl PS=$S24/sys/class/power_supply
    mkdir -p "$PS/battery" "$PS/usb"
    echo 64 > "$PS/battery/capacity"; echo Charging > "$PS/battery/status"; echo 1 > "$PS/battery/present"; echo 301 > "$PS/battery/temp"
    echo 4100000 > "$PS/battery/voltage_now"; echo 900 > "$PS/battery/current_now"; echo 1 > "$PS/usb/online"
    printf 'level=64\nstatus=2\npresent=1\ntemp=301\nvoltage_mv=4100\ncurrent_ua=900000\nusb=1\nfull_uah=4900000\ndesign_uah=5000000\n' > "$S24/.fake/hal/health"
    printf 'level=18\nstatus=3\npresent=1\ntemp=355\nvoltage_mv=3650\ncurrent_ua=-610000\ncycles=320\nfull_uah=3700000\ndesign_uah=4500000\n' > "$OTHER/.fake/hal/health"
    bat() { local p=$1; shift; with_hal "$p" env ASAN_OPTIONS=detect_leaks=0 ONEIOS_BATTERY_DIR="$p/sys/class/power_supply" "$@" "$BATCTL"; }
    out=$(bat "$S24" 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^bridges: hw: battery: backend samsung (the default)$' && printf '%s\n' "$out" | grep -q "^bridges: battery: on: $PS, status \"Charging\", 64%, plugged in" \
      && printf '%s\n' "$out" | grep -q '^sample: 64% present=1 plugged=1 charging=1 full=0 temp=301 (tenths C) voltage=4100 mV current=900 mA' \
      && ok "hw-backend: battery samsung (the default) on the S24: the power_supply nodes, 64%, charging, 900 mA" || bad "hw-backend: battery samsung on the S24" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(bat "$S24" ONEIOS_HW_BACKEND=treble 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^bridges: hw: battery: backend treble (hw-backend=treble)$' \
      && printf '%s\n' "$out" | grep -q '^bridges: battery: on: the health HAL android.hardware.health.IHealth/default (version 2), status "Charging", 64%, plugged in' \
      && printf '%s\n' "$out" | grep -q '^sample: 64% present=1 plugged=1 charging=1 full=0 temp=301 (tenths C) voltage=4100 mV current=900 mA full=4900 mAh design=5000 mAh' \
      && ok "hw-backend: battery treble on the S24: the health HAL, the same 64%, charging, 900 mA" || bad "hw-backend: battery treble on the S24" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(bat "$OTHER" ONEIOS_HW_BACKEND_BATTERY=treble 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q 'backend treble (hw-backend-battery=treble)' \
      && printf '%s\n' "$out" | grep -q '^sample: 18% present=1 plugged=0 charging=0 full=0 temp=355 (tenths C) voltage=3650 mV current=-610 mA full=3700 mAh design=4500 mAh cycles=320' \
      && ok "hw-backend: battery treble on another phone (no power_supply nodes): 18%, -610 mA, 320 cycles" || bad "hw-backend: battery treble on another phone" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(bat "$OTHER" 2>&1); st=$?
    [ $st = 1 ] && printf '%s\n' "$out" | grep -q 'battery: off: no readable' && ok "hw-backend: battery samsung on another phone with no power_supply nodes: off" || bad "hw-backend: battery samsung on another phone" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    rm "$S24/.fake/hal/health"
    out=$(bat "$S24" ONEIOS_HW_BACKEND=treble 2>&1); st=$?
    [ $st = 1 ] && [ "$(printf '%s\n' "$out" | grep -c 'treble: missing:')" = 1 ] \
      && printf '%s\n' "$out" | grep -q 'hw: battery: treble: missing: android.hardware.health.IHealth/default (no service: not registered .*); the bridge stays off (hw-backend-fallback=0)' \
      && ok "hw-backend: battery treble, no health HAL: one missing line, off" || bad "hw-backend: battery treble, no health HAL" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(bat "$S24" ONEIOS_HW_BACKEND=treble ONEIOS_HW_FALLBACK=1 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q 'falling back to samsung (hw-backend-fallback=1)' && printf '%s\n' "$out" | grep -q '^backend: samsung$' && printf '%s\n' "$out" | grep -q '^sample: 64%' \
      && ok "hw-backend: battery treble, no health HAL, hw-backend-fallback=1: the nodes" || bad "hw-backend: battery fallback" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  elif [ -n "$HAVE_HAL" ]; then bad "hw-backend: batteryctl does not build" "$(tail -n 2 "$W/hw-build.txt" | tr '\n' '|')"; fi

  # ---- thermal sensors: samsung (the zones, Samsung's thermistors, lrp) and treble (the thermal HAL); the same level
  if [ -n "$HAVE_HAL" ] && make -s -C "$B/thermal" build/host/thermalctl > "$W/hw-build.txt" 2>&1; then
    local THCTL=$B/thermal/build/host/thermalctl TZ=$S24/sys/class/thermal
    mkdir -p "$TZ/thermal_zone0" "$TZ/thermal_zone1" "$S24/sys/class/sec/sec-ap-thermistor" "$S24/sys/class/power_supply/battery"
    echo cpu-1-2 > "$TZ/thermal_zone0/type"; echo 61200 > "$TZ/thermal_zone0/temp"; echo gpuss-0 > "$TZ/thermal_zone1/type"; echo 55000 > "$TZ/thermal_zone1/temp"
    echo 365 > "$S24/sys/class/sec/sec-ap-thermistor/temperature"; echo 304 > "$S24/sys/class/power_supply/battery/temp"; echo 412 > "$S24/sys/class/power_supply/battery/lrp"
    printf '0 cpu-1-2 61.2\n1 gpu 55.0\n2 battery 30.4\n3 skin 41.2\n' > "$S24/.fake/hal/thermal"
    printf '0 cpu0 45.0\n0 cpu7 52.5\n13 soc 53.0\n2 battery 29.0\n3 skin-therm 35.5\n' > "$OTHER/.fake/hal/thermal"
    th() { local p=$1; shift; with_hal "$p" env ASAN_OPTIONS=detect_leaks=0 ONEIOS_THERMAL_ROOT="$p" "$@" "$THCTL"; }
    out=$(th "$S24" 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^backend: samsung$' && printf '%s\n' "$out" | grep -q '^zones: 2 of interest$' \
      && printf '%s\n' "$out" | grep -q '^reading (C): cpu 61.2 gpu 55.0 ddr - soc 61.2 (cpu-1-2) xo - ap 36.5 battery 30.4 lrp 41.2; case = 41.2 (lrp)$' \
      && printf '%s\n' "$out" | grep -q '^level: light (10)' \
      && ok "hw-backend: thermal samsung (the default) on the S24: 2 zones, ap, lrp; case 41.2 C, light" || bad "hw-backend: thermal samsung on the S24" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(th "$S24" ONEIOS_HW_BACKEND=treble 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^backend: treble$' && printf '%s\n' "$out" | grep -q 'thermal: on: the thermal HAL android.hardware.thermal.IThermal/default (version 2), 4 temperatures' \
      && printf '%s\n' "$out" | grep -q '^reading (C): cpu 61.2 gpu 55.0 ddr - soc 61.2 (cpu-1-2) xo - ap - battery 30.4 lrp -; case = 41.2 (the HAL.s SKIN)$' \
      && printf '%s\n' "$out" | grep -q '^level: light (10)' \
      && ok "hw-backend: thermal treble on the S24: the thermal HAL, the same case 41.2 C and the same level" || bad "hw-backend: thermal treble on the S24" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(th "$OTHER" ONEIOS_HW_BACKEND_THERMAL=treble 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q '^reading (C): cpu 52.5 gpu - ddr - soc 53.0 (soc) xo - ap - battery 29.0 lrp -; case = 35.5 (the HAL.s SKIN)$' \
      && printf '%s\n' "$out" | grep -q '^level: nominal' && ok "hw-backend: thermal treble on another phone: the HAL's CPU, SOC, battery and skin" || bad "hw-backend: thermal treble on another phone" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    rm "$OTHER/.fake/hal/thermal"
    out=$(th "$OTHER" ONEIOS_HW_BACKEND=treble 2>&1); st=$?
    [ $st = 1 ] && [ "$(printf '%s\n' "$out" | grep -c 'treble: missing:')" = 1 ] && printf '%s\n' "$out" | grep -q 'hw: thermal: treble: missing: android.hardware.thermal.IThermal/default (no service: .*the bridge stays off' \
      && ok "hw-backend: thermal treble, no thermal HAL: one missing line, off" || bad "hw-backend: thermal treble, no thermal HAL" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  elif [ -n "$HAVE_HAL" ]; then bad "hw-backend: thermalctl does not build" "$(tail -n 2 "$W/hw-build.txt" | tr '\n' '|')"; fi

  # ---- haptics: samsung looks for the evdev force-feedback device (none can be faked here: it needs the kernel's
  # ff-memless or a vibrator driver), treble plays through the fake vibrator HAL, whose calls are its log
  if [ -n "$HAVE_HAL" ] && make -s -C "$B" build/host/bridgectl > "$W/hw-build.txt" 2>&1; then
    local BCTL=$B/build/host/bridgectl VL=$S24/.fake/hal/vibrator.log
    mkdir -p "$S24/dev/input"
    echo 4 > "$S24/.fake/hal/vibrator"
    rm -f "$VL"
    out=$(with_hal "$S24" env ASAN_OPTIONS=detect_leaks=0 ONEIOS_INPUT_DIR="$S24/dev/input" "$BCTL" haptic 40 2>&1); st=$?
    [ $st = 1 ] && printf '%s\n' "$out" | grep -q 'hw: haptics: backend samsung (the default)' && printf '%s\n' "$out" | grep -q 'haptics: off: no force-feedback device with FF_PERIODIC + FF_SINE under' \
      && [ ! -e "$VL" ] && ok "hw-backend: haptics samsung (the default): the evdev search, the vibrator HAL never called" || bad "hw-backend: haptics samsung" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
    out=$(with_hal "$S24" env ASAN_OPTIONS=detect_leaks=0 ONEIOS_INPUT_DIR="$S24/dev/input" ONEIOS_HW_BACKEND=treble "$BCTL" haptic 40 2>&1); st=$?
    [ $st = 0 ] && printf '%s\n' "$out" | grep -q 'haptics: on: the vibrator HAL android.hardware.vibrator.IVibrator/default (version 2, capabilities 0x4: amplitude control' \
      && printf '%s\n' "$out" | grep -q 'effect 1 played on the vibrator HAL: 40 ms' && [ "$(tr "\n" " " < "$VL")" = "on 40 amplitude 1.00 off off " ] \
      && ok "hw-backend: haptics treble: the vibrator HAL plays on(40) at amplitude 1, off when it ends and again at the close (the motor is stopped whatever the worker believed)" || bad "hw-backend: haptics treble" "status $st: $(printf '%s' "$out" | tr '\n' '|'); HAL: $(tr '\n' ' ' < "$VL" 2>/dev/null)"
    rm -f "$VL" "$S24/.fake/hal/vibrator"
    out=$(with_hal "$S24" env ASAN_OPTIONS=detect_leaks=0 ONEIOS_INPUT_DIR="$S24/dev/input" ONEIOS_HW_BACKEND_HAPTICS=treble "$BCTL" haptic 40 2>&1); st=$?
    [ $st = 1 ] && [ "$(printf '%s\n' "$out" | grep -c 'treble: missing:')" = 1 ] && printf '%s\n' "$out" | grep -q 'hw: haptics: treble: missing: android.hardware.vibrator.IVibrator/default (no service: .*the bridge stays off' \
      && ! printf '%s\n' "$out" | grep -q 'force-feedback' && ok "hw-backend: haptics treble, no vibrator HAL: one missing line, off, no evdev search" || bad "hw-backend: haptics treble, no vibrator HAL" "status $st: $(printf '%s' "$out" | tr '\n' '|')"
  elif [ -n "$HAVE_HAL" ]; then bad "hw-backend: bridgectl does not build" "$(tail -n 2 "$W/hw-build.txt" | tr '\n' '|')"; fi
  rm -rf "$T"
}
