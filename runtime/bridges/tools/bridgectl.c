// SPDX-License-Identifier: MIT
// bridgectl: tries each host bridge on the phone, without xnuhost or a guest. Run as root (su):
//   bridgectl cpu [NVCPU]            the host's cores and the vCPU plan made from them
//   bridgectl haptic [MS [INTENSITY [HZ]]]   one vibration (default 200 ms, 1000, resonance)
//   bridgectl haptic-pattern         three taps and a ramp: what an iOS pattern feels like
// The backend: ONEIOS_HW_BACKEND, ONEIOS_HW_BACKEND_<BRIDGE>, ONEIOS_HW_FALLBACK (bridges/hw_backend.h): haptic's is
// the evdev device (samsung) or the vibrator HAL (treble). Off the phone: ONEIOS_INPUT_DIR, ONEIOS_BINDER.
//   bridgectl bt                     opens the Bluetooth HAL, sends HCI Reset and Read BD_ADDR, prints the events
//   bridgectl net                    opens the tap, answers the first DHCP request it sees, counts frames for 10 s
//   bridgectl audio HELPER [S [record]]  a quiet 440 Hz tone for S s (2) through the helper (path of oneios-audio),
//                                    then 0.5 s later a 660 Hz one for 0.5 s;
//                                    with record, then 2 s of the microphone (frames, peak, RMS); first the helper's
//                                    probe (the speaker's latency xnuhost would publish to iOS)
//   bridgectl audio-loop HELPER [N]  N clicks (10), 0.5 s apart, each 20 ms of 1 kHz at -30 dBFS, while the microphone
//                                    records: the round trip of each (handed to the helper to heard by the microphone:
//                                    the speaker's latency, the air and the microphone's), and their median
// Each prints what it found and ends with "ok" or the reason it failed.
#define _GNU_SOURCE
#include "bridges/bridges.h"
#include "bridges/hw_backend.h"
#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int cpu(int argc, char **argv)
{
  br_cpu_topology t;
  br_cpu_plan plan;
  unsigned nvcpu = argc > 2 ? (unsigned)atoi(argv[2]) : 8;
  if (br_cpu_read("/sys", &t)) { printf("no CPUs under /sys/devices/system/cpu\n"); return 1; }
  printf("host: %u CPUs, %u P, %u E\n", t.ncpu, t.n_p, t.n_e);
  for (unsigned i = 0; i < t.ncpu; i++) {
    const br_host_cpu *c = &t.cpu[i];
    if (c->present) printf("  cpu%u: %c capacity %u part 0x%03x policy %u %u..%u kHz (%u steps) L1d %u L2 %u L3 %u\n", i, c->type, c->capacity, c->part, c->policy,
                           c->min_khz, c->max_khz, c->nfreq, c->l1d_bytes, c->l2_bytes, c->l3_bytes);
  }
  if (br_cpu_plan_make(&t, nvcpu, false, &plan)) { printf("no plan for %u vCPUs\n", nvcpu); return 1; }
  printf("plan: %u vCPUs, %u E then %u P\n", plan.nvcpu, plan.n_e, plan.n_p);
  for (unsigned i = 0; i < plan.nvcpu; i++)
    printf("  vcpu%u: %c cluster %u host mask 0x%llx capacity %u max %u kHz\n", i, plan.v[i].type, plan.v[i].cluster_id, (unsigned long long)plan.v[i].affinity,
           plan.v[i].capacity, plan.v[i].max_khz);
  int rc = br_cpu_place(0, &plan, plan.nvcpu - 1);
  printf("placing this thread as vcpu%u: %s\n", plan.nvcpu - 1, rc ? strerror(-rc) : "done");
  rc = br_cpu_request(0, &plan, plan.nvcpu - 1, plan.v[plan.nvcpu - 1].max_khz);
  printf("uclamp.min %u for the top frequency: %s\n", br_cpu_util_for(&plan, plan.nvcpu - 1, plan.v[plan.nvcpu - 1].max_khz), rc ? strerror(-rc) : "done");
  return 0;
}

