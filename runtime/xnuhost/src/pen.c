// SPDX-License-Identifier: MIT
// The S Pen (spen, 2026-11-15): the phone's pen digitizer as the guest's pen device, a virtio-input device of its own
// (/arm-io/input-1) beside the touch screen's, which the guest publishes as iOS's pencil digitizer (Digitizer / Stylus,
// kernel/patches/oneios-hosted/hid, staged/hid_pen.rs) when it is booted with oneios-pen=1.
//
// The host side has three parts, each replaceable on its own:
//   the lookup     xh_pen_lookup: finds the pen's input device by its capabilities and opens it read only. The default,
//                  xh_pen_lookup_evdev, scans /dev/input/event0..63 by ioctl as input.c does for the touch screen (the
//                  Samsung kernel's device is "sec_e-pen": docs/agents.md, 2026-11-10). Another hardware backend (a
//                  generic/Treble one) sets xh_pen_lookup to its own function; nothing else here names a Samsung path.
//   the filter     xh_pen_feed / xh_pen_sample_of: the device's evdev events, frame by frame, into one sample in the
//                  guest's units: in range (BTN_TOOL_PEN or BTN_TOOL_RUBBER), the tip down (BTN_TOUCH), the side button
//                  (BTN_STYLUS, BTN_STYLUS2), x and y 0..32767 turned to the panel's frame (pen-rotate, pen-mirror),
//                  pressure 0..4095 (only while the tip is down), the height above the panel 0..255 (ABS_DISTANCE; never 0
//                  while hovering: UIKit makes no hover of a stylus event whose Z is 0), tilt x and y in degrees.
//   the guest      xh_pen_guest_device: what the guest's pen device states (those fixed ranges), so the guest's transport
//                  (DroidVMVirtioInput.c, KIND_PEN) needs no knowledge of the phone's digitizer.
// A frame whose sample is the one last sent is not sent again (a pen held still in the air reports nothing new to iOS),
// and only the fields that changed go out before the frame's SYN_REPORT. After SYN_DROPPED (the reader was too slow) the
// device's state is read again (EVIOCGKEY, EVIOCGABS), so that a lift or a leave lost in the drop does not leave the pen
// down or in range in iOS.
//
// The device is never grabbed (EVIOCGRAB): in a session nothing else reads it (zygote and system_server are gone, as for
// the touch screen: oneos/interface.md), and One UI gets it back as it was. A device that has the side keys (KEY_POWER,
// KEY_VOLUMEUP, KEY_VOLUMEDOWN) or multitouch positions is never taken as the pen.
//
// One line per time in range (the pen comes near, then leaves): `pen: in range N ms: F frames (H hovering, T touching),
// S strokes, pressure up to P/4095, tilt x a..b y c..d degrees, the side button pressed B times; U unchanged frames not sent`.
#define _GNU_SOURCE
#include "xnuhost.h"
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

enum { EV_SYN_ = 0, EV_KEY_ = 1, EV_ABS_ = 3, SYN_REPORT_ = 0, SYN_DROPPED_ = 3,
       ABS_X_ = 0, ABS_Y_ = 1, ABS_PRESSURE_ = 0x18, ABS_DISTANCE_ = 0x19, ABS_TILT_X_ = 0x1a, ABS_TILT_Y_ = 0x1b,
       ABS_MT_POSITION_X_ = 0x35, KEY_VOLUMEDOWN_ = 114, KEY_VOLUMEUP_ = 115, KEY_POWER_ = 116,
       BTN_TOOL_PEN_ = 0x140, BTN_TOOL_RUBBER_ = 0x141, BTN_TOUCH_ = 0x14a, BTN_STYLUS_ = 0x14b, BTN_STYLUS2_ = 0x14c };
static const uint16_t axis_code[XH_PEN_AXES] = { ABS_X_, ABS_Y_, ABS_PRESSURE_, ABS_DISTANCE_, ABS_TILT_X_, ABS_TILT_Y_ };

static bool byte_bit(const uint8_t *bits, size_t size, unsigned n) { return n / 8 < size && (bits[n / 8] >> (n % 8) & 1); }

