// SPDX-License-Identifier: MIT
// A scripted event source for the virtio-input device: touches and key presses from a text file, for a host
// with no touchscreen (the PC under qemu-aarch64) and for repeatable checks of the guest's input path.
// Setting: input-script=FILE (--input-script FILE). It runs beside the evdev reader, or alone with --no-input.
//
//   # comment
//   abs CODE MIN MAX [RES]   an axis of the device's configuration (what input.c takes from EVIOCGABS on the
//                            phone). Applied when the file is read, before the guest starts: the guest's driver
//                            takes the device as a touch screen only if it has ABS_MT_POSITION_X/Y (0x35, 0x36).
//   ready                    wait until the guest's driver runs the device (DRIVER_OK)
//   wait MS
//   ev TYPE CODE VALUE       one evdev event, as written
//   syn                      ev 0 0 0: the end of a frame; events reach the guest at a frame's end
//   tap X Y [MS]             a finger in slot 0 at X, Y (the axes' own units), lifted MS later (default 80)
//   swipe X0 Y0 X1 Y1 [MS]   a finger in slot 0 moved from one point to the other in MS (default 300), 16 ms a step
//   key CODE [MS]            a key pressed and released MS later (default 100): 116 power, 115 / 114 volume up / down
//   repeat [N]               the steps after `ready` (or all, without one) again: forever, or N more times
//
// The pen (pen.c; a file with any pen- line has one, given to the guest as its own device when xnuhost runs with pen=auto):
//   pen-abs CODE MIN MAX [RES]   an axis of the scripted pen's own device, as the phone's pen states it (ABS_X 0, ABS_Y 1,
//                                ABS_PRESSURE 0x18, ABS_DISTANCE 0x19, ABS_TILT_X 0x1a, ABS_TILT_Y 0x1b; RES for tilt:
//                                units per radian). Its events go through the pen's filter as the phone's do (pen=auto's
//                                pen-rotate and pen-mirror do not apply: the script is in the panel's frame)
//   pen-ev TYPE CODE VALUE       one event of the pen's device;  pen-syn: the end of its frame
//   pen-hover X Y [MS] [DIST]    the pen in range above X, Y (BTN_TOOL_PEN; ABS_DISTANCE DIST), then MS ms (default 100)
//   pen-stroke X0 Y0 X1 Y1 MS P0 P1 [TX TY]   the tip down at X0, Y0 with pressure P0, moved to X1, Y1 in MS ms (16 ms a
//                                step) while the pressure goes to P1, at tilt TX, TY (device units); then the tip up, in range
//   pen-button 0|1               the side button (BTN_STYLUS) up or down
//   pen-out                      the pen leaves (BTN_TOOL_PEN 0)
//
// Numbers are decimal or 0x hexadecimal. A bad line stops xnuhost's start with its number. The events go through
// input.c's filter (xh_input_feed) as a touch screen's do: only the codes the panel's would pass, each frame's end
// preceded by EV_MSC / MSC_TIMESTAMP with the time it is sent, and one `input: touch:` line per gesture.
#include "xnuhost.h"
#include <ctype.h>
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

enum { EV_SYN_ = 0, EV_KEY_ = 1, EV_ABS_ = 3, BTN_TOUCH_ = 0x14a, BTN_TOOL_PEN_ = 0x140, BTN_STYLUS_ = 0x14b,
       ABS_X_ = 0, ABS_Y_ = 1, ABS_PRESSURE_ = 0x18, ABS_DISTANCE_ = 0x19, ABS_TILT_X_ = 0x1a, ABS_TILT_Y_ = 0x1b,
       ABS_MT_SLOT_ = 0x2f, ABS_MT_POSITION_X_ = 0x35, ABS_MT_POSITION_Y_ = 0x36, ABS_MT_TRACKING_ID_ = 0x39 };