static int haptic(int argc, char **argv, bool pattern)
{
  br_haptic *h = br_haptic_open_as(getenv("ONEIOS_INPUT_DIR"), BR_HW_UNSET, NULL);   // the backend: hw-backend-haptics
  if (!h) return 1;
  if (pattern) {
    for (unsigned i = 0; i < 3; i++) { br_haptic_vib(h, 15, 1000, 0); usleep(150000); }
    for (unsigned level = 100; level <= 1000; level += 100) { br_haptic_vib(h, 200, level, 0); usleep(60000); }
    usleep(300000);
  } else {
    unsigned ms = argc > 2 ? (unsigned)atoi(argv[2]) : 200, intensity = argc > 3 ? (unsigned)atoi(argv[3]) : 1000, hz = argc > 4 ? (unsigned)atoi(argv[4]) : 0;
    br_haptic_vib(h, ms, intensity, hz);
    usleep((ms + 100) * 1000);
  }
  br_haptic_close(h);
  return 0;
}

static volatile int bt_state = -1;
static volatile unsigned bt_events;
static void bt_rx(void *ctx, uint8_t type, const uint8_t *packet, size_t length)
{
  (void)ctx;
  printf("  <- type %u, %zu bytes:", type, length);
  for (size_t i = 0; i < length && i < 24; i++) printf(" %02x", packet[i]);
  printf("\n");
  bt_events++;
}
static void bt_changed(void *ctx, int state) { (void)ctx; bt_state = state; printf("  state %d\n", state); }

static int bt(void)
{
  static const uint8_t reset[] = { 0x03, 0x0c, 0x00 }, read_address[] = { 0x09, 0x10, 0x00 };
  uint8_t address[6];
  if (!br_bt_address("/mnt/vendor/efs/bluetooth/bt_addr", address)) printf("address file: %02x:%02x:%02x:%02x:%02x:%02x\n", address[0], address[1], address[2], address[3], address[4], address[5]);
  int hal = br_hci_hidl_probe();
  if (hal < 0) { printf("no Bluetooth HAL found (%d)\n", hal); return 1; }
  printf("HAL %s registered\n", hal == 11 ? "1.1" : "1.0");
  br_hci *h = br_hci_open_hidl(bt_rx, bt_changed, NULL);
  if (!h || br_hci_open(h)) return 1;
  for (unsigned i = 0; i < 200 && bt_state < 1; i++) usleep(100000);      // the firmware download takes seconds
  if (bt_state != 1) { printf("the HAL did not become ready (state %d)\n", bt_state); return 1; }
  int rc = br_hci_send(h, 1, reset, sizeof reset);
  usleep(500000);
  if (!rc) rc = br_hci_send(h, 1, read_address, sizeof read_address);
  usleep(500000);
  br_hci_close(h);
  usleep(200000);
  if (rc || bt_events < 2) { printf("send %d, %u events (2 expected)\n", rc, bt_events); return 1; }
  printf("bt: ok: %u events (Command Complete of HCI Reset and Read BD_ADDR expected)\n", bt_events);
  return 0;
}

static volatile unsigned net_frames;
static void net_rx(void *ctx, const uint8_t *frame, size_t length) { (void)ctx; (void)frame; (void)length; net_frames++; }

static int net(void)
{
  br_net_config c;
  br_net_config_defaults(&c);
  br_net *n = br_net_open_tap(&c, net_rx, NULL);
  if (!n) return 1;
  printf("tap %s is open; run `oneios-net.sh up` in another shell, then `ping -I %s 192.168.77.2` shows ARP requests here\n", c.ifname, c.ifname);
  sleep(10);
  printf("%u frames from the tap in 10 s\n", net_frames);
  br_net_close(n);
  return 0;
}

