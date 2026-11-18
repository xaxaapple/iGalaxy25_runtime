// SPDX-License-Identifier: MIT
// The treble backend's HAL bridges against fake HALs on a real binder driver (`make test-binder`: a binderfs in a user
// namespace; skipped without one). A child plays servicemanager and the HALs (tests/fake_hal.c, values from files of
// a folder, read at every call); the parent opens the bridges with hw-backend treble and checks what they read and
// call: the lookup by name, the descriptor check, the parcels, a HAL that is not registered (the one missing line,
// then off, or the samsung form with hw-backend-fallback=1), and a HAL whose process dies under the bridge.
#define _GNU_SOURCE
#include "bridges/bridges.h"
#include "bridges/hw_backend.h"
#include "../battery/battery.h"
#include "../thermal/thermal.h"
#include "../hal/hal.h"
#include <errno.h>
#include <fcntl.h>
#include <linux/android/binderfs.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

int fake_hal_serve(const char *values, int ready_fd);

static int failures, checks;
#define CHECK(cond) do { checks++; if (!(cond)) { failures++; printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); fflush(stdout); } } while (0)
static char lines[8192];
static void log_line(int level, const char *line)
{
  printf("  [%s] %s\n", level == 0 ? "E" : "I", line);
  size_t n = strlen(lines);
  snprintf(lines + n, sizeof lines - n, "%s\n", line);
}
static bool logged(const char *text) { return strstr(lines, text) != NULL; }

static char values[] = "/tmp/oneios-hal-XXXXXX";
static void put(const char *name, const char *text)
{
  char path[300];
  snprintf(path, sizeof path, "%s/%s", values, name);
  FILE *f = fopen(path, "w");
  if (f) { fputs(text, f); fclose(f); }
}

static pid_t start_fake(void)
{
  int fds[2];
  char line[8] = "";
  if (pipe(fds)) return -1;
  pid_t pid = fork();
  if (pid == 0) { close(fds[0]); _exit(fake_hal_serve(values, fds[1])); }
  close(fds[1]);
  CHECK(read(fds[0], line, 6) == 6 && !memcmp(line, "ready\n", 6));
  close(fds[0]);
  return pid;
}
static void stop_fake(pid_t pid) { kill(pid, SIGKILL); waitpid(pid, NULL, 0); }

static void binderfs_add(const char *path)
{
  const char *slash = strrchr(path, '/');
  if (!slash || access(path, F_OK) == 0) return;
  char control[512];
  snprintf(control, sizeof control, "%.*s/binder-control", (int)(slash - path), path);
  int fd = open(control, O_RDWR | O_CLOEXEC);
  if (fd < 0) return;
  struct binderfs_device d = { 0 };
  snprintf(d.name, sizeof d.name, "%s", slash + 1);
  (void)ioctl(fd, BINDER_CTL_ADD, &d);
  close(fd);
}