// ---- the lookup's test (portable) ------------------------------------------------------------------
int xh_pen_rank(const char *name, const uint8_t *keys, size_t nkeys, const uint8_t *abs, size_t nabs, const char **why)
{
  const char *no = NULL;
  if (byte_bit(keys, nkeys, KEY_POWER_) || byte_bit(keys, nkeys, KEY_VOLUMEUP_) || byte_bit(keys, nkeys, KEY_VOLUMEDOWN_))
    no = "it has side keys (never taken)";
  else if (byte_bit(abs, nabs, ABS_MT_POSITION_X_)) no = "a multitouch device";
  else if (!byte_bit(keys, nkeys, BTN_TOOL_PEN_)) no = "no BTN_TOOL_PEN";
  else if (!byte_bit(abs, nabs, ABS_X_) || !byte_bit(abs, nabs, ABS_Y_)) no = "no ABS_X/ABS_Y";
  else if (!byte_bit(abs, nabs, ABS_PRESSURE_)) no = "no ABS_PRESSURE";
  if (why) *why = no;
  if (no) return 0;
  // A pen: more for what iOS can use (tilt, the side button), and for a name that says it is one (Samsung's "sec_e-pen").
  int rank = 1 + byte_bit(abs, nabs, ABS_TILT_X_) + byte_bit(keys, nkeys, BTN_STYLUS_) + byte_bit(abs, nabs, ABS_DISTANCE_);
  if (name && (strstr(name, "pen") || strstr(name, "Pen") || strstr(name, "stylus") || strstr(name, "Stylus"))) rank += 4;
  return rank;
}

// ---- the filter (portable: tests/test_xnuhost.c) ---------------------------------------------------
static int32_t scaled(int32_t v, const xh_pen_axis *a, int32_t top)
{
  if (!a->present || a->max <= a->min) return 0;
  if (v < a->min) v = a->min;
  if (v > a->max) v = a->max;
  return (int32_t)((int64_t)(v - a->min) * top / ((int64_t)a->max - a->min));
}

// A tilt axis in whole degrees: by its resolution (evdev's for tilt: units per radian), else as degrees when its range
// lies within +-90, else its range's end as 90 degrees. Kept within +-89 (the guest's altitude needs some lean left).
static int32_t degrees(int32_t v, const xh_pen_axis *a)
{
  if (!a->present) return 0;
  int64_t d;
  int32_t end = a->max > -a->min ? a->max : -a->min;
  if (a->res > 0) d = (int64_t)v * 18000000 / ((int64_t)a->res * 314159);
  else if (end <= 90) d = v;
  else d = end ? (int64_t)v * 90 / end : 0;
  return (int32_t)(d > 89 ? 89 : d < -89 ? -89 : d);
}

void xh_pen_sample_of(const xh_pen_device *d, unsigned rotate, bool mirror, const xh_pen_state *st, xh_pen_sample *out)
{
  memset(out, 0, sizeof *out);
  // In range: either end near the panel. A tip that touches is in range even if the driver never said so.
  out->touch = st->touch;
  out->in_range = st->tool || st->rubber || st->touch;
  if (!out->in_range) return;
  out->eraser = st->rubber && !st->tool;
  out->barrel = st->stylus;
  int32_t x = scaled(st->value[XH_PEN_X], &d->axis[XH_PEN_X], XH_PEN_XY_MAX), y = scaled(st->value[XH_PEN_Y], &d->axis[XH_PEN_Y], XH_PEN_XY_MAX);
  int32_t tx = degrees(st->value[XH_PEN_TILT_X], &d->axis[XH_PEN_TILT_X]), ty = degrees(st->value[XH_PEN_TILT_Y], &d->axis[XH_PEN_TILT_Y]);
  // The digitizer's frame turned clockwise by `rotate` onto the panel's, then mirrored left to right; the tilt with it.
  int32_t px = x, py = y, ptx = tx, pty = ty;
  switch (rotate) {
  case 90:  px = XH_PEN_XY_MAX - y; py = x; ptx = -ty; pty = tx; break;
  case 180: px = XH_PEN_XY_MAX - x; py = XH_PEN_XY_MAX - y; ptx = -tx; pty = -ty; break;
  case 270: px = y; py = XH_PEN_XY_MAX - x; ptx = ty; pty = -tx; break;
  default: break;
  }
  if (mirror) { px = XH_PEN_XY_MAX - px; ptx = -ptx; }
  out->x = px; out->y = py; out->tilt_x = ptx; out->tilt_y = pty;
  if (st->touch) {
    out->pressure = d->axis[XH_PEN_PRESSURE].present ? scaled(st->value[XH_PEN_PRESSURE], &d->axis[XH_PEN_PRESSURE], XH_PEN_PRESSURE_MAX) : XH_PEN_PRESSURE_MAX / 2;
  } else {
    // The height above the panel while hovering: never 0 (UIKit's hover needs the stylus event's Z above 0); half
    // way for a device that states none.
    int32_t h = d->axis[XH_PEN_DISTANCE].present ? scaled(st->value[XH_PEN_DISTANCE], &d->axis[XH_PEN_DISTANCE], XH_PEN_DISTANCE_MAX) : XH_PEN_DISTANCE_MAX / 2;
    out->distance = h < 1 ? 1 : h;
  }
}

