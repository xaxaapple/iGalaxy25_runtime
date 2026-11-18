// SPDX-License-Identifier: MIT
// Host test of the AAudio helper (audio/aaudio_helper.c, compiled in here with its main renamed) against a fake
// AAudio. The fake speaker is a stand-in for AudioFlinger's normal mixer: a server thread that, once the track has
// been filled to its buffer size (AudioFlinger's start threshold) and after a start stall, takes `pull` frames
// every pull period on its own clock, and takes nothing in a period where the track holds less (AudioFlinger mixes
// a normal track only when it holds a whole period). The phone's shape is burst 1600, pull 1600 (33 ms), buffer
// 3200 (docs/hardware.md section 6). The fake microphone gives 48 kHz in real time. A producer plays the guest:
// 240-frame periods every 5 ms on a clock that runs faster or slower than the server's.
//
// Since 2026-11-15 the fake HAL may also have MMAP (fake_hal): a LOW_LATENCY stream is then MMAP exclusive or shared,
// read a burst at a time from its start (the DSP, AAudio's endpoint), its timestamp a fixed number of frames behind
// its read; the helper's path choice (exclusive, shared only with system_server, a mixer track), its level on MMAP,
// its end-to-end figure and the probe are checked against it.
// Checks: no underrun of the helper or of the server after the start, no overflow, the drift steered the right way,
// the latency bounded, the microphone's periods reach the socket, the stop is clean, the linger, the target back to
// its floor after underruns, a stall's backlog gone at the first silence.
// Latency per stage (printed as "stage: ..."): marker periods (240 equal frames, a value no other frame has) are
// timed when the guest sends them, when the helper writes them to AAudio (socket + ring) and when the server reads
// them (AAudio's queue); Android's mixer and HAL after that are not modelled (the phone's share, docs/hardware.md).
#define main helper_main
#include "../audio/aaudio_helper.c"
#undef main
#include <math.h>

static int failures, checks;
#define CHECK(c) do { checks++; if (!(c)) { failures++; fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); } } while (0)

// ---- the fake AAudio ----------------------------------------------------------------------------
#define MARKS 256
struct AAudioStreamBuilderStruct { int direction, rate, channels, perf, sharing; };
struct AAudioStreamStruct {
  int direction, rate, channels, burst, pull, buffer, perf, sharing, capacity;
  int mmap;                                             // 0 a mixer track; 1 MMAP (exclusive: the DSP, shared: AAudio's endpoint)
  pthread_mutex_t lock;
  pthread_t server;
  bool server_on, filled, closed;
  atomic_bool quit;
  struct timespec t0;                                   // the microphone's start; the speaker's first write
  bool started;
  int64_t written, read;
  uint64_t dac_underflows, nonzero, pulls_done;
  double last_pull;                                     // when the server last took frames
  // markers: the guest's send time (filled by the test), the write time and position, the read time
  int next_mark;
  double sent[MARKS], wrote[MARKS], taken[MARKS];
  int64_t at[MARKS];
};
static double dac_ppm;                                  // the server's rate against the producer's clock
static double dac_stall_s;                              // a new track takes nothing for this long after it is filled
// How late the fake server's pulls and the guest's sends ran against their schedules (s): a loaded host (other
// builds on the same CPUs) makes the threads of this test late, and then an underrun is the host's, not the
// helper's. The checks that need an on-time host are skipped (and say so) when this passes HOST_LATE.
static double host_late_max;
#define HOST_LATE 0.004                                 // an idle host runs them within about 1 ms
static bool host_on_time(const char *what)
{
  if (host_late_max <= HOST_LATE) return true;
  fprintf(stderr, "   the host ran this test's threads up to %.1f ms late: %s not checked\n", host_late_max * 1e3, what);
  return false;
}
static int dac_pull = 1600, dac_burst = 1600;
static aaudio_result_t mmap_policy_rc = AAUDIO_OK;      // what the fake AAudio_setMMapPolicy answers
static int mmap_policy_set = -1, fake_mmap_used;        // fake_mmap_used: a mixer track that says MMAP anyway
// The fake HAL's MMAP: none (a LOW_LATENCY open gets a mixer track, as AAudio's fallback), shared only, or
// exclusive too. An MMAP stream's reader takes `mmap_burst` frames every burst period from its start on, whatever
// the stream holds (exclusive: the DSP reads on, and a short buffer is an xrun; shared: AAudio's endpoint takes
// what there is), and its timestamp presents a frame `mmap_below` frames after its read (the HAL and the DSP).
enum { HAL_NO_MMAP, HAL_MMAP_SHARED, HAL_MMAP_EXCLUSIVE };
static int fake_hal = HAL_NO_MMAP, mmap_burst = 192, mmap_capacity = 3072, mmap_below = 240;

static double elapsed(const struct timespec *t0) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (t.tv_sec - t0->tv_sec) + (t.tv_nsec - t0->tv_nsec) / 1e9; }
static int16_t mark_value(int k) { return (int16_t)(3001 + 7 * k); }

aaudio_result_t AAudio_setMMapPolicy(int32_t policy) { if (mmap_policy_rc == AAUDIO_OK) mmap_policy_set = policy; return mmap_policy_rc; }
bool AAudioStream_isMMapUsed(AAudioStream *s) { return s->mmap || fake_mmap_used; }