static xh_input_step *steps;
static unsigned nsteps;
static xh_virtio *target;
static xh_input_source source;                 // input.c's filter: the script is one touch screen with the side keys
static xh_pen_source pen_script;               // pen.c's filter, for the pen- lines, when the guest has a pen device
static bool pen_on;
static char script_path[512];
static pthread_t thread;
static volatile bool running;

static bool push(xh_input_step **list, unsigned *n, unsigned *cap, xh_input_step step)
{
  if (*n == *cap) {
    unsigned grown = *cap ? *cap * 2 : 64;
    xh_input_step *more = realloc(*list, grown * sizeof **list);
    if (!more) return false;
    *list = more;
    *cap = grown;
  }
  (*list)[(*n)++] = step;
  return true;
}

static bool event(xh_input_step **list, unsigned *n, unsigned *cap, unsigned type, unsigned code, int32_t value)
{
  return push(list, n, cap, (xh_input_step){ .kind = XH_STEP_EVENT, .type = (uint16_t)type, .code = (uint16_t)code, .value = value });
}

static bool pen_event(xh_input_step **list, unsigned *n, unsigned *cap, unsigned type, unsigned code, int32_t value)
{
  return push(list, n, cap, (xh_input_step){ .kind = XH_STEP_EVENT, .pen = true, .type = (uint16_t)type, .code = (uint16_t)code, .value = value });
}

static bool pause_ms(xh_input_step **list, unsigned *n, unsigned *cap, long ms)
{
  return push(list, n, cap, (xh_input_step){ .kind = XH_STEP_WAIT, .value = (int32_t)ms });
}

// Up to `max` numbers after the directive; the count, or -1 for something that is not a number.
static int numbers(char *p, long *out, int max)
{
  int n = 0;
  char *save = NULL;
  for (char *word = strtok_r(p, " \t", &save); word; word = strtok_r(NULL, " \t", &save)) {
    char *end;
    errno = 0;
    long v = strtol(word, &end, 0);
    if (*end || errno || n == max) return -1;
    out[n++] = v;
  }
  return n;
}