// A quiet tone (-30 dBFS: never loud, whatever the media volume) paced by the clock as the guest's engine paces
// its periods; then, with `record`, two seconds of the microphone: frames, peak and RMS. The helper's own lines
// (its stream's mode, the ring's report) are on stderr.
static int audio(int argc, char **argv)
{
  const char *path = argv[2];
  unsigned seconds = argc > 3 ? (unsigned)atoi(argv[3]) : 2;
  bool record = argc > 4 && !strcmp(argv[4], "record");
  if (seconds < 1 || seconds > 30) seconds = 2;
  void *ctx;
  const br_audio_backend *b = br_audio_helper(path, &ctx);
  if (!b) return 1;
  int probed = br_audio_probe_latency(b, ctx, 2000);
  if (probed > 0) printf("audio: probe: speaker latency %d frames (%.1f ms): oneios-audio-latency=%d in a session\n", probed, probed / 48.0, probed);
  else printf("audio: probe: not measured (%s)\n", strerror(-probed));
  if (b->start(ctx, true, 48000, 2)) return 1;
  int16_t period[480];
  double phase = 0;
  struct timespec at;
  clock_gettime(CLOCK_MONOTONIC, &at);
  unsigned sent = 0;
  for (unsigned p = 0; p < seconds * 200; p++) {                // 5 ms periods
    for (unsigned i = 0; i < 240; i++) { period[2 * i] = period[2 * i + 1] = (int16_t)(1000 * sin(phase)); phase += 2 * M_PI * 440 / 48000; }
    if (b->write(ctx, period, 240) == 240) sent++;
    at.tv_nsec += 5000000;
    if (at.tv_nsec >= 1000000000) { at.tv_nsec -= 1000000000; at.tv_sec++; }
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &at, NULL);
  }
  printf("audio: %u of %u periods of a 440 Hz tone at -30 dBFS sent\n", sent, seconds * 200);
  b->stop(ctx, true);
  // A second, short tone 0.5 s later, as iOS stops and starts its device around each sound: the helper's stream
  // lingers, so this one starts without the track's stall (its report says so).
  usleep(500000);
  if (b->start(ctx, true, 48000, 2)) return 1;
  clock_gettime(CLOCK_MONOTONIC, &at);
  for (unsigned p = 0; p < 100; p++) {
    for (unsigned i = 0; i < 240; i++) { period[2 * i] = period[2 * i + 1] = (int16_t)(1000 * sin(phase)); phase += 2 * M_PI * 660 / 48000; }
    (void)b->write(ctx, period, 240);
    at.tv_nsec += 5000000;
    if (at.tv_nsec >= 1000000000) { at.tv_nsec -= 1000000000; at.tv_sec++; }
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &at, NULL);
  }
  b->stop(ctx, true);
  int rc = sent ? 0 : 1;
  if (record) {
    if (b->start(ctx, false, 48000, 1)) { b->close(ctx); return 1; }
    int16_t pcm[240];
    uint64_t frames = 0, counted = 0;
    double sum = 0;
    int peak = 0;
    struct timespec t0, t;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    do {
      int n = b->read(ctx, pcm, 240);
      if (n < 0) break;
      // The first 250 ms are the input's start (a full-scale click on the phone): not counted in the peak and RMS.
      for (int i = 0; i < n && frames + (uint64_t)i >= 12000; i++) { int v = abs(pcm[i]); if (v > peak) peak = v; sum += (double)pcm[i] * pcm[i]; counted++; }
      frames += (uint64_t)(n > 0 ? n : 0);
      clock_gettime(CLOCK_MONOTONIC, &t);
    } while ((t.tv_sec - t0.tv_sec) * 1000 + (t.tv_nsec - t0.tv_nsec) / 1000000 < 2000);
    b->stop(ctx, false);
    double rms = counted ? sqrt(sum / (double)counted) : 0;
    printf("audio: microphone: %llu frames in 2 s (96000 expected), peak %d (%.1f dBFS), RMS %.1f dBFS%s\n", (unsigned long long)frames, peak,
           peak ? 20 * log10(peak / 32768.0) : -999.0, rms > 0 ? 20 * log10(rms / 32768.0) : -999.0,
           frames && !peak ? " (all zero: silenced?)" : "");
    if (!frames) rc = 1;
  }
  usleep(300000);                                               // the helper's last report
  b->close(ctx);
  usleep(100000);
  return rc;
}