static void emit_virtio(void *context, uint16_t type, uint16_t code, int32_t value) { xh_virtio_input_event(context, type, code, value); }

void xh_pen_source_init(xh_pen_source *s, xh_virtio *dev, const xh_pen_device *d, unsigned rotate, bool mirror)
{
  memset(s, 0, sizeof *s);
  s->emit = emit_virtio;
  s->context = dev;
  s->device = *d;
  s->fd = d->fd;
  s->rotate = rotate;
  s->mirror = mirror;
}

static void log_proximity(xh_pen_source *s, int64_t now_ns)
{
  xh_pen_stats *t = &s->stats;
  long long ms = t->since_ns && now_ns > t->since_ns ? (now_ns - t->since_ns) / 1000000 : 0;
  char tilt[64] = "";
  if (s->device.axis[XH_PEN_TILT_X].present) snprintf(tilt, sizeof tilt, ", tilt x %d..%d y %d..%d degrees", t->tilt_min[0], t->tilt_max[0], t->tilt_min[1], t->tilt_max[1]);
  LOGI("pen: in range %lld ms: %u frame%s (%u hovering, %u touching), %u stroke%s, pressure up to %d/4095%s, the side button pressed %u time%s; %u unchanged frame%s not sent",
       ms, t->frames, t->frames == 1 ? "" : "s", t->hover, t->touching, t->strokes, t->strokes == 1 ? "" : "s", t->pressure_max, tilt,
       t->presses, t->presses == 1 ? "" : "s", t->unchanged, t->unchanged == 1 ? "" : "s");
  memset(t, 0, sizeof *t);
}