int xh_input_script_parse(const char *text, xh_input_step **out, unsigned *count, unsigned *bad_line)
{
  xh_input_step *list = NULL;
  unsigned n = 0, cap = 0, line = 0, tracking = 0;
  char *copy = strdup(text);
  bool ok = copy != NULL;
  for (char *row = copy, *next; ok && row && *row; row = next) {
    next = strchr(row, '\n');
    if (next) *next++ = 0;
    line++;
    char *hash = strchr(row, '#');
    if (hash) *hash = 0;
    while (isspace((unsigned char)*row)) row++;
    size_t length = strlen(row);
    while (length && isspace((unsigned char)row[length - 1])) row[--length] = 0;
    if (!*row) continue;
    char *args = row + strcspn(row, " \t");
    if (*args) *args++ = 0;
    long v[9];
    int c = numbers(args, v, 9);
    if ((!strcmp(row, "abs") || !strcmp(row, "pen-abs")) && (c == 3 || c == 4) && v[0] >= 0 && v[0] < 0x40 && v[1] < v[2]) {
      ok = push(&list, &n, &cap, (xh_input_step){ .kind = XH_STEP_ABS, .pen = row[0] == 'p', .code = (uint16_t)v[0], .min = (int32_t)v[1],
                                                  .max = (int32_t)v[2], .value = c == 4 ? (int32_t)v[3] : 0 });
    } else if (!strcmp(row, "pen-ev") && c == 3 && v[0] >= 0 && v[0] <= 0xffff && v[1] >= 0 && v[1] <= 0xffff) {
      ok = pen_event(&list, &n, &cap, (unsigned)v[0], (unsigned)v[1], (int32_t)v[2]);
    } else if (!strcmp(row, "pen-syn") && c == 0) {
      ok = pen_event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if (!strcmp(row, "pen-hover") && c >= 2 && c <= 4 && (c < 3 || v[2] >= 0)) {
      ok = pen_event(&list, &n, &cap, EV_ABS_, ABS_X_, (int32_t)v[0]) && pen_event(&list, &n, &cap, EV_ABS_, ABS_Y_, (int32_t)v[1]) &&
           (c < 4 || pen_event(&list, &n, &cap, EV_ABS_, ABS_DISTANCE_, (int32_t)v[3])) &&
           pen_event(&list, &n, &cap, EV_KEY_, BTN_TOOL_PEN_, 1) && pen_event(&list, &n, &cap, EV_SYN_, 0, 0) &&
           pause_ms(&list, &n, &cap, c >= 3 ? v[2] : 100);
    } else if (!strcmp(row, "pen-stroke") && (c == 7 || c == 9) && v[4] >= 0) {
      long x0 = v[0], y0 = v[1], x1 = v[2], y1 = v[3], ms = v[4], p0 = v[5], p1 = v[6], moves = ms / 16 > 0 ? ms / 16 : 1;
      ok = (c < 9 || (pen_event(&list, &n, &cap, EV_ABS_, ABS_TILT_X_, (int32_t)v[7]) && pen_event(&list, &n, &cap, EV_ABS_, ABS_TILT_Y_, (int32_t)v[8]))) &&
           pen_event(&list, &n, &cap, EV_KEY_, BTN_TOOL_PEN_, 1) && pen_event(&list, &n, &cap, EV_ABS_, ABS_X_, (int32_t)x0) &&
           pen_event(&list, &n, &cap, EV_ABS_, ABS_Y_, (int32_t)y0) && pen_event(&list, &n, &cap, EV_ABS_, ABS_DISTANCE_, 0) &&
           pen_event(&list, &n, &cap, EV_ABS_, ABS_PRESSURE_, (int32_t)p0) && pen_event(&list, &n, &cap, EV_KEY_, BTN_TOUCH_, 1) &&
           pen_event(&list, &n, &cap, EV_SYN_, 0, 0);
      for (long i = 1; ok && i <= moves; i++)
        ok = pause_ms(&list, &n, &cap, ms / moves) && pen_event(&list, &n, &cap, EV_ABS_, ABS_X_, (int32_t)(x0 + (x1 - x0) * i / moves)) &&
             pen_event(&list, &n, &cap, EV_ABS_, ABS_Y_, (int32_t)(y0 + (y1 - y0) * i / moves)) &&
             pen_event(&list, &n, &cap, EV_ABS_, ABS_PRESSURE_, (int32_t)(p0 + (p1 - p0) * i / moves)) && pen_event(&list, &n, &cap, EV_SYN_, 0, 0);
      ok = ok && pen_event(&list, &n, &cap, EV_ABS_, ABS_PRESSURE_, 0) && pen_event(&list, &n, &cap, EV_KEY_, BTN_TOUCH_, 0) &&
           pen_event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if (!strcmp(row, "pen-button") && c == 1 && (v[0] == 0 || v[0] == 1)) {
      ok = pen_event(&list, &n, &cap, EV_KEY_, BTN_STYLUS_, (int32_t)v[0]) && pen_event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if (!strcmp(row, "pen-out") && c == 0) {
      ok = pen_event(&list, &n, &cap, EV_KEY_, BTN_TOUCH_, 0) && pen_event(&list, &n, &cap, EV_KEY_, BTN_TOOL_PEN_, 0) &&
           pen_event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if (!strcmp(row, "ready") && c == 0) {
      ok = push(&list, &n, &cap, (xh_input_step){ .kind = XH_STEP_READY });
    } else if (!strcmp(row, "wait") && c == 1 && v[0] >= 0) {
      ok = pause_ms(&list, &n, &cap, v[0]);
    } else if (!strcmp(row, "ev") && c == 3 && v[0] >= 0 && v[0] <= 0xffff && v[1] >= 0 && v[1] <= 0xffff) {
      ok = event(&list, &n, &cap, (unsigned)v[0], (unsigned)v[1], (int32_t)v[2]);
    } else if (!strcmp(row, "syn") && c == 0) {
      ok = event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if ((!strcmp(row, "tap") && (c == 2 || c == 3)) || (!strcmp(row, "swipe") && (c == 4 || c == 5))) {
      bool swipe = row[0] == 's';
      long x0 = v[0], y0 = v[1], x1 = swipe ? v[2] : x0, y1 = swipe ? v[3] : y0;
      long ms = swipe ? (c == 5 ? v[4] : 300) : (c == 3 ? v[2] : 80);
      long moves = swipe ? (ms / 16 > 0 ? ms / 16 : 1) : 0;
      ok = ms >= 0 && event(&list, &n, &cap, EV_ABS_, ABS_MT_SLOT_, 0) && event(&list, &n, &cap, EV_ABS_, ABS_MT_TRACKING_ID_, (int32_t)++tracking) &&
           event(&list, &n, &cap, EV_ABS_, ABS_MT_POSITION_X_, (int32_t)x0) && event(&list, &n, &cap, EV_ABS_, ABS_MT_POSITION_Y_, (int32_t)y0) &&
           event(&list, &n, &cap, EV_KEY_, BTN_TOUCH_, 1) && event(&list, &n, &cap, EV_SYN_, 0, 0);
      for (long i = 1; ok && i <= moves; i++)
        ok = pause_ms(&list, &n, &cap, ms / moves) && event(&list, &n, &cap, EV_ABS_, ABS_MT_POSITION_X_, (int32_t)(x0 + (x1 - x0) * i / moves)) &&
             event(&list, &n, &cap, EV_ABS_, ABS_MT_POSITION_Y_, (int32_t)(y0 + (y1 - y0) * i / moves)) && event(&list, &n, &cap, EV_SYN_, 0, 0);
      ok = ok && (swipe || pause_ms(&list, &n, &cap, ms)) && event(&list, &n, &cap, EV_ABS_, ABS_MT_TRACKING_ID_, -1) &&
           event(&list, &n, &cap, EV_KEY_, BTN_TOUCH_, 0) && event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if (!strcmp(row, "key") && (c == 1 || c == 2) && v[0] > 0 && v[0] <= 0xffff && (c == 1 || v[1] >= 0)) {
      ok = event(&list, &n, &cap, EV_KEY_, (unsigned)v[0], 1) && event(&list, &n, &cap, EV_SYN_, 0, 0) && pause_ms(&list, &n, &cap, c == 2 ? v[1] : 100) &&
           event(&list, &n, &cap, EV_KEY_, (unsigned)v[0], 0) && event(&list, &n, &cap, EV_SYN_, 0, 0);
    } else if (!strcmp(row, "repeat") && (c == 0 || (c == 1 && v[0] > 0))) {
      ok = push(&list, &n, &cap, (xh_input_step){ .kind = XH_STEP_REPEAT, .value = c ? (int32_t)v[0] : -1 });
    } else ok = false;
  }
  free(copy);
  if (!ok) { free(list); if (bad_line) *bad_line = line; return -1; }
  *out = list;
  *count = n;
  return 0;
}

static void sleep_ms(long ms)
{
  for (; ms > 0 && running; ms -= 50) {
    long part = ms < 50 ? ms : 50;
    nanosleep(&(struct timespec){ .tv_sec = 0, .tv_nsec = part * 1000000L }, NULL);
  }
}

static void *script_main(void *arg)
{
  (void)arg;
  unsigned start = 0;
  int32_t left = 0;                            // repeats still to run; 0 before the first `repeat` is met
  bool counting = false;
  for (unsigned i = 0; i < nsteps && running; i++) {
    const xh_input_step *s = &steps[i];
    switch (s->kind) {
    case XH_STEP_ABS: break;                   // applied when the file was read
    case XH_STEP_READY:
      while (running && !xh_virtio_ready(target)) sleep_ms(200);
      if (running) LOGI("input script: the guest's driver runs the input device");
      start = i + 1;
      break;
    case XH_STEP_WAIT: sleep_ms(s->value); break;
    case XH_STEP_EVENT: {
      // Through input.c's filter, as the panel's events go: a frame's end gets its time (EV_MSC / MSC_TIMESTAMP, the
      // time it is sent), and the gesture is counted and logged (input: touch: ...) with its touch-to-frame time.
      struct timespec now;
      clock_gettime(CLOCK_MONOTONIC, &now);
      const int64_t now_ns = (int64_t)now.tv_sec * 1000000000 + now.tv_nsec;
      const xh_input_ev ev = { now_ns, s->type, s->code, s->value };
      if (s->pen) { if (pen_on) xh_pen_feed(&pen_script, &ev, 1, now_ns); }   // the pen's filter (pen.c), as the phone's pen
      else xh_input_feed(&source, &ev, 1, now_ns, xh_counter(), g_info.timebase_hz);
      break;
    }
    case XH_STEP_REPEAT:
      if (s->value < 0) { i = start - 1; break; }
      if (!counting) { counting = true; left = s->value; }
      if (left > 0) { left--; i = start - 1; } else counting = false;
      break;
    }
  }
  if (running) LOGI("input script: done");
  return NULL;
}

int xh_input_script_load(const char *path, bool *has_pen)
{
  *has_pen = false;
  if (!path || !*path) return 0;
  FILE *f = fopen(path, "r");
  if (!f) { LOGE("input script %s: %s", path, strerror(errno)); return -1; }
  static char text[65536];
  size_t n = fread(text, 1, sizeof text - 1, f);
  bool whole = feof(f);
  fclose(f);
  text[n] = 0;
  unsigned bad = 0;
  if (!whole || xh_input_script_parse(text, &steps, &nsteps, &bad)) {
    LOGE("input script %s: %s", path, whole ? "bad line" : "longer than 64 KiB");
    if (whole) LOGE("input script %s: line %u", path, bad);
    return -1;
  }
  snprintf(script_path, sizeof script_path, "%s", path);
  for (unsigned i = 0; i < nsteps; i++) *has_pen |= steps[i].pen;
  return 0;
}

int xh_input_script_start(xh_virtio *dev, xh_virtio *pen)
{
  if (!nsteps) return 0;
  unsigned events = 0, pen_events = 0;
  xh_pen_device scripted = { .fd = -1 };
  snprintf(scripted.name, sizeof scripted.name, "the script's pen");
  scripted.stylus = true;
  for (unsigned i = 0; i < nsteps; i++) {
    const xh_input_step *s = &steps[i];
    if (s->kind == XH_STEP_ABS && s->pen) {
      for (unsigned a = 0; a < XH_PEN_AXES; a++)
        if (s->code == (const uint16_t[]){ ABS_X_, ABS_Y_, ABS_PRESSURE_, ABS_DISTANCE_, ABS_TILT_X_, ABS_TILT_Y_ }[a])
          scripted.axis[a] = (xh_pen_axis){ s->min, s->max, s->value, true };
    } else if (s->kind == XH_STEP_ABS) xh_virtio_input_set_abs(dev, s->code, s->min, s->max, s->value);
    else if (s->kind == XH_STEP_EVENT) { events++; pen_events += s->pen; }
  }
  target = dev;
  xh_input_source_init(&source, dev, "script", true);
  if (pen) {
    xh_pen_source_init(&pen_script, pen, &scripted, 0, false);
    pen_on = true;
    char axes[200];
    xh_pen_describe(&scripted, axes, sizeof axes);
    LOGI("input script: a pen (%s) on the guest's pen device", axes);
  } else if (pen_events) LOGI("input script: %u pen events, no pen device (pen=off): left out", pen_events);
  running = true;
  if (pthread_create(&thread, NULL, script_main, NULL)) { running = false; return -1; }
  LOGI("input script: %s, %u steps (%u events, %u of them the pen's)", script_path, nsteps, events, pen_events);
  return 0;
}

void xh_input_script_stop(void)
{
  if (!running) return;
  running = false;
  pthread_join(thread, NULL);
}