// audio-loop: the microphone's frames, each with the time it came in here (a reader thread), against the clicks,
// each with the time its first period was handed to the helper. A frame's time is its period's arrival less the
// frames after it in that period: it includes the microphone's own path (capture, AAudio, the helper, the socket),
// so a round trip is the speaker's latency plus the air plus the microphone's latency. The helper's speaker report
// (stderr) says the speaker's part from AAudio's timestamps; the difference is the microphone's and the air's.
typedef struct { const br_audio_backend *b; void *ctx; int16_t *pcm; double *at; unsigned frames, cap; bool run; } loop_mic;
static double mono_s(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (double)t.tv_sec + t.tv_nsec / 1e9; }
static void *loop_reader(void *arg)
{
  loop_mic *m = arg;
  int16_t pcm[240];
  while (__atomic_load_n(&m->run, __ATOMIC_ACQUIRE) && m->frames + 240 <= m->cap) {
    int n = m->b->read(m->ctx, pcm, 240);
    if (n < 0) break;
    if (n == 0) continue;
    double now = mono_s();
    memcpy(m->pcm + m->frames, pcm, (size_t)n * 2);
    for (int i = 0; i < n; i++) m->at[m->frames + (unsigned)i] = now - (double)(n - 1 - i) / 48000;
    m->frames += (unsigned)n;
  }
  return NULL;
}
static int cmp_double(const void *a, const void *b) { double x = *(const double *)a, y = *(const double *)b; return x < y ? -1 : x > y; }