// The server: once filled to the buffer size and after the stall, a pull every pull period on its own clock. An
// MMAP stream's reader instead runs from the start: a burst every burst period.
static void *server(void *arg)
{
  AAudioStream *s = arg;
  double period = (double)s->pull / s->rate / (1 + dac_ppm / 1e6);
  struct timespec start;
  for (; !s->mmap;) {                                    // AudioFlinger's start threshold: the buffer filled once
    if (atomic_load(&s->quit)) return NULL;
    pthread_mutex_lock(&s->lock);
    bool full = s->written - s->read >= s->buffer;
    pthread_mutex_unlock(&s->lock);
    if (full) break;
    struct timespec nap = { 0, 200000 };
    nanosleep(&nap, NULL);
  }
  struct timespec stall = { (time_t)dac_stall_s, (long)((dac_stall_s - (time_t)dac_stall_s) * 1e9) };
  if (!s->mmap) nanosleep(&stall, NULL);
  clock_gettime(CLOCK_MONOTONIC, &start);
  for (uint64_t n = 1; !atomic_load(&s->quit); n++) {
    double due = n * period;
    struct timespec at = { start.tv_sec + (time_t)due, start.tv_nsec + (long)((due - (time_t)due) * 1e9) };
    if (at.tv_nsec >= 1000000000) { at.tv_sec++; at.tv_nsec -= 1000000000; }
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &at, NULL);
    pthread_mutex_lock(&s->lock);
    double now = now_s(), late = now - ((double)at.tv_sec + at.tv_nsec / 1e9);
    if (late > host_late_max) host_late_max = late;
    if (s->mmap) {
      // A short buffer is an xrun (the DSP plays what is there, then stale data or silence). Counted in the client's
      // frames, as AAudio's catch-up keeps them: the read never passes the write.
      int64_t have = s->written - s->read, take = have > s->pull ? s->pull : have;
      if (have < s->pull && s->written > 0) s->dac_underflows++;   // before the first write it is the stream's start
      s->read += take;
      s->pulls_done++;
      s->last_pull = now;
      for (int k = 0; k < s->next_mark; k++) if (s->taken[k] == 0 && s->at[k] >= 0 && s->at[k] < s->read) s->taken[k] = now;
    } else if (s->written - s->read >= s->pull) {
      s->read += s->pull;
      s->pulls_done++;
      s->last_pull = now;
      for (int k = 0; k < s->next_mark; k++) if (s->taken[k] == 0 && s->at[k] >= 0 && s->at[k] < s->read) s->taken[k] = now;
    } else if (s->pulls_done) s->dac_underflows++;      // not ready: nothing taken this period
    pthread_mutex_unlock(&s->lock);
  }
  return NULL;
}

