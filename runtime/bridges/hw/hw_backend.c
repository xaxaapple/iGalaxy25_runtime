// SPDX-License-Identifier: MIT
// The hardware backend layer's settings and choice (bridges/hw_backend.h). Pure: no log, no files; xnuhost's config.c
// links it for the settings. hw_log.c has the log lines.
#define _GNU_SOURCE
#include "bridges/hw_backend.h"
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *const bridge_names[BR_HW_NBRIDGE] = {
  "backlight", "battery", "thermal", "haptics", "touch", "keys", "audio", "camera", "wifi", "bluetooth", "display", "encoder",
};

void br_hw_settings_init(br_hw_settings *s)
{
  memset(s, 0, sizeof *s);
  s->fallback = -1;
}

const char *br_hw_name(int backend) { return backend == BR_HW_SAMSUNG ? "samsung" : backend == BR_HW_TREBLE ? "treble" : "unset"; }

const char *br_hw_bridge_name(unsigned bridge) { return bridge < BR_HW_NBRIDGE ? bridge_names[bridge] : "?"; }

int br_hw_parse_name(const char *text)
{
  if (!text) return -1;
  if (!strcmp(text, "samsung")) return BR_HW_SAMSUNG;
  if (!strcmp(text, "treble")) return BR_HW_TREBLE;
  return -1;
}

int br_hw_setting(br_hw_settings *s, const char *key, const char *value)
{
  if (strncmp(key, "hw-backend", 10)) return 1;
  const char *rest = key + 10;
  if (!strcmp(rest, "-fallback")) {
    if (!strcmp(value, "1") || !strcmp(value, "yes") || !strcmp(value, "true")) s->fallback = 1;
    else if (!strcmp(value, "0") || !strcmp(value, "no") || !strcmp(value, "false")) s->fallback = 0;
    else return -1;
    return 0;
  }
  int backend = br_hw_parse_name(value);
  if (!*rest) { if (backend < 0) return -1; s->all = (int8_t)backend; return 0; }
  if (*rest != '-') return 1;
  for (unsigned i = 0; i < BR_HW_NBRIDGE; i++)
    if (!strcmp(rest + 1, bridge_names[i])) { if (backend < 0) return -1; s->one[i] = (int8_t)backend; return 0; }
  return -1;                                      // hw-backend-<a bridge there is none of>: a typo, not someone else's key
}

void br_hw_settings_merge(br_hw_settings *s, const br_hw_settings *under)
{
  if (!s->all) s->all = under->all;
  for (unsigned i = 0; i < BR_HW_NBRIDGE; i++)
    if (!s->one[i]) s->one[i] = under->one[i];
  if (s->fallback < 0) s->fallback = under->fallback;
}

int br_hw_settings_env(br_hw_settings *s, const char **bad)
{
  static char names[BR_HW_NBRIDGE][40];
  const char *v = getenv("ONEIOS_HW_BACKEND");
  if (v && *v && br_hw_setting(s, "hw-backend", v)) { if (bad) *bad = "ONEIOS_HW_BACKEND"; return -1; }
  for (unsigned i = 0; i < BR_HW_NBRIDGE; i++) {
    char *n = names[i];
    snprintf(n, sizeof names[i], "ONEIOS_HW_BACKEND_%s", bridge_names[i]);
    for (char *c = n; *c; c++) *c = (char)toupper((unsigned char)*c);
    char key[40];
    snprintf(key, sizeof key, "hw-backend-%s", bridge_names[i]);
    if ((v = getenv(n)) && *v && br_hw_setting(s, key, v)) { if (bad) *bad = n; return -1; }
  }
  v = getenv("ONEIOS_HW_FALLBACK");
  if (v && *v && br_hw_setting(s, "hw-backend-fallback", v)) { if (bad) *bad = "ONEIOS_HW_FALLBACK"; return -1; }
  return 0;
}

int br_hw_choose(const br_hw_settings *s, unsigned bridge, int forced, char *why, size_t size)
{
  int backend = BR_HW_SAMSUNG;
  char text[64] = "the default";
  if (forced == BR_HW_SAMSUNG || forced == BR_HW_TREBLE) { backend = forced; snprintf(text, sizeof text, "the caller's choice"); }
  else if (s && bridge < BR_HW_NBRIDGE && s->one[bridge]) {
    backend = s->one[bridge];
    snprintf(text, sizeof text, "hw-backend-%s=%s", bridge_names[bridge], br_hw_name(backend));
  } else if (s && s->all) { backend = s->all; snprintf(text, sizeof text, "hw-backend=%s", br_hw_name(backend)); }
  if (why && size) snprintf(why, size, "%s", text);
  return backend;
}

bool br_hw_fallback_allowed(const br_hw_settings *s) { return s && s->fallback == 1; }

static br_hw_settings current = { .fallback = -1 };

void br_hw_configure(const br_hw_settings *s) { if (s) current = *s; else br_hw_settings_init(&current); }

const br_hw_settings *br_hw_current(void) { return &current; }