static int audio_loop(int argc, char **argv)
{
  const char *path = argv[2];
  unsigned clicks = argc > 3 ? (unsigned)atoi(argv[3]) : 10;
  if (clicks < 1 || clicks > 20) clicks = 10;
  void *ctx;
  const br_audio_backend *b = br_audio_helper(path, &ctx);
  if (!b) return 1;
  loop_mic m = { .b = b, .ctx = ctx, .cap = 48000 * 14, .run = true };
  m.pcm = calloc(m.cap, sizeof *m.pcm);
  m.at = calloc(m.cap, sizeof *m.at);
  double sent[20] = { 0 }, heard[20];
  pthread_t reader;
  if (!m.pcm || !m.at || b->start(ctx, false, 48000, 1) || b->start(ctx, true, 48000, 2) || pthread_create(&reader, NULL, loop_reader, &m)) {
    printf("audio-loop: the speaker or the microphone did not start\n");
    b->close(ctx);
    return 1;
  }
  // 1 s of silence (the microphone's start click, the noise floor), then a click every 0.5 s: 4 periods of 1 kHz.
  int16_t period[480];
  struct timespec at;
  clock_gettime(CLOCK_MONOTONIC, &at);
  const unsigned periods = 200 + clicks * 100 + 60;
  for (unsigned p = 0, k = 0; p < periods; p++) {
    bool tone = p >= 200 && (p - 200) % 100 < 4 && (p - 200) / 100 < clicks;
    for (unsigned i = 0; i < 240; i++) period[2 * i] = period[2 * i + 1] = tone ? (int16_t)(1000 * sin(2 * M_PI * 1000 * i / 48000.0)) : 0;
    if (tone && (p - 200) % 100 == 0) sent[k++] = mono_s();
    (void)b->write(ctx, period, 240);
    at.tv_nsec += 5000000;
    if (at.tv_nsec >= 1000000000) { at.tv_nsec -= 1000000000; at.tv_sec++; }
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &at, NULL);
  }
  b->stop(ctx, true);
  usleep(200000);
  __atomic_store_n(&m.run, false, __ATOMIC_RELEASE);
  pthread_join(reader, NULL);
  b->stop(ctx, false);
  // The noise: RMS of 1 ms windows from 0.4 s in to the first click; the threshold 8 times their median (18 dB), at
  // least -56 dBFS.
  double windows[1000];
  unsigned nw = 0;
  for (unsigned f = 0; f + 48 <= m.frames && nw < 1000; f += 48) {
    if (m.at[f] < sent[0] - 0.6 || m.at[f + 47] >= sent[0]) continue;
    double s = 0;
    for (unsigned i = 0; i < 48; i++) s += (double)m.pcm[f + i] * m.pcm[f + i];
    windows[nw++] = sqrt(s / 48);
  }
  qsort(windows, nw, sizeof *windows, cmp_double);
  double noise = nw ? windows[nw / 2] : 0, threshold = noise * 8 > 50 ? noise * 8 : 50;
  unsigned found = 0;
  for (unsigned k = 0; k < clicks; k++) {
    heard[k] = -1;
    for (unsigned f = 0; f + 48 <= m.frames; f += 8) {
      if (m.at[f] < sent[k]) continue;
      if (m.at[f] > sent[k] + 0.4) break;
      double s = 0;
      for (unsigned i = 0; i < 48; i++) s += (double)m.pcm[f + i] * m.pcm[f + i];
      if (sqrt(s / 48) > threshold) { heard[k] = (m.at[f] - sent[k]) * 1e3; break; }
    }
    if (heard[k] >= 0) printf("audio-loop: click %u: heard %.1f ms after it was handed to the helper\n", k + 1, heard[k]);
    else printf("audio-loop: click %u: not heard within 400 ms\n", k + 1);
    if (heard[k] >= 0) heard[found++] = heard[k];
  }
  qsort(heard, found, sizeof *heard, cmp_double);
  printf("audio-loop: %u frames from the microphone, noise %.1f dBFS, threshold %.1f dBFS\n", m.frames,
         noise > 0 ? 20 * log10(noise / 32768.0) : -999.0, 20 * log10(threshold / 32768.0));
  if (found) printf("audio-loop: round trip (the speaker + the air + the microphone) median %.1f ms, %.1f..%.1f, %u of %u clicks\n",
                    heard[found / 2], heard[0], heard[found - 1], found, clicks);
  usleep(300000);                                               // the helper's last report
  b->close(ctx);
  usleep(100000);
  free(m.pcm);
  free(m.at);
  return found * 2 >= clicks ? 0 : 1;
}

// The bridges' own lines (why a HAL was not found, the HAL's state), as xnuhost logs them.
static void say(int level, const char *line) { printf("  %s%s\n", level == 0 ? "error: " : "", line); }

int main(int argc, char **argv)
{
  int rc = 2;
  setvbuf(stdout, NULL, _IOLBF, 0);
  br_log_sink = say;
  br_hw_settings hw;
  const char *bad = NULL;
  br_hw_settings_init(&hw);
  if (br_hw_settings_env(&hw, &bad)) { fprintf(stderr, "bridgectl: bad %s (samsung or treble; ONEIOS_HW_FALLBACK 0 or 1)\n", bad); return 2; }
  br_hw_configure(&hw);
  if (argc < 2) { fprintf(stderr, "usage: bridgectl cpu|haptic|haptic-pattern|bt|net|audio|audio-loop ... (see tools/bridgectl.c)\n"); return 2; }
  if (!strcmp(argv[1], "cpu")) rc = cpu(argc, argv);
  else if (!strcmp(argv[1], "haptic")) rc = haptic(argc, argv, false);
  else if (!strcmp(argv[1], "haptic-pattern")) rc = haptic(argc, argv, true);
  else if (!strcmp(argv[1], "bt")) rc = bt();
  else if (!strcmp(argv[1], "net")) rc = net();
  else if (!strcmp(argv[1], "audio") && argc > 2) rc = audio(argc, argv);
  else if (!strcmp(argv[1], "audio-loop") && argc > 2) rc = audio_loop(argc, argv);
  printf("%s\n", rc == 0 ? "ok" : rc == 2 ? "unknown command" : "failed");
  return rc;
}
