// SPDX-License-Identifier: MIT
// batteryctl: what the battery bridge reads on this phone. Run as root with One UI up; reads only.
//   batteryctl            the sample once, and what iOS would be told
//   batteryctl watch [S]  a line whenever the sample changes, for S seconds (default 60): plug and unplug
// The backend: ONEIOS_HW_BACKEND, ONEIOS_HW_BACKEND_BATTERY (samsung, the default: the power_supply nodes; treble: the
// health HAL), ONEIOS_HW_FALLBACK (bridges/hw_backend.h). Off the phone: ONEIOS_BATTERY_DIR (the power_supply class
// folder), ONEIOS_BINDER (the binder device, for the treble backend).
#define _GNU_SOURCE
#include "battery.h"
#include "bridges/bridges.h"
#include "bridges/hw_backend.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void print(const char *what, const br_battery_sample *s)
{
  printf("%s: %u%% present=%d plugged=%d charging=%d full=%d temp=%d (tenths C) voltage=%u mV current=%d mA full=%u mAh design=%u mAh cycles=%u minutes=%d\n",
         what, s->percent, s->present, s->plugged, s->charging, s->full, s->temp_deci_c, s->voltage_mv, s->current_ma, s->full_mah, s->design_mah, s->cycles, s->minutes);
  fflush(stdout);
}

static void changed(void *ctx, const br_battery_sample *s) { (void)ctx; print("change", s); }

int main(int argc, char **argv)
{
  bool watch = argc > 1 && !strcmp(argv[1], "watch");
  int seconds = watch && argc > 2 ? atoi(argv[2]) : 60;
  br_hw_settings hw;
  const char *bad = NULL;
  br_hw_settings_init(&hw);
  if (br_hw_settings_env(&hw, &bad)) { printf("batteryctl: failed: bad %s (samsung or treble; ONEIOS_HW_FALLBACK 0 or 1)\n", bad); return 2; }
  br_hw_configure(&hw);
  int backend = br_hw_choose(&hw, BR_HW_BATTERY, BR_HW_UNSET, NULL, 0);
  // ONEIOS_BATTERY_DIR: the options (the bridge then reads no other variable); otherwise the phone's paths and variables.
  br_battery_options o = { .dir = getenv("ONEIOS_BATTERY_DIR"), .supplies = getenv("ONEIOS_BATTERY_SUPPLIES") };
  br_battery *b = br_battery_open(o.dir ? &o : NULL);
  if (!b) { printf("batteryctl: failed: no battery (%s backend)\n", br_hw_name(backend)); return 1; }
  printf("backend: %s\n", br_hw_name(br_battery_backend(b)));
  br_battery_sample s;
  (void)br_battery_query(b, &s);
  print("sample", &s);
  if (watch) {
    br_battery_listen(b, changed, NULL);
    sleep(seconds > 0 ? (unsigned)seconds : 60u);
    br_battery_listen(b, NULL, NULL);
  }
  br_battery_close(b);
  puts("batteryctl: ok");
  return 0;
}
