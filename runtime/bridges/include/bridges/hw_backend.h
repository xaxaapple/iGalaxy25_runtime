// SPDX-License-Identifier: MIT
// The hardware backend layer: which form of the phone's hardware each bridge uses (docs/hardware.md, "Hardware
// backends", the audit table; docs/architecture.md, "The backend layer").
//   samsung   today's paths, the default: the Galaxy S24 Ultra's own nodes and names (sec_*, the panel's backlight
//             node, Samsung's thermistors, the force-feedback device sec_vibrator_inputff, ...). Unchanged.
//   treble    the vendor-neutral interfaces any Treble/GSI phone has: the stable AIDL HALs through servicemanager
//             (health, thermal, vibrator, ...), evdev found by its capabilities, the standard sysfs classes found by
//             named paths (never a directory walk).
// Settings (xnuhost.conf, `--set KEY=VALUE` or `--hw-backend X` on the command line; the ctl tools: the environment
// ONEIOS_HW_BACKEND, ONEIOS_HW_BACKEND_<BRIDGE>, ONEIOS_HW_FALLBACK):
//   hw-backend=samsung|treble           every bridge (default samsung)
//   hw-backend-<bridge>=samsung|treble  one bridge (backlight, battery, thermal, haptics, touch, keys, audio, camera,
//                                       wifi, bluetooth, display): wins over hw-backend
//   hw-backend-fallback=0|1             a treble bridge whose interface is missing falls back to samsung (1) or stays
//                                       off (0, the default)
// Pure apart from br_hw_note/br_hw_missing, which log through br_log. No locks: xnuhost configures it once, before the
// bridges open.
#ifndef ONEIOS_BRIDGES_HW_BACKEND_H
#define ONEIOS_BRIDGES_HW_BACKEND_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// A backend. 0 is "unset": in the settings, not given; in a bridge's options, the process's setting.
enum { BR_HW_UNSET = 0, BR_HW_SAMSUNG = 1, BR_HW_TREBLE = 2 };

enum {
  BR_HW_BACKLIGHT, BR_HW_BATTERY, BR_HW_THERMAL, BR_HW_HAPTICS, BR_HW_TOUCH, BR_HW_KEYS,
  BR_HW_AUDIO, BR_HW_CAMERA, BR_HW_WIFI, BR_HW_BLUETOOTH, BR_HW_DISPLAY, BR_HW_ENCODER,
  BR_HW_NBRIDGE
};

typedef struct {
  int8_t all;                             // hw-backend: BR_HW_*
  int8_t one[BR_HW_NBRIDGE];              // hw-backend-<bridge>
  int8_t fallback;                        // hw-backend-fallback: -1 unset, 0, 1
} br_hw_settings;

void br_hw_settings_init(br_hw_settings *s);                    // everything unset
// One setting. 0 taken; -1 a key of ours with a bad value; 1 not a key of ours.
int  br_hw_setting(br_hw_settings *s, const char *key, const char *value);
// What `s` leaves unset is taken from `under` (the command line over the image set's xnuhost.conf).
void br_hw_settings_merge(br_hw_settings *s, const br_hw_settings *under);
// The environment's (ONEIOS_HW_BACKEND, ONEIOS_HW_BACKEND_BACKLIGHT, ..., ONEIOS_HW_FALLBACK): the ctl tools.
// 0, or -1 with the bad variable's name in *bad.
int  br_hw_settings_env(br_hw_settings *s, const char **bad);
const char *br_hw_name(int backend);                            // "samsung", "treble", "unset"
const char *br_hw_bridge_name(unsigned bridge);                 // "backlight", ... ("?" past the end)
int  br_hw_parse_name(const char *text);                        // BR_HW_SAMSUNG, BR_HW_TREBLE, or -1

// The choice for one bridge: forced (a bridge's options, a test; BR_HW_UNSET: none), else hw-backend-<bridge>, else
// hw-backend, else samsung. `why` (may be NULL) says which: "hw-backend-touch=treble", "hw-backend=treble",
// "the default", "the caller's choice".
int  br_hw_choose(const br_hw_settings *s, unsigned bridge, int forced, char *why, size_t size);
bool br_hw_fallback_allowed(const br_hw_settings *s);

// The process's settings: xnuhost sets them from its config before any bridge opens; a tool from the environment.
// Before that they are all unset (every bridge samsung, no fallback).
void br_hw_configure(const br_hw_settings *s);
const br_hw_settings *br_hw_current(void);
// The choice for a bridge from the process's settings, with its one log line:
//   "hw: backlight: backend treble (hw-backend=treble)"
int  br_hw_select(unsigned bridge, int forced);
// A treble interface that is missing: one line, and whether the bridge may use samsung instead:
//   "hw: battery: treble: missing: <what>; falling back to samsung (hw-backend-fallback=1)"   -> true
//   "hw: battery: treble: missing: <what>; the bridge stays off (hw-backend-fallback=0)"       -> false
bool br_hw_missing(unsigned bridge, const char *what);

#endif
