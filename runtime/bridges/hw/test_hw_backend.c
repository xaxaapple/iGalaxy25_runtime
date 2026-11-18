// SPDX-License-Identifier: MIT
// Host unit tests of the hardware backend layer (bridges/hw_backend.h): the settings, the merge of the command line
// over the image set's xnuhost.conf, the environment of the ctl tools, the choice and its log lines.
#define _GNU_SOURCE
#include "bridges/bridges.h"
#include "bridges/hw_backend.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int checks, failures;
#define CHECK(cond) do { checks++; if (!(cond)) { failures++; printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); } } while (0)

static char last[256];
static int last_level = -1;
static void sink(int level, const char *line) { snprintf(last, sizeof last, "%s", line); last_level = level; }

int main(void)
{
  br_hw_settings s, under;
  char why[64];
  br_log_sink = sink;

  // Nothing set: every bridge samsung, "the default", no fallback. A zeroed struct counts as nothing set too.
  br_hw_settings_init(&s);
  for (unsigned i = 0; i < BR_HW_NBRIDGE; i++) CHECK(br_hw_choose(&s, i, BR_HW_UNSET, why, sizeof why) == BR_HW_SAMSUNG && !strcmp(why, "the default"));
  CHECK(!br_hw_fallback_allowed(&s));
  br_hw_settings zero = { 0 };
  CHECK(br_hw_choose(&zero, BR_HW_BATTERY, BR_HW_UNSET, NULL, 0) == BR_HW_SAMSUNG && !br_hw_fallback_allowed(&zero));
  CHECK(br_hw_choose(NULL, BR_HW_BATTERY, BR_HW_UNSET, why, sizeof why) == BR_HW_SAMSUNG);

  // The keys: hw-backend, hw-backend-<bridge>, hw-backend-fallback; other keys are not ours; bad values refused.
  CHECK(br_hw_setting(&s, "hw-backend", "treble") == 0 && s.all == BR_HW_TREBLE);
  CHECK(br_hw_setting(&s, "hw-backend-touch", "samsung") == 0 && s.one[BR_HW_TOUCH] == BR_HW_SAMSUNG);
  CHECK(br_hw_setting(&s, "hw-backend-backlight", "treble") == 0 && s.one[BR_HW_BACKLIGHT] == BR_HW_TREBLE);
  CHECK(br_hw_setting(&s, "hw-backend-fallback", "1") == 0 && s.fallback == 1 && br_hw_fallback_allowed(&s));
  CHECK(br_hw_setting(&s, "hw-backend-fallback", "no") == 0 && s.fallback == 0 && !br_hw_fallback_allowed(&s));
  CHECK(br_hw_setting(&s, "hw-backend", "pixel") == -1 && s.all == BR_HW_TREBLE);
  CHECK(br_hw_setting(&s, "hw-backend-fallback", "maybe") == -1 && s.fallback == 0);
  CHECK(br_hw_setting(&s, "hw-backend-toaster", "treble") == -1);
  CHECK(br_hw_setting(&s, "hw-backend-battery", "") == -1 && s.one[BR_HW_BATTERY] == BR_HW_UNSET);
  CHECK(br_hw_setting(&s, "haptics", "sim") == 1 && br_hw_setting(&s, "hw-backendx", "treble") == 1);
  const char *names[] = { "backlight", "battery", "thermal", "haptics", "touch", "keys", "audio", "camera", "wifi", "bluetooth", "display", "encoder" };
  for (unsigned i = 0; i < BR_HW_NBRIDGE; i++) {
    char key[40];
    snprintf(key, sizeof key, "hw-backend-%s", names[i]);
    br_hw_settings t;
    br_hw_settings_init(&t);
    CHECK(!strcmp(br_hw_bridge_name(i), names[i]) && br_hw_setting(&t, key, "treble") == 0 && t.one[i] == BR_HW_TREBLE);
  }
  CHECK(!strcmp(br_hw_bridge_name(BR_HW_NBRIDGE), "?"));

  // The choice: per bridge over hw-backend over the default; a forced one (a test, a ctl tool) over all.
  CHECK(br_hw_choose(&s, BR_HW_TOUCH, BR_HW_UNSET, why, sizeof why) == BR_HW_SAMSUNG && !strcmp(why, "hw-backend-touch=samsung"));
  CHECK(br_hw_choose(&s, BR_HW_BACKLIGHT, BR_HW_UNSET, why, sizeof why) == BR_HW_TREBLE && !strcmp(why, "hw-backend-backlight=treble"));
  CHECK(br_hw_choose(&s, BR_HW_THERMAL, BR_HW_UNSET, why, sizeof why) == BR_HW_TREBLE && !strcmp(why, "hw-backend=treble"));
  CHECK(br_hw_choose(&s, BR_HW_THERMAL, BR_HW_SAMSUNG, why, sizeof why) == BR_HW_SAMSUNG && !strcmp(why, "the caller's choice"));

  // The command line over the image set's xnuhost.conf: what the first leaves unset comes from the second.
  br_hw_settings_init(&s);
  br_hw_settings_init(&under);
  CHECK(br_hw_setting(&s, "hw-backend-battery", "samsung") == 0);
  CHECK(br_hw_setting(&under, "hw-backend", "treble") == 0 && br_hw_setting(&under, "hw-backend-battery", "treble") == 0);
  CHECK(br_hw_setting(&under, "hw-backend-haptics", "treble") == 0 && br_hw_setting(&under, "hw-backend-fallback", "1") == 0);
  br_hw_settings_merge(&s, &under);
  CHECK(s.all == BR_HW_TREBLE && s.one[BR_HW_BATTERY] == BR_HW_SAMSUNG && s.one[BR_HW_HAPTICS] == BR_HW_TREBLE && s.fallback == 1);
  br_hw_settings_init(&under);
  CHECK(br_hw_setting(&s, "hw-backend-fallback", "0") == 0);
  br_hw_settings_merge(&s, &under);
  CHECK(s.fallback == 0);

  // The environment (the ctl tools).
  br_hw_settings_init(&s);
  const char *bad = NULL;
  setenv("ONEIOS_HW_BACKEND", "treble", 1);
  setenv("ONEIOS_HW_BACKEND_KEYS", "samsung", 1);
  setenv("ONEIOS_HW_FALLBACK", "1", 1);
  CHECK(br_hw_settings_env(&s, &bad) == 0 && s.all == BR_HW_TREBLE && s.one[BR_HW_KEYS] == BR_HW_SAMSUNG && s.fallback == 1);
  setenv("ONEIOS_HW_BACKEND_BATTERY", "lg", 1);
  CHECK(br_hw_settings_env(&s, &bad) == -1 && bad && !strcmp(bad, "ONEIOS_HW_BACKEND_BATTERY"));
  unsetenv("ONEIOS_HW_BACKEND"); unsetenv("ONEIOS_HW_BACKEND_KEYS"); unsetenv("ONEIOS_HW_FALLBACK"); unsetenv("ONEIOS_HW_BACKEND_BATTERY");

  // The process's settings and the log lines. Not configured: samsung, the default.
  CHECK(br_hw_select(BR_HW_BATTERY, BR_HW_UNSET) == BR_HW_SAMSUNG && !strcmp(last, "hw: battery: backend samsung (the default)") && last_level == 1);
  br_hw_settings_init(&s);
  CHECK(br_hw_setting(&s, "hw-backend", "treble") == 0);
  br_hw_configure(&s);
  CHECK(br_hw_select(BR_HW_THERMAL, BR_HW_UNSET) == BR_HW_TREBLE && !strcmp(last, "hw: thermal: backend treble (hw-backend=treble)"));
  CHECK(!br_hw_missing(BR_HW_THERMAL, "android.hardware.thermal.IThermal/default is not registered") && last_level == 0 &&
        !strcmp(last, "hw: thermal: treble: missing: android.hardware.thermal.IThermal/default is not registered; the bridge stays off (hw-backend-fallback=0)"));
  CHECK(br_hw_setting(&s, "hw-backend-fallback", "1") == 0);
  br_hw_configure(&s);
  CHECK(br_hw_missing(BR_HW_HAPTICS, "x") && last_level == 1 && !strcmp(last, "hw: haptics: treble: missing: x; falling back to samsung (hw-backend-fallback=1)"));
  br_hw_configure(NULL);
  CHECK(br_hw_select(BR_HW_HAPTICS, BR_HW_UNSET) == BR_HW_SAMSUNG && !br_hw_fallback_allowed(br_hw_current()));

  printf("hw_backend: %d checks, %d failures\n", checks, failures);
  return failures != 0;
}
