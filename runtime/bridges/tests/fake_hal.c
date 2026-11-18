// SPDX-License-Identifier: MIT
// fake-hal: a stand-in servicemanager with stand-in HALs on a binder device, for the treble backend's tests
// (tests/test_hal_binder.c) and the stand-in phone (tools/oneios-tool/tests/phone/hw-backend.sh). Both ends use
// wifi/aidl.h and hal/hal.h: what it proves is the client's protocol (the lookup by name, the descriptor and version
// checks, the codes, the parcels as hal.h writes them), not the vendor's HALs; those are the phone's.
//
//   fake-hal DIR     the context manager of ONEIOS_BINDER (a binderfs device), until killed; "ready" on stdout.
// Each HAL is registered when its file is there, and the file is read again at every call:
//   DIR/health       android.hardware.health.IHealth/default: key=value lines (level, status 1..5, present, temp
//                    in tenths, voltage_mv, current_ua, cycles, full_uah, design_uah, ac, usb, wireless, dock)
//   DIR/thermal      android.hardware.thermal.IThermal/default: lines "TYPE NAME CELSIUS" (TYPE: TemperatureType)
//   DIR/vibrator     android.hardware.vibrator.IVibrator/default: its capabilities (a number; empty: 4, amplitude);
//                    every call appended to DIR/vibrator.log ("on 30", "off", "amplitude 0.50")
#define _GNU_SOURCE
#include "../hal/hal.h"
#include <errno.h>
#include <stdarg.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum { OBJ_MANAGER = 0, OBJ_HEALTH = 1, OBJ_THERMAL = 2, OBJ_VIBRATOR = 3 };
static char dir[256];

static bool file_text(const char *name, char *out, size_t size)
{
  char path[320];
  snprintf(path, sizeof path, "%s/%s", dir, name);
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return false;
  ssize_t n = read(fd, out, size - 1);
  close(fd);
  out[n > 0 ? n : 0] = 0;
  return true;
}