static void test_battery(void)
{
  put("health", "level=73\nstatus=2\npresent=1\ntemp=312\nvoltage_mv=4105\ncurrent_ua=1250000\ncycles=41\nfull_uah=4850000\ndesign_uah=5000000\nusb=1\n");
  pid_t pid = start_fake();
  br_battery_options o = { .backend = BR_HW_TREBLE, .dir = "/nonexistent", .poll_ms = 50 };
  lines[0] = 0;
  br_battery *b = br_battery_open(&o);
  CHECK(b != NULL);
  CHECK(logged("hw: battery: backend treble (the caller's choice)") && logged("battery: on: the health HAL android.hardware.health.IHealth/default (version 2), status \"Charging\", 73%, plugged in"));
  if (b) {
    br_battery_sample s;
    CHECK(br_battery_query(b, &s) == 0);
    CHECK(s.percent == 73 && s.present && s.plugged && s.charging && !s.full && s.temp_deci_c == 312 && s.voltage_mv == 4105);
    CHECK(s.current_ma == 1250 && s.full_mah == 4850 && s.design_mah == 5000 && s.cycles == 41 && s.minutes == (4850 - 4850 * 73 / 100) * 60 / 1250);
    // The HAL is asked again at every read: a change shows at the next query.
    put("health", "level=72\nstatus=3\npresent=1\ntemp=300\nvoltage_mv=4000\ncurrent_ua=-400000\ncycles=41\nfull_uah=4850000\n");
    CHECK(br_battery_query(b, &s) == 0 && s.percent == 72 && !s.plugged && !s.charging && s.current_ma == -400 && s.design_mah == 4850);
    // The HAL's process dies: the read fails, iOS keeps the last level, one error line.
    stop_fake(pid);
    pid = -1;
    lines[0] = 0;
    CHECK(br_battery_query(b, &s) == -1 && s.percent == 72);
    CHECK(logged("battery: the health HAL's getHealthInfo failed"));
    br_battery_close(b);
  }
  if (pid > 0) stop_fake(pid);

  // Not registered: the one missing line, then off; with hw-backend-fallback=1 the samsung nodes.
  char path[300];
  snprintf(path, sizeof path, "%s/health", values);
  unlink(path);
  pid = start_fake();
  lines[0] = 0;
  CHECK(br_battery_open(&o) == NULL);
  CHECK(logged("hw: battery: treble: missing: android.hardware.health.IHealth/default (no service: not registered with servicemanager, or SELinux denies the lookup); the bridge stays off (hw-backend-fallback=0)"));
  char sys[200];
  snprintf(sys, sizeof sys, "%s/power_supply", values);
  mkdir(sys, 0755);
  snprintf(path, sizeof path, "%s/battery", sys);
  mkdir(path, 0755);
  put("power_supply/battery/capacity", "55\n");
  put("power_supply/battery/status", "Discharging\n");
  br_hw_settings hw;
  br_hw_settings_init(&hw);
  CHECK(br_hw_setting(&hw, "hw-backend-fallback", "1") == 0);
  br_hw_configure(&hw);
  o.dir = sys;
  lines[0] = 0;
  br_battery *f = br_battery_open(&o);
  CHECK(f != NULL && logged("falling back to samsung (hw-backend-fallback=1)") && logged("battery: on: ") && logged("55%"));
  if (f) br_battery_close(f);
  br_hw_configure(NULL);
  stop_fake(pid);
}

static void test_thermal(void)
{
  put("thermal", "0 cpu-0-0 48.5\n0 cpu-1-2 61.2\n1 gpu 55.0\n2 battery 30.4\n3 skin 37.1\n");
  pid_t pid = start_fake();
  br_thermal_options o = { .backend = BR_HW_TREBLE, .root = "/nonexistent", .poll_ms = 50, .load_ms = 3600000 };
  lines[0] = 0;
  br_thermal *t = br_thermal_open(&o);
  CHECK(t != NULL);
  CHECK(logged("hw: thermal: backend treble (the caller's choice)") && logged("thermal: on: the thermal HAL android.hardware.thermal.IThermal/default (version 2), 5 temperatures; case 37.1 C (SKIN), SoC 61.2 C (cpu-1-2), battery 30.4 C; nominal"));
  if (t) {
    br_thermal_state s;
    br_thermal_get(t, &s);
    CHECK(br_thermal_backend(t) == BR_HW_TREBLE && s.raw.cpu == 612 && s.raw.gpu == 550 && s.raw.skin == 371 && s.raw.battery == 304 && s.level == BR_THERMAL_NOMINAL);
    // The worker asks again: a hot case is the same level as Samsung's lrp would give.
    put("thermal", "0 cpu-1-2 70.0\n3 skin 44.5\n2 battery 33.0\n");
    usleep(300000);
    br_thermal_get(t, &s);
    CHECK(s.raw.skin == 445 && s.raw.cpu == 700 && s.level == BR_THERMAL_HEAVY);
    stop_fake(pid);
    pid = -1;
    lines[0] = 0;
    for (int i = 0; i < 60 && !logged("getTemperatures failed"); i++) usleep(50000);   // the next reading, within 3 s
    usleep(100000);
    br_thermal_get(t, &s);
    CHECK(logged("thermal: the thermal HAL's getTemperatures failed") && s.raw.skin == BR_THERMAL_UNKNOWN);
    br_thermal_close(t);
  }
  if (pid > 0) stop_fake(pid);
  char path[300];
  snprintf(path, sizeof path, "%s/thermal", values);
  unlink(path);
  pid = start_fake();
  lines[0] = 0;
  CHECK(br_thermal_open(&o) == NULL && logged("hw: thermal: treble: missing: android.hardware.thermal.IThermal/default (no service: ") && logged("the bridge stays off"));
  put("thermal", "");                                               // registered, but no temperature
  lines[0] = 0;
  CHECK(br_thermal_open(&o) == NULL && logged("treble: missing: android.hardware.thermal.IThermal/default: getTemperatures: no temperature"));
  stop_fake(pid);
}