aaudio_result_t AAudio_createStreamBuilder(AAudioStreamBuilder **b) { *b = calloc(1, sizeof **b); return AAUDIO_OK; }
const char *AAudio_convertResultToText(aaudio_result_t r) { return r ? "error" : "ok"; }
void AAudioStreamBuilder_setDirection(AAudioStreamBuilder *b, aaudio_direction_t v) { b->direction = v; }
void AAudioStreamBuilder_setSampleRate(AAudioStreamBuilder *b, int32_t v) { b->rate = v; }
void AAudioStreamBuilder_setChannelCount(AAudioStreamBuilder *b, int32_t v) { b->channels = v; }
void AAudioStreamBuilder_setFormat(AAudioStreamBuilder *b, aaudio_format_t v) { (void)b; (void)v; }
void AAudioStreamBuilder_setPerformanceMode(AAudioStreamBuilder *b, aaudio_performance_mode_t v) { b->perf = v; }
void AAudioStreamBuilder_setSharingMode(AAudioStreamBuilder *b, aaudio_sharing_mode_t v) { b->sharing = v; }
void AAudioStreamBuilder_setUsage(AAudioStreamBuilder *b, aaudio_usage_t v) { (void)b; (void)v; }
void AAudioStreamBuilder_setContentType(AAudioStreamBuilder *b, aaudio_content_type_t v) { (void)b; (void)v; }
void AAudioStreamBuilder_setInputPreset(AAudioStreamBuilder *b, aaudio_input_preset_t v) { (void)b; (void)v; }
aaudio_result_t AAudioStreamBuilder_delete(AAudioStreamBuilder *b) { free(b); return AAUDIO_OK; }
static AAudioStream *last_out, *last_in;
static int out_opens;
aaudio_result_t AAudioStreamBuilder_openStream(AAudioStreamBuilder *b, AAudioStream **out)
{
  AAudioStream *s = calloc(1, sizeof *s);
  s->direction = b->direction; s->rate = b->rate; s->channels = b->channels; s->perf = b->perf;
  s->burst = s->direction == AAUDIO_DIRECTION_OUTPUT ? dac_burst : 960;
  s->pull = s->direction == AAUDIO_DIRECTION_OUTPUT ? dac_pull : 960;
  s->sharing = AAUDIO_SHARING_MODE_SHARED;
  // AAudio's MMAP: only for LOW_LATENCY, only when the policy is not NEVER (AudioStreamBuilder::build), exclusive
  // only when asked and the HAL has it (else shared).
  if (s->direction == AAUDIO_DIRECTION_OUTPUT && b->perf == AAUDIO_PERFORMANCE_MODE_LOW_LATENCY && mmap_policy_set != 1 && fake_hal != HAL_NO_MMAP) {
    s->mmap = 1;
    s->burst = s->pull = mmap_burst;
    if (b->sharing == AAUDIO_SHARING_MODE_EXCLUSIVE && fake_hal == HAL_MMAP_EXCLUSIVE) s->sharing = AAUDIO_SHARING_MODE_EXCLUSIVE;
  }
  s->capacity = s->mmap ? mmap_capacity : 3 * s->burst;
  s->buffer = s->mmap ? 2 * s->burst : 3 * s->burst;
  pthread_mutex_init(&s->lock, NULL);
  *out = s;
  if (s->direction == AAUDIO_DIRECTION_OUTPUT) { last_out = s; out_opens++; } else last_in = s;
  return AAUDIO_OK;
}
aaudio_result_t AAudioStream_requestStart(AAudioStream *s)
{
  clock_gettime(CLOCK_MONOTONIC, &s->t0);
  s->started = s->direction == AAUDIO_DIRECTION_INPUT;
  if (s->direction == AAUDIO_DIRECTION_OUTPUT) s->server_on = pthread_create(&s->server, NULL, server, s) == 0;
  return AAUDIO_OK;
}
aaudio_result_t AAudioStream_requestStop(AAudioStream *s) { (void)s; return AAUDIO_OK; }
aaudio_result_t AAudioStream_close(AAudioStream *s)      // the stream is kept for the checks; its server stops
{
  atomic_store(&s->quit, true);
  if (s->server_on) { pthread_join(s->server, NULL); s->server_on = false; }
  s->closed = true;
  return AAUDIO_OK;
}
aaudio_result_t AAudioStream_write(AAudioStream *s, const void *buffer, int32_t frames, int64_t timeout_ns)
{
  // As AudioTrack's blocking write: what fits goes at once, the rest as room comes, until the timeout.
  struct timespec t0;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  const int16_t *pcm = buffer;
  int32_t done = 0;
  pthread_mutex_lock(&s->lock);
  while (done < frames) {
    int64_t room = s->buffer - (s->written - s->read);
    if (room <= 0) {
      pthread_mutex_unlock(&s->lock);
      if (elapsed(&t0) * 1e9 > timeout_ns) return done;
      struct timespec nap = { 0, 200000 };
      nanosleep(&nap, NULL);
      pthread_mutex_lock(&s->lock);
      continue;
    }
    int32_t n = frames - done < room ? frames - done : (int32_t)room;
    double now = now_s();
    for (int i = done * s->channels; i < (done + n) * s->channels; i++) {
      if (pcm[i]) s->nonzero++;
      if (s->channels == 2 && i % 2 == 0 && s->next_mark < MARKS && s->sent[s->next_mark] > 0 && pcm[i] == mark_value(s->next_mark)) {
        s->wrote[s->next_mark] = now;
        s->at[s->next_mark] = s->written + i / 2 - done;
        s->next_mark++;
      }
    }
    s->written += n;
    done += n;
  }
  pthread_mutex_unlock(&s->lock);
  return frames;
}
aaudio_result_t AAudioStream_read(AAudioStream *s, void *buffer, int32_t frames, int64_t timeout_ns)
{
  (void)timeout_ns;
  while ((int64_t)(elapsed(&s->t0) * s->rate) < s->read + frames) { struct timespec nap = { 0, 500000 }; nanosleep(&nap, NULL); }
  int16_t *pcm = buffer;
  for (int i = 0; i < frames * s->channels; i++) pcm[i] = (int16_t)(100 + (s->read + i / s->channels) % 1000);
  s->read += frames;
  return frames;
}
aaudio_result_t AAudioStream_setBufferSizeInFrames(AAudioStream *s, int32_t frames)
{
  pthread_mutex_lock(&s->lock);
  if (frames < s->burst) frames = s->burst;                // AAudio: never less than a burst
  if (frames > s->capacity) frames = s->capacity;
  if (frames > 0) s->buffer = frames;
  pthread_mutex_unlock(&s->lock);
  return s->buffer;
}
aaudio_result_t AAudioStream_getTimestamp(AAudioStream *s, clockid_t clock, int64_t *position, int64_t *time_ns)
{
  (void)clock;
  pthread_mutex_lock(&s->lock);
  bool ok = s->pulls_done > 0 && (!s->mmap || s->read >= mmap_below);
  *position = s->mmap ? s->read - mmap_below : s->read;      // MMAP: the HAL and the DSP after the read
  *time_ns = (int64_t)(s->last_pull * 1e9);
  pthread_mutex_unlock(&s->lock);
  return ok ? AAUDIO_OK : AAUDIO_ERROR_INVALID_STATE;
}
int32_t AAudioStream_getFramesPerBurst(AAudioStream *s) { return s->burst; }
int32_t AAudioStream_getBufferSizeInFrames(AAudioStream *s) { return s->buffer; }
int32_t AAudioStream_getBufferCapacityInFrames(AAudioStream *s) { return s->capacity; }
int32_t AAudioStream_getSampleRate(AAudioStream *s) { return s->rate; }
int32_t AAudioStream_getChannelCount(AAudioStream *s) { return s->channels; }
int32_t AAudioStream_getDeviceId(AAudioStream *s) { (void)s; return 3; }
int32_t AAudioStream_getSessionId(AAudioStream *s) { (void)s; return -1; }
int32_t AAudioStream_getXRunCount(AAudioStream *s) { return (int32_t)s->dac_underflows; }
aaudio_sharing_mode_t AAudioStream_getSharingMode(AAudioStream *s) { return s->sharing; }
// libaudioclient takes FAST off a blocking writer's track: what is given is NONE. An MMAP stream is LOW_LATENCY.
aaudio_performance_mode_t AAudioStream_getPerformanceMode(AAudioStream *s) { return s->mmap ? AAUDIO_PERFORMANCE_MODE_LOW_LATENCY : AAUDIO_PERFORMANCE_MODE_NONE; }
int64_t AAudioStream_getFramesWritten(AAudioStream *s) { pthread_mutex_lock(&s->lock); int64_t v = s->written; pthread_mutex_unlock(&s->lock); return v; }
// A mixer track's read moves a pull at a time; an MMAP stream's is AAudio's clock model, which moves on between the
// DSP's bursts (never past what was written).
int64_t AAudioStream_getFramesRead(AAudioStream *s)
{
  pthread_mutex_lock(&s->lock);
  int64_t v = s->read;
  if (s->mmap && s->pulls_done) {
    int64_t on = (int64_t)((now_s() - s->last_pull) * s->rate);
    v += on < 0 ? 0 : on > s->pull ? s->pull : on;
    if (v > s->written) v = s->written > s->read ? s->written : s->read;
  }
  pthread_mutex_unlock(&s->lock);
  return v;
}