// One sample to the guest: the fields that changed, then the frame's end. Nothing when nothing changed.
static void send(xh_pen_source *s, const xh_pen_sample *n, int64_t now_ns)
{
  const xh_pen_sample *o = &s->sent;
  bool all = !s->sent_any, any = false;
  xh_pen_stats *t = &s->stats;
  if (n->in_range && !t->since_ns) {
    t->since_ns = now_ns ? now_ns : 1;
    t->tilt_min[0] = t->tilt_max[0] = n->tilt_x;
    t->tilt_min[1] = t->tilt_max[1] = n->tilt_y;
  }
  if (n->in_range) {
    t->frames++;
    if (n->touch) t->touching++; else t->hover++;
    if (n->pressure > t->pressure_max) t->pressure_max = n->pressure;
    if (n->tilt_x < t->tilt_min[0]) t->tilt_min[0] = n->tilt_x;
    if (n->tilt_x > t->tilt_max[0]) t->tilt_max[0] = n->tilt_x;
    if (n->tilt_y < t->tilt_min[1]) t->tilt_min[1] = n->tilt_y;
    if (n->tilt_y > t->tilt_max[1]) t->tilt_max[1] = n->tilt_y;
    if (n->touch && !o->touch) t->strokes++;
    if (n->barrel && !o->barrel) t->presses++;
  }
#define FIELD(field, type, code) do { if (all || n->field != o->field) { s->emit(s->context, type, code, n->field); any = true; } } while (0)
  // Positions and axes before the keys, so that a tip that comes down does so where its frame says.
  if (n->in_range) {
    FIELD(x, EV_ABS_, ABS_X_);
    FIELD(y, EV_ABS_, ABS_Y_);
    FIELD(tilt_x, EV_ABS_, ABS_TILT_X_);
    FIELD(tilt_y, EV_ABS_, ABS_TILT_Y_);
  }
  FIELD(pressure, EV_ABS_, ABS_PRESSURE_);
  FIELD(distance, EV_ABS_, ABS_DISTANCE_);
  FIELD(barrel, EV_KEY_, BTN_STYLUS_);
  FIELD(touch, EV_KEY_, BTN_TOUCH_);
  bool pen = n->in_range && !n->eraser, rubber = n->in_range && n->eraser, was_pen = o->in_range && !o->eraser, was_rubber = o->in_range && o->eraser;
  if (all || pen != was_pen) { s->emit(s->context, EV_KEY_, BTN_TOOL_PEN_, pen); any = true; }
  if (all || rubber != was_rubber) { s->emit(s->context, EV_KEY_, BTN_TOOL_RUBBER_, rubber); any = true; }
#undef FIELD
  if (!any) {
    if (n->in_range) t->unchanged++;
    return;
  }
  s->emit(s->context, EV_SYN_, SYN_REPORT_, 0);
  s->frames_sent++;
  bool left = o->in_range && !n->in_range;
  s->sent = *n;
  s->sent_any = true;
  if (left) log_proximity(s, now_ns);
}

static void resync(xh_pen_source *s, int64_t now_ns)
{
  s->drops++;
  xh_pen_state st;
  if (!s->read_state || s->read_state(s, &st)) {
    LOGI("pen: events dropped by the kernel (the reader was too slow); the device's state could not be read again");
    return;
  }
  s->state = st;
  xh_pen_sample n;
  xh_pen_sample_of(&s->device, s->rotate, s->mirror, &s->state, &n);
  send(s, &n, now_ns);
  LOGI("pen: events dropped by the kernel (the reader was too slow, %u time%s); the state was read again: %s", s->drops,
       s->drops == 1 ? "" : "s", n.touch ? "touching" : n.in_range ? "in range" : "out of range");
}

void xh_pen_feed(xh_pen_source *s, const xh_input_ev *ev, unsigned n, int64_t now_ns)
{
  for (unsigned e = 0; e < n; e++) {
    const xh_input_ev *v = &ev[e];
    if (!s->events++) LOGI("pen: the first event from %s", *s->device.name ? s->device.name : "the pen's device");
    if (s->dropping) {
      if (v->type == EV_SYN_ && v->code == SYN_REPORT_) { s->dropping = false; resync(s, now_ns); }
      continue;
    }
    if (v->type == EV_SYN_) {
      if (v->code == SYN_DROPPED_) { s->dropping = true; continue; }
      if (v->code != SYN_REPORT_) continue;
      xh_pen_sample sample;
      xh_pen_sample_of(&s->device, s->rotate, s->mirror, &s->state, &sample);
      send(s, &sample, now_ns);
    } else if (v->type == EV_KEY_) {
      bool down = v->value != 0;
      switch (v->code) {
      case BTN_TOOL_PEN_: s->state.tool = down; break;
      case BTN_TOOL_RUBBER_: s->state.rubber = down; break;
      case BTN_TOUCH_: s->state.touch = down; break;
      case BTN_STYLUS_: s->state.stylus_bits = down ? s->state.stylus_bits | 1 : s->state.stylus_bits & ~1u; break;
      case BTN_STYLUS2_: s->state.stylus_bits = down ? s->state.stylus_bits | 2 : s->state.stylus_bits & ~2u; break;
      default: break;
      }
      s->state.stylus = s->state.stylus_bits != 0;
    } else if (v->type == EV_ABS_) {
      for (unsigned a = 0; a < XH_PEN_AXES; a++)
        if (v->code == axis_code[a]) s->state.value[a] = v->value;
    }
  }
}