static char *vib_log(void)
{
  static char text[2048];
  char path[300];
  snprintf(path, sizeof path, "%s/vibrator.log", values);
  FILE *f = fopen(path, "r");
  size_t n = f ? fread(text, 1, sizeof text - 1, f) : 0;
  if (f) fclose(f);
  text[n] = 0;
  unlink(path);
  return text;
}

static void test_haptics(void)
{
  put("vibrator", "4\n");                                          // CAP_AMPLITUDE_CONTROL
  pid_t pid = start_fake();
  lines[0] = 0;
  br_haptic *h = br_haptic_open_as("/nonexistent", BR_HW_TREBLE, NULL);
  CHECK(h != NULL && logged("hw: haptics: backend treble (the caller's choice)") && logged("haptics: on: the vibrator HAL android.hardware.vibrator.IVibrator/default (version 2, capabilities 0x4: amplitude control"));
  if (h) {
    br_haptic_vib(h, 30, 1000, 0);                                  // full intensity: amplitude 1
    br_haptic_flush(h);
    CHECK(!strcmp(vib_log(), "on 30\namplitude 1.00\n"));
    br_haptic_vib(h, 80, 500, 0);                                   // half intensity: level 147 of 255
    br_haptic_flush(h);
    char *log = vib_log();
    CHECK(strstr(log, "on 80\namplitude 0.58\n") && strstr(log, "refused") == NULL);
    br_haptic_stop(h);
    br_haptic_flush(h);
    CHECK(!strcmp(vib_log(), "off\n"));
    br_haptic_stats st;
    br_haptic_stats_get(h, &st);
    CHECK(st.played == 2 && st.stopped >= 1 && st.failed == 0);
    br_haptic_close(h);                                             // the close stops the motor
    CHECK(!strcmp(vib_log(), "off\n"));
  }
  // No amplitude control: on() only.
  put("vibrator", "0\n");
  h = br_haptic_open_as("/nonexistent", BR_HW_TREBLE, NULL);
  CHECK(h != NULL && logged("no amplitude control"));
  if (h) { br_haptic_vib(h, 20, 300, 0); br_haptic_flush(h); CHECK(!strcmp(vib_log(), "on 20\n")); br_haptic_close(h); vib_log(); }
  // Not registered: the missing line, off; with the fallback the evdev search (nothing under /nonexistent here).
  char path[300];
  snprintf(path, sizeof path, "%s/vibrator", values);
  unlink(path);
  lines[0] = 0;
  CHECK(br_haptic_open_as("/nonexistent", BR_HW_TREBLE, NULL) == NULL && logged("hw: haptics: treble: missing: android.hardware.vibrator.IVibrator/default (no service: ") && logged("the bridge stays off"));
  CHECK(!logged("no force-feedback device"));
  br_hw_settings hw;
  br_hw_settings_init(&hw);
  CHECK(br_hw_setting(&hw, "hw-backend-fallback", "1") == 0);
  br_hw_configure(&hw);
  lines[0] = 0;
  CHECK(br_haptic_open_as("/nonexistent", BR_HW_TREBLE, NULL) == NULL && logged("falling back to samsung") && logged("haptics: off: no force-feedback device with FF_PERIODIC + FF_SINE under /nonexistent"));
  br_hw_configure(NULL);
  stop_fake(pid);
}

int main(void)
{
  setvbuf(stdout, NULL, _IOLBF, 0);
  br_log_sink = log_line;
  const char *device = getenv("ONEIOS_BINDER");
  if (device) binderfs_add(device);
  if (!device || access(device, R_OK | W_OK)) { printf("test_hal_binder: set ONEIOS_BINDER to a binderfs binder device (make test-binder does)\n"); return 77; }
  if (!mkdtemp(values)) return 1;
  test_battery();
  test_thermal();
  test_haptics();
  char command[300];
  snprintf(command, sizeof command, "rm -rf %s", values);
  CHECK(system(command) == 0);
  printf("hal binder: %d checks, %d failed\n", checks, failures);
  return failures != 0;
}