// ---- the test -----------------------------------------------------------------------------------
static int guest_fd;
static void *run_helper(void *arg)
{
  char fd[16];
  snprintf(fd, sizeof fd, "%d", *(int *)arg);
  char *argv[] = { (char *)"oneios-audio", fd, NULL };
  return (void *)(intptr_t)helper_main(2, argv);
}

static void send_header(uint8_t kind, bool output, uint8_t channels, uint32_t value)
{
  struct au_header h = { .kind = kind, .output = output, .channels = channels, .value = value };
  CHECK(send(guest_fd, &h, sizeof h, MSG_NOSIGNAL) == sizeof h);
}

// The guest: SECONDS of 240-frame periods, its clock `ppm` faster than the server's. A quiet sine, or digital
// silence (iOS's engine running with nothing to play). Every `mark_every`-th period is a marker (0: none). Every
// `late_every`-th period the guest's vCPU is late by `late` periods: they come back to back after it.
typedef struct { double seconds, ppm; bool silence; unsigned mark_every, late_every, late; } guest_play;
static int marks_sent;
static void play_as(guest_play g)
{
  struct { struct au_header h; int16_t pcm[480]; } p;
  double period_ns = 5e6 / (1 + g.ppm / 1e6), phase = 0;
  struct timespec start, at;
  clock_gettime(CLOCK_MONOTONIC, &start);
  unsigned lost = 0;                                     // periods of lateness so far (the schedule catches up)
  for (unsigned n = 0; n < g.seconds * 200; n++) {
    p.h = (struct au_header){ .kind = AU_PCM, .output = 1, .channels = 2, .value = 240 };
    bool mark = g.mark_every && n % g.mark_every == g.mark_every / 2 && marks_sent < MARKS;
    for (unsigned i = 0; i < 240; i++) {
      int16_t v = mark ? mark_value(marks_sent) : g.silence ? 0 : (int16_t)(1000 * sin(phase));
      p.pcm[2 * i] = p.pcm[2 * i + 1] = v;
      phase += 2 * M_PI * 440 / 48000;
    }
    if (mark) last_out->sent[marks_sent++] = now_s();
    CHECK(send(guest_fd, &p, sizeof p, MSG_NOSIGNAL) == sizeof p);
    // Late: nothing for `late` periods, then those come back to back (the schedule is absolute).
    int64_t next = (int64_t)((n + 1) * period_ns);
    if (g.late_every && n % g.late_every == g.late_every - 1) { lost = g.late; next = (int64_t)((n + 1 + lost) * period_ns); }
    else if (lost) { lost--; continue; }
    at.tv_sec = start.tv_sec + (time_t)((start.tv_nsec + next) / 1000000000);
    at.tv_nsec = (start.tv_nsec + next) % 1000000000;
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &at, NULL);
    double late = now_s() - ((double)at.tv_sec + at.tv_nsec / 1e9);
    if (late > host_late_max) host_late_max = late;
  }
}
static void play(double seconds, double ppm) { play_as((guest_play){ .seconds = seconds, .ppm = ppm }); }
// The helper's receive loop may still have periods in its socket on a loaded host: up to 1 s for them to come in.
static void wait_in(uint64_t frames)
{
  for (int i = 0; i < 500; i++) {
    pthread_mutex_lock(&ring_lock);
    bool done = ostat.in >= frames;
    pthread_mutex_unlock(&ring_lock);
    if (done) return;
    struct timespec nap = { 0, 2000000 };
    nanosleep(&nap, NULL);
  }
}

// The stages of the markers sent from `first` on: socket + ring (sent to written), AAudio (written to read).
typedef struct { double ring_ms, aaudio_ms, total_ms, max_ms, e2e_ms, late_ms; int n, e2e_n; } stages;
static stages measure(const char *name, int first)
{
  AAudioStream *s = last_out;
  stages r = { 0 };
  double ring_max = 0, aaudio_max = 0;
  pthread_mutex_lock(&s->lock);
  for (int k = first; k < s->next_mark; k++) {
    if (s->taken[k] == 0) continue;
    double ring_ms = (s->wrote[k] - s->sent[k]) * 1e3, aaudio_ms = (s->taken[k] - s->wrote[k]) * 1e3;
    r.ring_ms += ring_ms; r.aaudio_ms += aaudio_ms; r.n++;
    if (ring_ms > ring_max) ring_max = ring_ms;
    if (aaudio_ms > aaudio_max) aaudio_max = aaudio_ms;
    if (ring_ms + aaudio_ms > r.max_ms) r.max_ms = ring_ms + aaudio_ms;
  }
  pthread_mutex_unlock(&s->lock);
  if (r.n) { r.ring_ms /= r.n; r.aaudio_ms /= r.n; }
  r.total_ms = r.ring_ms + r.aaudio_ms;
  printf("stage: %-44s %3d markers: socket+ring %5.1f ms (max %5.1f), AAudio queue %5.1f ms (max %5.1f), to the server's read %5.1f ms (max %5.1f)\n",
         name, r.n, r.ring_ms, ring_max, r.aaudio_ms, aaudio_max, r.total_ms, r.max_ms);
  return r;
}