unsigned xh_pen_rotation(const char *setting, const xh_pen_device *d, unsigned width, unsigned height, const char **why)
{
  if (setting && *setting && strcmp(setting, "auto")) {
    if (why) *why = "pen-rotate";
    return (unsigned)atoi(setting);
  }
  // auto: a digitizer whose long side is its x axis under a portrait panel (or the other way round) is turned a quarter.
  // Which quarter is not known for the S24 Ultra's [U]: 90 is a guess; pen-rotate=270 (and pen-mirror) otherwise.
  const xh_pen_axis *x = &d->axis[XH_PEN_X], *y = &d->axis[XH_PEN_Y];
  int64_t w = (int64_t)x->max - x->min, h = (int64_t)y->max - y->min;
  if (x->res > 0 && y->res > 0) { w = w * 1000 / x->res; h = h * 1000 / y->res; }
  bool pen_portrait = h >= w, panel_portrait = height >= width;
  if (why) *why = pen_portrait == panel_portrait ? "auto: the digitizer and the panel are both portrait or both landscape" : "auto: the digitizer's long side is across the panel's (a guess: pen-rotate=270 if strokes come out turned)";
  return pen_portrait == panel_portrait ? 0 : 90;
}

void xh_pen_guest_device(xh_virtio *dev)
{
  static const uint16_t keys[] = { BTN_TOOL_PEN_, BTN_TOOL_RUBBER_, BTN_TOUCH_, BTN_STYLUS_ };
  xh_virtio_input_set_identity(dev, "OneiOS pen", "xnuhost-pen", keys, sizeof keys / sizeof *keys);
  xh_virtio_input_set_abs(dev, ABS_X_, 0, XH_PEN_XY_MAX, 0);
  xh_virtio_input_set_abs(dev, ABS_Y_, 0, XH_PEN_XY_MAX, 0);
  xh_virtio_input_set_abs(dev, ABS_PRESSURE_, 0, XH_PEN_PRESSURE_MAX, 0);
  xh_virtio_input_set_abs(dev, ABS_DISTANCE_, 0, XH_PEN_DISTANCE_MAX, 0);
  // Degrees: no resolution, the guest's transport takes the values as degrees then.
  xh_virtio_input_set_abs(dev, ABS_TILT_X_, -90, 90, 0);
  xh_virtio_input_set_abs(dev, ABS_TILT_Y_, -90, 90, 0);
}

void xh_pen_describe(const xh_pen_device *d, char *text, size_t size)
{
  static const char *names[XH_PEN_AXES] = { "x", "y", "pressure", "distance", "tilt x", "tilt y" };
  size_t used = 0;
  text[0] = 0;
  for (unsigned a = 0; a < XH_PEN_AXES; a++) {
    const xh_pen_axis *x = &d->axis[a];
    char res[24] = "";
    if (x->present && x->res) snprintf(res, sizeof res, " (res %d)", x->res);
    int n = x->present ? snprintf(text + used, size - used, "%s%s %d..%d%s", used ? ", " : "", names[a], x->min, x->max, res)
                       : snprintf(text + used, size - used, "%sno %s", used ? ", " : "", names[a]);
    if (n < 0 || (size_t)n >= size - used) return;
    used += (size_t)n;
  }
  snprintf(text + used, size - used, "; keys: tool pen%s, touch%s", d->rubber ? ", rubber" : "", d->stylus ? ", stylus (the side button)" : ", no side button");
}

#if defined(__linux__)
#include <linux/input.h>
#include <sys/ioctl.h>

// The device's state now, by ioctl (after SYN_DROPPED).
static int read_state(xh_pen_source *s, xh_pen_state *st)
{
  memset(st, 0, sizeof *st);
  uint8_t key[KEY_MAX / 8 + 1] = { 0 };
  if (s->fd < 0 || ioctl(s->fd, EVIOCGKEY(sizeof key), key) < 0) return -1;
  st->tool = byte_bit(key, sizeof key, BTN_TOOL_PEN);
  st->rubber = byte_bit(key, sizeof key, BTN_TOOL_RUBBER);
  st->touch = byte_bit(key, sizeof key, BTN_TOUCH);
  st->stylus_bits = (byte_bit(key, sizeof key, BTN_STYLUS) ? 1u : 0) | (byte_bit(key, sizeof key, BTN_STYLUS2) ? 2u : 0);
  st->stylus = st->stylus_bits != 0;
  for (unsigned a = 0; a < XH_PEN_AXES; a++) {
    struct input_absinfo info;
    if (s->device.axis[a].present && ioctl(s->fd, EVIOCGABS(axis_code[a]), &info) == 0) st->value[a] = info.value;
  }
  return 0;
}