static void note(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void note(const char *fmt, ...)
{
  char path[320], line[96];
  va_list ap;
  va_start(ap, fmt);
  int n = vsnprintf(line, sizeof line - 1, fmt, ap);
  va_end(ap);
  line[n++] = '\n';
  snprintf(path, sizeof path, "%s/vibrator.log", dir);
  int fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
  if (fd >= 0) { (void)!write(fd, line, (size_t)n); close(fd); }
}

static void health_of(hal_health_info *h)
{
  char text[1024];
  memset(h, 0, sizeof *h);
  snprintf(h->technology, sizeof h->technology, "Li-ion");
  if (!file_text("health", text, sizeof text)) return;
  for (char *line = strtok(text, "\n"); line; line = strtok(NULL, "\n")) {
    char key[32];
    long v;
    if (sscanf(line, "%31[^=]=%ld", key, &v) != 2) continue;
    int32_t x = (int32_t)v;
    if (!strcmp(key, "level")) h->level = x;
    else if (!strcmp(key, "status")) h->status = x;
    else if (!strcmp(key, "present")) h->present = x;
    else if (!strcmp(key, "temp")) h->temp_deci_c = x;
    else if (!strcmp(key, "voltage_mv")) h->voltage_mv = x;
    else if (!strcmp(key, "current_ua")) h->current_ua = x;
    else if (!strcmp(key, "cycles")) h->cycles = x;
    else if (!strcmp(key, "full_uah")) h->full_uah = x;
    else if (!strcmp(key, "design_uah")) h->design_uah = x;
    else if (!strcmp(key, "ac")) h->ac = x;
    else if (!strcmp(key, "usb")) h->usb = x;
    else if (!strcmp(key, "wireless")) h->wireless = x;
    else if (!strcmp(key, "dock")) h->dock = x;
  }
}

static unsigned temps_of(hal_temperature *t, unsigned max)
{
  char text[2048];
  unsigned n = 0;
  if (!file_text("thermal", text, sizeof text)) return 0;
  for (char *line = strtok(text, "\n"); line && n < max; line = strtok(NULL, "\n")) {
    hal_temperature one = { 0 };
    if (sscanf(line, "%d %23s %f", &one.type, one.name, &one.celsius) == 3) t[n++] = one;
  }
  return n;
}

static int incoming(void *ctx, uint64_t object, uint32_t code, uint32_t flags, ab_reader *r, ab_parcel *reply)
{
  (void)ctx; (void)flags;
  char text[128];
  ab_init(reply);
  if (object == OBJ_MANAGER) {
    if (code != HAL_SM_GET_SERVICE || !ab_get_token(r, HAL_SERVICE_MANAGER) || !ab_get_string(r, text, sizeof text)) return AB_UNKNOWN_TRANSACTION;
    ab_put_ok(reply);
    char probe[8];
    if (!strcmp(text, HAL_HEALTH_INSTANCE) && file_text("health", probe, sizeof probe)) ab_put_local(reply, OBJ_HEALTH);
    else if (!strcmp(text, HAL_THERMAL_INSTANCE) && file_text("thermal", probe, sizeof probe)) ab_put_local(reply, OBJ_THERMAL);
    else if (!strcmp(text, HAL_VIBRATOR_INSTANCE) && file_text("vibrator", probe, sizeof probe)) ab_put_local(reply, OBJ_VIBRATOR);
    else ab_put_null_binder(reply);
    return 0;
  }
  const char *descriptor = object == OBJ_HEALTH ? HAL_HEALTH : object == OBJ_THERMAL ? HAL_THERMAL : HAL_VIBRATOR;
  if (ab_standard(code, descriptor, 2, "fake", reply)) return 0;
  if (!ab_get_token(r, descriptor)) return AB_BAD_TYPE;
  if (object == OBJ_HEALTH && code == HAL_HEALTH_GET_INFO) {
    hal_health_info h;
    health_of(&h);
    ab_put_ok(reply);
    hal_health_encode(reply, &h);
    return 0;
  }
  if (object == OBJ_THERMAL && code == HAL_THERMAL_GET_TEMPERATURES) {
    hal_temperature t[HAL_TEMPS_MAX];
    unsigned n = temps_of(t, HAL_TEMPS_MAX);
    ab_put_ok(reply);
    hal_temperatures_encode(reply, t, n);
    return 0;
  }
  if (object == OBJ_VIBRATOR) {
    switch (code) {
    case HAL_VIB_GET_CAPABILITIES: {
      int caps = file_text("vibrator", text, sizeof text) && *text ? atoi(text) : HAL_VIB_CAP_AMPLITUDE;
      ab_put_ok(reply);
      ab_put_i32(reply, caps);
      return 0;
    }
    case HAL_VIB_OFF: note("off"); ab_put_ok(reply); return 0;
    case HAL_VIB_ON: {
      int32_t ms = ab_get_i32(r);
      uint32_t handle;
      uint64_t local;
      bool callback = ab_get_binder(r, &handle, &local);
      note("on %d%s", ms, callback ? " with a callback" : "");
      ab_put_ok(reply);
      return 0;
    }
    case HAL_VIB_SET_AMPLITUDE: {
      float a = ab_get_float(r);
      if (!(a > 0.0f && a <= 1.0f)) { note("amplitude %.2f refused", (double)a); ab_put_service_error(reply, 1); return 0; }   // EX_ILLEGAL_ARGUMENT's role
      note("amplitude %.2f", (double)a);
      ab_put_ok(reply);
      return 0;
    }
    default: break;
    }
  }
  return AB_UNKNOWN_TRANSACTION;
}

int fake_hal_serve(const char *values, int ready_fd)
{
  snprintf(dir, sizeof dir, "%s", values);
  ab *b = ab_open(NULL, incoming, NULL, NULL);
  if (!b) { fprintf(stderr, "fake-hal: no binder device (ONEIOS_BINDER): %s\n", strerror(errno)); return 91; }
  int rc = ab_context_manager(b);          // it waits out a killed predecessor (EBUSY, 2 s)
  if (rc) { fprintf(stderr, "fake-hal: not the context manager: %s\n", strerror(errno)); return 92; }
  (void)!write(ready_fd, "ready\n", 6);
  for (;;) pause();
}

#ifndef FAKE_HAL_LIB
#include <linux/android/binderfs.h>
#include <sys/ioctl.h>
int main(int argc, char **argv)
{
  if (argc != 2) { fprintf(stderr, "usage: ONEIOS_BINDER=DEVICE fake-hal DIR\n"); return 2; }
  // A binderfs of a kernel with no default devices has only binder-control: ask it for the device by name.
  const char *device = getenv("ONEIOS_BINDER"), *slash = device ? strrchr(device, '/') : NULL;
  if (slash && access(device, F_OK)) {
    char control[512];
    snprintf(control, sizeof control, "%.*s/binder-control", (int)(slash - device), device);
    int fd = open(control, O_RDWR | O_CLOEXEC);
    struct binderfs_device d = { 0 };
    snprintf(d.name, sizeof d.name, "%s", slash + 1);
    if (fd >= 0) { (void)ioctl(fd, BINDER_CTL_ADD, &d); close(fd); }
  }
  setvbuf(stdout, NULL, _IONBF, 0);
  return fake_hal_serve(argv[1], 1);
}
#endif