static void speaker_case(double ppm)
{
  fprintf(stderr, "-- the guest's clock %+.0f ppm against the server's\n", ppm);
  host_late_max = 0;
  send_header(AU_START, true, 2, 48000);
  play(3, ppm);                                         // the start: the start threshold's excess comes down
  pthread_mutex_lock(&ring_lock);
  struct out_stats s0 = ostat;
  pthread_mutex_unlock(&ring_lock);
  play(6, ppm);
  wait_in(9 * 48000);
  pthread_mutex_lock(&ring_lock);
  struct out_stats s = ostat;
  unsigned fill = ring_count, aim = target;
  pthread_mutex_unlock(&ring_lock);
  AAudioStream *dac = last_out;
  int64_t queued = AAudioStream_getFramesWritten(dac) - AAudioStream_getFramesRead(dac);
  send_header(AU_STOP, true, 0, 0);
  // A loaded host sends the test's periods late and then back to back (100-240 ms on 2026-11-15 at a load of 30 on
  // 4 CPUs): the ring's overflow and its level then are the host's (origin/main's helper failed them the same way).
  bool on_time = host_late_max <= HOST_LATE;
  if (on_time) CHECK(s.overflows == 0);
  CHECK(s.in == 9 * 48000);
  CHECK(dac->nonzero > 2 * 8 * 48000);                  // the tone reached the server
  // Drift, over the last 6 s: 2000 ppm is 576 frames there, of which the level's dead band (+-5 ms, 480 frames)
  // may take all but 96. So: the net slips go the right way by at least 48 frames, and at 0 ppm stay under half
  // of 576, and the level ends within 10 ms of its aim. Not checked: the count of slips each way (the writer's late
  // wakes move the level both ways). These and "no underrun" need the host to run the test's threads on time.
  long net = ((long)s.slips_down - (long)s0.slips_down) - ((long)s.slips_up - (long)s0.slips_up);
  if (host_on_time("no underrun, the slips and the level")) {
    CHECK(s.underruns == 0);
    CHECK(dac->dac_underflows == 0);
    if (ppm > 0) CHECK(net >= 48);
    if (ppm < 0) CHECK(net <= -48);
    if (ppm == 0) CHECK(labs(net) < 288);
    CHECK(s.margin > (int)aim - (int)MS_FRAMES(10) && s.margin < (int)aim + (int)MS_FRAMES(10));
  }
  if (on_time) CHECK(FRAMES_MS((unsigned)queued + fill) < 120);   // ring + AAudio's buffer of 58 ms + a burst
}

static pthread_t helper;
static void helper_up(const char *linger)
{
  static int pair[2];
  setenv("ONEIOS_AUDIO_LINGER_S", linger, 1);
  CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, pair) == 0);
  guest_fd = pair[0];
  CHECK(pthread_create(&helper, NULL, run_helper, &pair[1]) == 0);
}
static void helper_down(void)
{
  close(guest_fd);                                       // xnuhost goes: the helper stops its streams and returns
  void *rc;
  pthread_join(helper, &rc);
  CHECK(rc == 0);
  CHECK(!out_dir.stream && !in_dir.stream);
}
static void nap_s(double s) { struct timespec t = { (time_t)s, (long)((s - (time_t)s) * 1e9) }; nanosleep(&t, NULL); }

// Latency per stage on a lingering stream: the guest starts on the helper's silence (the server's pulls known).
static stages latency_case(const char *name, int pull, int burst)
{
  fprintf(stderr, "-- latency: server pulls %d frames, burst %d\n", pull, burst);
  dac_pull = pull; dac_burst = burst; dac_ppm = 0; dac_stall_s = 0;
  helper_up("60");
  host_late_max = 0;
  send_header(AU_START, true, 2, 48000);
  play(0.5, 0);                                          // the stream opens and starts; then a pause in silence
  send_header(AU_STOP, true, 0, 0);
  nap_s(0.5);
  pthread_mutex_lock(&ring_lock);
  if (fake_hal == HAL_NO_MMAP) CHECK(pulls.size == (unsigned)pull);   // the pulls were seen, their size right
  pthread_mutex_unlock(&ring_lock);
  stages all = { 0 };
  for (int start = 0; start < 6; start++) {              // six sounds, each starting at another phase of the pulls
    int first = last_out->next_mark;
    send_header(AU_START, true, 2, 48000);
    play_as((guest_play){ .seconds = 1.0, .mark_every = 20 });
    wait_in((uint64_t)48000);
    pthread_mutex_lock(&ring_lock);                      // the helper's own end to end of this sound (its STOP resets it)
    all.e2e_ms += ostat.e2e_sum * 1e3; all.late_ms += ostat.late_sum * 1e3; all.e2e_n += (int)ostat.e2e_n;
    pthread_mutex_unlock(&ring_lock);
    send_header(AU_STOP, true, 0, 0);
    nap_s(0.31 + 0.007 * start);
    stages r = measure(name, first);
    all.ring_ms += r.ring_ms * r.n; all.aaudio_ms += r.aaudio_ms * r.n; all.n += r.n;
    if (r.max_ms > all.max_ms) all.max_ms = r.max_ms;
  }
  if (all.n) { all.ring_ms /= all.n; all.aaudio_ms /= all.n; }
  if (all.e2e_n) { all.e2e_ms /= all.e2e_n; all.late_ms /= all.e2e_n; }
  all.total_ms = all.ring_ms + all.aaudio_ms;
  printf("stage: %-44s all: socket+ring %.1f ms, AAudio queue %.1f ms, total %.1f ms (max %.1f); the helper's end-to-end %.1f ms "
         "(guest late %.1f ms; %d periods)\n", name, all.ring_ms, all.aaudio_ms, all.total_ms, all.max_ms, all.e2e_ms, all.late_ms, all.e2e_n);
  if (host_on_time("no underrun")) {
    pthread_mutex_lock(&ring_lock);
    CHECK(ostat.underruns == 0);
    pthread_mutex_unlock(&ring_lock);
    CHECK(last_out->dac_underflows == 0);
  }
  if (host_late_max <= HOST_LATE) CHECK(all.n >= 6 * 8);   // a late host loses markers to its overflows
  helper_down();
  marks_sent = 0;
  return all;
}