int xh_pen_lookup_evdev(xh_pen_device *out, char *why, size_t size)
{
  int best_rank = 0, best_fd = -1;
  char best_path[32] = "", best_name[64] = "", skipped[160] = "";
  for (unsigned i = 0; i < 64; i++) {
    char path[32], name[64] = "";
    snprintf(path, sizeof path, "/dev/input/event%u", i);
    int fd = open(path, O_RDONLY | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0) continue;
    uint8_t key[KEY_MAX / 8 + 1] = { 0 }, abs[ABS_MAX / 8 + 1] = { 0 };
    ioctl(fd, EVIOCGNAME(sizeof name - 1), name);
    ioctl(fd, EVIOCGBIT(EV_KEY, sizeof key), key);
    ioctl(fd, EVIOCGBIT(EV_ABS, sizeof abs), abs);
    const char *no = NULL;
    int rank = xh_pen_rank(name, key, sizeof key, abs, sizeof abs, &no);
    // Say why a device that looks like a pen by its bits or its name was not taken.
    if (!rank && (byte_bit(key, sizeof key, BTN_TOOL_PEN) || strstr(name, "pen")) && !*skipped)
      snprintf(skipped, sizeof skipped, "; %s \"%s\" not taken: %s", path, name, no);
    if (rank > best_rank) {
      if (best_fd >= 0) close(best_fd);
      best_fd = fd;
      best_rank = rank;
      snprintf(best_path, sizeof best_path, "%s", path);
      snprintf(best_name, sizeof best_name, "%s", name);
    } else close(fd);
  }
  if (best_fd < 0) {
    snprintf(why, size, "no input device has a pen's bits (BTN_TOOL_PEN, ABS_X/Y, ABS_PRESSURE, no side keys)%s", skipped);
    return -1;
  }
  memset(out, 0, sizeof *out);
  out->fd = best_fd;
  snprintf(out->path, sizeof out->path, "%s", best_path);
  snprintf(out->name, sizeof out->name, "%s", best_name);
  uint8_t key[KEY_MAX / 8 + 1] = { 0 }, abs[ABS_MAX / 8 + 1] = { 0 };
  ioctl(best_fd, EVIOCGBIT(EV_KEY, sizeof key), key);
  ioctl(best_fd, EVIOCGBIT(EV_ABS, sizeof abs), abs);
  out->rubber = byte_bit(key, sizeof key, BTN_TOOL_RUBBER);
  out->stylus = byte_bit(key, sizeof key, BTN_STYLUS) || byte_bit(key, sizeof key, BTN_STYLUS2);
  for (unsigned a = 0; a < XH_PEN_AXES; a++) {
    struct input_absinfo info;
    if (!byte_bit(abs, sizeof abs, axis_code[a]) || ioctl(best_fd, EVIOCGABS(axis_code[a]), &info)) continue;
    out->axis[a] = (xh_pen_axis){ info.minimum, info.maximum, info.resolution, info.maximum > info.minimum };
  }
  // Event times on CLOCK_MONOTONIC for this reader (a property of the open file): the log's in-range times.
  int clock = CLOCK_MONOTONIC;
  (void)ioctl(best_fd, EVIOCSCLOCKID, &clock);
  snprintf(why, size, "%s", skipped[0] ? skipped + 2 : "");
  return 0;
}

int xh_pen_read_state(xh_pen_source *s, xh_pen_state *state) { return read_state(s, state); }
#else
int xh_pen_lookup_evdev(xh_pen_device *out, char *why, size_t size) { (void)out; snprintf(why, size, "not a Linux host"); return -1; }
int xh_pen_read_state(xh_pen_source *s, xh_pen_state *state) { (void)s; (void)state; return -1; }
#endif

xh_pen_lookup_fn xh_pen_lookup = xh_pen_lookup_evdev;