int main(void)
{
  setvbuf(stderr, NULL, _IOLBF, 0);
  setvbuf(stdout, NULL, _IOLBF, 0);
  setenv("ONEIOS_AUDIO_SYSTEM_SERVER", "0", 1);          // as in a session (the host's /proc is not looked at)
  // 1. Drift, one stream per case (no linger). The fake HAL has no MMAP: LOW_LATENCY (the default) asked with MMAP
  // allowed gets AAudio's own fallback, a mixer track, which is kept (one open).
  helper_up("0");
  int opens_before = out_opens;
  dac_ppm = 0; speaker_case(0);
  CHECK(mmap_policy_set == 2 && last_out->perf == AAUDIO_PERFORMANCE_MODE_LOW_LATENCY && out_opens == opens_before + 1);
  CHECK(out_path == PATH_LEGACY && !continuous && target_ms == 20);
  dac_ppm = 2000; speaker_case(-2000);                    // the server 2000 ppm fast: the guest is slow against it
  dac_ppm = -2000; speaker_case(2000);
  // The microphone: about a second of 240-frame periods comes back, in order.
  send_header(AU_START, false, 1, 48000);
  struct { struct au_header h; int16_t pcm[AU_MAX_FRAMES]; } p;
  unsigned periods = 0, frames = 0;
  int16_t first = 0;
  struct timespec t0;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  while (elapsed(&t0) < 1.0) {
    ssize_t got = recv(guest_fd, &p, sizeof p, 0);
    if (got < (ssize_t)sizeof p.h || p.h.kind != AU_PCM) continue;
    if (!periods) first = p.pcm[0];
    periods++;
    frames += p.h.value;
  }
  send_header(AU_STOP, false, 0, 0);
  CHECK(periods >= 180 && periods <= 210);
  CHECK(frames == periods * 240);
  CHECK(first == 100);
  CHECK(last_in->perf == AAUDIO_PERFORMANCE_MODE_NONE);   // the microphone stays NONE
  helper_down();

  // 2. Linger: a START soon after a STOP takes the open stream (no new open, no stall); the stream closes after
  // the silence.
  fprintf(stderr, "-- linger 1 s; the new track stalls 200 ms\n");
  dac_ppm = 0;
  dac_stall_s = 0.2;
  helper_up("1");
  host_late_max = 0;
  send_header(AU_START, true, 2, 48000);
  play(1, 0);
  wait_in(48000);
  pthread_mutex_lock(&ring_lock);
  bool kept = ostat.overflows == 0 && ostat.dropped == 0;  // the stall's audio waits in the ring, none is cut
  pthread_mutex_unlock(&ring_lock);
  if (host_on_time("nothing cut after the stall")) CHECK(kept);
  send_header(AU_STOP, true, 0, 0);
  bool tail = false;                                     // the tail plays after the STOP, nothing is cleared
  for (int i = 0; i < 100 && !tail; i++) {
    nap_s(0.02);
    pthread_mutex_lock(&ring_lock);
    tail = ostat.played + ostat.dropped == ostat.in && ring_count == 0;
    pthread_mutex_unlock(&ring_lock);
  }
  CHECK(tail);
  AAudioStream *first_stream = last_out;
  CHECK(out_dir.stream != NULL);
  host_late_max = 0;
  send_header(AU_START, true, 2, 48000);
  play(1, 0);
  wait_in(48000);
  CHECK(last_out == first_stream);
  pthread_mutex_lock(&ring_lock);
  CHECK(ostat.overflows == 0 && ostat.in == 48000);
  uint64_t linger_underruns = ostat.underruns;
  pthread_mutex_unlock(&ring_lock);
  if (host_on_time("no underrun on the lingering stream")) CHECK(linger_underruns == 0 && first_stream->dac_underflows == 0);
  send_header(AU_STOP, true, 0, 0);
  nap_s(2.6);
  CHECK(out_dir.stream == NULL);
  helper_down();

  // 3. A new track's stall: 300 ms of sound, then iOS's engine runs on with digital silence. The backlog of the
  // stall goes at the first silence (not over 16 s of faster playing): a marker 1 s later is on time.
  fprintf(stderr, "-- a new track stalls 200 ms, then the sound is followed by silence\n");
  dac_stall_s = 0.2;
  helper_up("0");
  host_late_max = 0;
  send_header(AU_START, true, 2, 48000);
  play(0.3, 0);
  play_as((guest_play){ .seconds = 1.0, .silence = true });
  int stall_first = last_out->next_mark;
  play_as((guest_play){ .seconds = 1.0, .silence = true, .mark_every = 20 });
  stages after_stall = measure("after a 200 ms stall, then 1 s of silence", stall_first);
  pthread_mutex_lock(&ring_lock);
  uint64_t silence_cut = ostat.silence, overflows = ostat.overflows;
  pthread_mutex_unlock(&ring_lock);
  send_header(AU_STOP, true, 0, 0);
  CHECK(after_stall.n >= 8);
  if (host_on_time("the stall's silence cut and the latency after it")) {
    CHECK(silence_cut >= MS_FRAMES(150));                 // most of the stall's 200 ms went as silence
    CHECK(overflows == 0);
    CHECK(after_stall.total_ms < 80);                     // on time again (268 ms before)
  }
  helper_down();
  marks_sent = 0;

  // 4. A guest whose vCPU is late by 40 ms every 0.5 s (more than the target): underruns raise the target, and it
  // comes back down to its floor (ONEIOS_AUDIO_TARGET_DECAY_S=1 here) once the guest is on time.
  fprintf(stderr, "-- the guest late by 40 ms every 0.5 s, then on time\n");
  dac_stall_s = 0;
  setenv("ONEIOS_AUDIO_TARGET_DECAY_S", "1", 1);
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(3.0, 0);                                          // the start's excess gone: the level at its aim
  play_as((guest_play){ .seconds = 2.0, .late_every = 100, .late = 8 });
  pthread_mutex_lock(&ring_lock);
  unsigned peak = ostat.target_peak;
  uint64_t late_underruns = ostat.underruns;
  pthread_mutex_unlock(&ring_lock);
  host_late_max = 0;
  play(5.0, 0);
  pthread_mutex_lock(&ring_lock);
  unsigned settled = target;
  pthread_mutex_unlock(&ring_lock);
  fprintf(stderr, "   %llu underruns while late; the target up to %u ms, %u ms after 5 s on time\n",
          (unsigned long long)late_underruns, FRAMES_MS(peak), FRAMES_MS(settled));
  CHECK(late_underruns >= 1 && peak > MS_FRAMES(20));
  if (host_on_time("the target back at its floor")) CHECK(settled == MS_FRAMES(20));
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  unsetenv("ONEIOS_AUDIO_TARGET_DECAY_S");

  // 5. Latency per stage, the guest starting at six phases of the server's pulls. The phone's shape: pulls of 1600
  // frames (33 ms) and a burst of 1600; then a server that pulls 800 (17 ms) with the same burst.
  stages phone = latency_case("pulls 1600, burst 1600 (the phone's)", 1600, 1600);
  if (host_on_time("the latency bounds")) {
    CHECK(phone.aaudio_ms < 20 + 33.3 / 2 + 8);           // the target plus half a pull, and a little
    CHECK(phone.ring_ms < 8);
  }
  stages half = latency_case("pulls 800, burst 1600", 800, 1600);
  if (host_on_time("the latency bounds")) CHECK(half.aaudio_ms < 20 + 16.7 / 2 + 8);   // the burst no longer counts: the pull does

  // 6. AAudio_setMMapPolicy fails: a mixer track LOW_LATENCY (AAudio's own fallback) is kept; a mixer track that
  // says MMAP (shared, no system_server): closed, and with MMAP not refusable opened again NONE. A track that
  // says MMAP with MMAP refused: opened again NONE.
  fprintf(stderr, "-- MMAP policy not settable; MMAP in use anyway\n");
  dac_pull = dac_burst = 1600;
  mmap_policy_rc = -1;
  int opens = out_opens;
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(0.2, 0);
  CHECK(out_opens == opens + 1 && last_out->perf == AAUDIO_PERFORMANCE_MODE_LOW_LATENCY && out_path == PATH_LEGACY);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  fake_mmap_used = 1;
  opens = out_opens;
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(0.2, 0);
  CHECK(out_opens == opens + 2 && last_out->perf == AAUDIO_PERFORMANCE_MODE_NONE);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  mmap_policy_rc = AAUDIO_OK;
  opens = out_opens;
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(0.2, 0);
  CHECK(out_opens == opens + 3 && last_out->perf == AAUDIO_PERFORMANCE_MODE_NONE && mmap_policy_set == 1);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  fake_mmap_used = 0;

  // 7. ONEIOS_AUDIO_PERF=none: NONE (the path before 2026-11-15); ONEIOS_AUDIO_MMAP=off: LOW_LATENCY, MMAP refused.
  setenv("ONEIOS_AUDIO_PERF", "none", 1);
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(0.2, 0);
  CHECK(last_out->perf == AAUDIO_PERFORMANCE_MODE_NONE && mmap_policy_set == 1);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  unsetenv("ONEIOS_AUDIO_PERF");
  setenv("ONEIOS_AUDIO_MMAP", "off", 1);
  fake_hal = HAL_MMAP_EXCLUSIVE;
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(0.2, 0);
  CHECK(last_out->perf == AAUDIO_PERFORMANCE_MODE_LOW_LATENCY && !last_out->mmap && mmap_policy_set == 1 && out_path == PATH_LEGACY);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  unsetenv("ONEIOS_AUDIO_MMAP");

  // 8. MMAP exclusive (the HAL has it): the DSP reads a burst of 192 every 4 ms from the start, and presents a
  // frame 5 ms after its read. One open, the level 5 ms above a chunk, no xrun, the drift steered.
  fprintf(stderr, "-- MMAP exclusive: burst %d, %d frames below the read\n", mmap_burst, mmap_below);
  fake_hal = HAL_MMAP_EXCLUSIVE;
  helper_up("0");
  opens = out_opens;
  dac_ppm = 0; speaker_case(0);
  CHECK(out_opens == opens + 1 && last_out->mmap && last_out->sharing == AAUDIO_SHARING_MODE_EXCLUSIVE && mmap_policy_set == 2);
  CHECK(out_path == PATH_MMAP_EXCLUSIVE && continuous && target_ms == 5);
  dac_ppm = 2000; speaker_case(-2000);
  dac_ppm = -2000; speaker_case(2000);
  dac_ppm = 0;
  helper_down();
  stages mmap_x = latency_case("MMAP exclusive, burst 192", 1600, 1600);
  if (host_on_time("the MMAP latency bounds")) {
    CHECK(mmap_x.aaudio_ms < 10 + 2.5 + 4 + 3);         // the start's target (10 ms), half a chunk, a burst, a little
    CHECK(mmap_x.ring_ms < 3);
    CHECK(mmap_x.e2e_n > 100 && mmap_x.e2e_ms > 5 && mmap_x.e2e_ms < 5 + 10 + 4 + 6);  // below the read 5 ms + the level + lateness
  }

  // 9. MMAP shared offered: without system_server closed before its start, a mixer track instead; with it kept.
  fprintf(stderr, "-- MMAP shared, without and with system_server\n");
  fake_hal = HAL_MMAP_SHARED;
  opens = out_opens;
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(0.3, 0);
  CHECK(out_opens == opens + 2 && !last_out->mmap && out_path == PATH_LEGACY && mmap_policy_set == 1);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  setenv("ONEIOS_AUDIO_SYSTEM_SERVER", "1", 1);
  opens = out_opens;
  helper_up("0");
  host_late_max = 0;
  send_header(AU_START, true, 2, 48000);
  play(2.0, 0);
  wait_in(2 * 48000);
  CHECK(out_opens == opens + 1 && last_out->mmap && last_out->sharing == AAUDIO_SHARING_MODE_SHARED && out_path == PATH_MMAP_SHARED);
  pthread_mutex_lock(&ring_lock);
  uint64_t shared_underruns = ostat.underruns;
  pthread_mutex_unlock(&ring_lock);
  if (host_on_time("no underrun on MMAP shared")) CHECK(shared_underruns == 0 && last_out->dac_underflows == 0);
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  setenv("ONEIOS_AUDIO_SYSTEM_SERVER", "0", 1);

  // 10. The probe (xnuhost before the boot): the speaker opened in silence, its latency answered (below the read
  // plus the target: 240 + 240 frames here), the stream kept for the guest's first START (no second open); the
  // output latency iOS is told comes back in the end-to-end line.
  fprintf(stderr, "-- the probe\n");
  fake_hal = HAL_MMAP_EXCLUSIVE;
  helper_up("60");
  opens = out_opens;
  host_late_max = 0;
  send_header(AU_PROBE, true, 0, 0);
  struct au_header answer = { 0 };
  for (clock_gettime(CLOCK_MONOTONIC, &t0); elapsed(&t0) < 3.0 && answer.kind != AU_LATENCY;) {
    struct pollfd wait = { .fd = guest_fd, .events = POLLIN };
    if (poll(&wait, 1, 100) > 0 && recv(guest_fd, &answer, sizeof answer, 0) < (ssize_t)sizeof answer) answer.kind = 0;
  }
  fprintf(stderr, "   probe: %u frames\n", answer.value);
  // A late host makes the silence's writes late, AAudio's xruns raise the target, and the probe says so (+5 ms each).
  CHECK(answer.kind == AU_LATENCY && answer.value >= 400 && answer.value <= 240 + MS_FRAMES(80) && out_opens == opens + 1);
  pthread_mutex_lock(&ring_lock);
  unsigned probed_target = target;
  pthread_mutex_unlock(&ring_lock);
  CHECK(labs((long)answer.value - (long)(240 + probed_target)) <= 96);   // below the read + the level, as the helper holds it
  send_header(AU_TOLD, true, 0, answer.value);
  send_header(AU_START, true, 2, 48000);
  play(1.0, 0);
  wait_in(48000);
  CHECK(out_opens == opens + 1 && told_frames == answer.value);
  send_header(AU_STOP, true, 0, 0);
  helper_down();

  // 11. MMAP exclusive, the guest late by 40 ms every 0.5 s: underruns (the level's floor of 5 ms is far below)
  // raise the target; on time again it comes back to 5 ms.
  fprintf(stderr, "-- MMAP exclusive: the guest late by 40 ms every 0.5 s, then on time\n");
  setenv("ONEIOS_AUDIO_TARGET_DECAY_S", "1", 1);
  helper_up("0");
  send_header(AU_START, true, 2, 48000);
  play(1.0, 0);
  play_as((guest_play){ .seconds = 2.0, .late_every = 100, .late = 8 });
  pthread_mutex_lock(&ring_lock);
  peak = ostat.target_peak;
  late_underruns = ostat.underruns;
  pthread_mutex_unlock(&ring_lock);
  host_late_max = 0;
  play(5.0, 0);
  pthread_mutex_lock(&ring_lock);
  settled = target;
  pthread_mutex_unlock(&ring_lock);
  fprintf(stderr, "   %llu underruns while late; the target up to %u ms, %u ms after 5 s on time\n",
          (unsigned long long)late_underruns, FRAMES_MS(peak), FRAMES_MS(settled));
  CHECK(late_underruns >= 1 && peak > MS_FRAMES(10));     // the stream starts at 10 ms; the late guest raised it
  // On time again it comes down as far as the guest's lateness allows (5 ms needs periods within 2 ms).
  if (host_on_time("the MMAP target back down")) {
    CHECK(settled < peak && settled <= MS_FRAMES(10));
    if (host_late_max < 0.0015) CHECK(settled == MS_FRAMES(5));
  }
  send_header(AU_STOP, true, 0, 0);
  helper_down();
  unsetenv("ONEIOS_AUDIO_TARGET_DECAY_S");
  fake_hal = HAL_NO_MMAP;

  printf("test_audio_helper: %d checks, %d failed\n", checks, failures);
  return failures != 0;
}
