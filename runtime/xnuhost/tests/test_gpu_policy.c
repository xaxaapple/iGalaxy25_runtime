// SPDX-License-Identifier: MIT
// The GPU's per-process policy (drm2kgsl/oneios_gpu_policy.h) on any host, no KGSL: the classes from names, the guest's
// hint and whether the render server reads a context's surfaces, their KGSL priorities, the surface table, and the level
// for frame streams of the kinds the phone draws (docs/gpu.md 2.2b). Then the same streams under the rule before
// 2026-11-15 (gpu-level=hold: every render-server frame at the top, to 150 ms after it) and under the deadline rule, side
// by side: the share of time the clock is held at the top and the frames over budget. Last, a model GPU with a widget-like
// client beside other work, under #42's rules and now: how long each client's fences take (fix-gpu-widgets).
#include "oneios_gpu_policy.h"
#include <stdio.h>
#include <stdlib.h>

static unsigned checks, failed;
#define CHECK(x) do { checks++; if (!(x)) { failed++; printf("FAILED %s:%d: %s\n", __FILE__, __LINE__, #x); } } while (0)

static enum og_class cls(const char *name, uint8_t hint, uint8_t qos) { return og_class_of(name, strlen(name), hint, qos, false, NULL, NULL); }
static enum og_class shown(const char *name, uint8_t hint, uint8_t qos) { return og_class_of(name, strlen(name), hint, qos, true, NULL, NULL); }

static void classes(void)
{
  // The render server by name, whatever its role says.
  CHECK(cls("xnu:backboardd", 0, 0) == OG_CLASS_RENDER);
  CHECK(cls("xnu:backboardd", OG_HINT_VALID | OG_ROLE_BACKGROUND, OG_QOS_BACKGROUND) == OG_CLASS_RENDER);
  CHECK(cls("backboardd", 0, 0) == OG_CLASS_RENDER);
  // The name list holds only daemons that never draw what the user sees (p_name: at most 32 characters).
  CHECK(cls("xnu:photoanalysisd", 0, 0) == OG_CLASS_BACKGROUND && cls("xnu:mediaanalysisd", 0, 0) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:assetsd", 0, 0) == OG_CLASS_BACKGROUND && cls("xnu:searchd", 0, 0) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:analyticsd", 0, 0) == OG_CLASS_BACKGROUND && cls("xnu:suggestd", 0, 0) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:photoanalysisd", OG_HINT_VALID | OG_ROLE_FOREGROUND, OG_QOS_USER_INTERACTIVE) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:assetsdx", 0, 0) == OG_CLASS_DEFAULT);       // a name is the whole name
  CHECK(cls("xnu:SpringBoard", 0, 0) == OG_CLASS_FRONT);
  // What the user sees is no longer on it (until fix-gpu-widgets: WidgetRenderer*, *Widget*, *Poster*, iconservices*,
  // *Thumbnail*): such a process is classed by the guest's role, like any other.
  CHECK(cls("xnu:WidgetRenderer_Default", 0, 0) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:PosterBoard", OG_HINT_VALID | OG_ROLE_FOREGROUND, OG_QOS_USER_INTERACTIVE) == OG_CLASS_FRONT);
  CHECK(cls("xnu:MercuryPosterExtension", 0, 0) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:PhotosReliveWidget", 0, 0) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:iconservicesagent", 0, 0) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:QuickLookThumbnailExtension", 0, 0) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:chronod", OG_HINT_VALID | OG_ROLE_DEFAULT, OG_QOS_USER_INITIATED) == OG_CLASS_DEFAULT);
  // A widget renderer that RunningBoard calls non-UI, or whose thread runs at utility QoS, is background by its role...
  CHECK(cls("xnu:WidgetRenderer_Default", OG_HINT_VALID | OG_ROLE_NONUI, OG_QOS_USER_INITIATED) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:WidgetRenderer_Default", OG_HINT_VALID | OG_ROLE_UNSPECIFIED, OG_QOS_UTILITY) == OG_CLASS_BACKGROUND);
  // ... until the render server reads one of its surfaces: then never background, whatever says so.
  CHECK(shown("xnu:WidgetRenderer_Default", OG_HINT_VALID | OG_ROLE_NONUI, OG_QOS_USER_INITIATED) == OG_CLASS_DEFAULT);
  CHECK(shown("xnu:WidgetRenderer_Default", OG_HINT_VALID | OG_ROLE_UNSPECIFIED, OG_QOS_UTILITY) == OG_CLASS_DEFAULT);
  CHECK(shown("xnu:Preferences", OG_HINT_VALID | OG_ROLE_FOREGROUND | OG_HINT_DARWINBG, 0) == OG_CLASS_DEFAULT);
  CHECK(shown("xnu:photoanalysisd", 0, 0) == OG_CLASS_DEFAULT);
  CHECK(shown("xnu:MobileSafari", OG_HINT_VALID | OG_ROLE_FOREGROUND, OG_QOS_USER_INTERACTIVE) == OG_CLASS_FRONT);
  CHECK(shown("xnu:backboardd", 0, 0) == OG_CLASS_RENDER);
  // No name rule: the guest's role, then the thread's QoS.
  CHECK(cls("xnu:MobileSafari", 0, 0) == OG_CLASS_DEFAULT);                                       // an older guest
  CHECK(cls("xnu:MobileSafari", OG_HINT_VALID | OG_ROLE_FOREGROUND, OG_QOS_USER_INTERACTIVE) == OG_CLASS_FRONT);
  CHECK(cls("xnu:MobileSafari", OG_HINT_VALID | OG_ROLE_BACKGROUND, OG_QOS_USER_INTERACTIVE) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:Preferences", OG_HINT_VALID | OG_ROLE_FOREGROUND | OG_HINT_DARWINBG, 0) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:mediaserverd", OG_HINT_VALID | OG_ROLE_UNSPECIFIED, OG_QOS_UTILITY) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:mediaserverd", OG_HINT_VALID | OG_ROLE_UNSPECIFIED, OG_QOS_USER_INITIATED) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:mediaserverd", OG_HINT_VALID | OG_ROLE_DEFAULT, OG_QOS_UNSPECIFIED) == OG_CLASS_DEFAULT);
  CHECK(cls("xnu:Extension", OG_HINT_VALID | OG_ROLE_NONUI, OG_QOS_USER_INITIATED) == OG_CLASS_BACKGROUND);
  CHECK(cls("xnu:x", OG_HINT_VALID | OG_ROLE_GRAPHICS_SERVER, 0) == OG_CLASS_FRONT);
  // Lists of one's own replace the defaults.
  CHECK(og_class_of("xnu:Foo", 7, 0, 0, false, "Foo,Bar*", "") == OG_CLASS_BACKGROUND);
  CHECK(og_class_of("xnu:Barn", 8, 0, 0, false, "Foo,Bar*", "") == OG_CLASS_BACKGROUND);
  CHECK(og_class_of("xnu:Barn", 8, 0, 0, true, "Foo,Bar*", "") == OG_CLASS_DEFAULT);
  CHECK(og_class_of("xnu:photoanalysisd", 18, 0, 0, false, "Foo", "") == OG_CLASS_DEFAULT);
  CHECK(og_class_of("xnu:SpringBoard", 15, 0, 0, false, "", "") == OG_CLASS_DEFAULT);
  CHECK(og_name_listed("abc", 3, "*b*") && og_name_listed("abc", 3, "*c") && !og_name_listed("abc", 3, "*d*") && !og_name_listed("ab", 2, "abc*"));
  CHECK(!og_name_listed("abc", 3, "") && !og_name_listed("abc", 3, ",,") && og_name_listed("abc", 3, ",abc,"));
  // Priorities: the render server and the front app above the default; background with the default, never under it (KGSL
  // runs a lower ringbuffer only while every higher one is empty); never KGSL's first ringbuffer.
  CHECK(og_kgsl_priority(OG_CLASS_RENDER) == 4 && og_kgsl_priority(OG_CLASS_FRONT) == 4);
  CHECK(og_kgsl_priority(OG_CLASS_DEFAULT) == 8 && og_kgsl_priority(OG_CLASS_BACKGROUND) == 8);
  for (unsigned c = 0; c < 4; c++) CHECK(og_kgsl_priority((enum og_class)c) >= 4 && og_kgsl_priority((enum og_class)c) <= 11);
}

// Whose surfaces another process reads: blobs over the same first guest page, in either order; one context's own blobs are
// not another's; the table keeps the newest entries when it fills.
static void pages(void)
{
  static struct og_pages t;
  memset(&t, 0, sizeof t);
  CHECK(og_pages_note(&t, 0x880004000ull, 10, 3).res == 0);             // the widget draws into a surface
  CHECK(og_pages_note(&t, 0x880004000ull, 11, 3).res == 0);             // its own second blob over it
  struct og_page o = og_pages_note(&t, 0x880004000ull, 20, 1);          // the render server imports it
  CHECK(o.res == 11 && o.ctx == 3);
  CHECK(og_pages_note(&t, 0x900000000ull, 21, 1).res == 0);             // the render server's own first
  o = og_pages_note(&t, 0x900000000ull, 30, 5);                         // then a client over the same pages
  CHECK(o.res == 21 && o.ctx == 1);
  CHECK(og_pages_note(&t, 0, 40, 5).res == 0 && og_pages_note(&t, 0x1000, 0, 5).res == 0);
  // Three times the table's size of blobs: the newest 512 are all still known.
  for (uint32_t i = 0; i < 3 * OG_PAGES; i++) og_pages_note(&t, 0x1000000000ull + (uint64_t)i * 0x4000, 1000 + i, 7);
  unsigned found = 0;
  for (uint32_t i = 3 * OG_PAGES - 512; i < 3 * OG_PAGES; i++)
    found += og_pages_note(&t, 0x1000000000ull + (uint64_t)i * 0x4000, 90000 + i, 8).res == 1000 + i;
  CHECK(found == 512);
}

#define MS 1000000ull
#define US 1000ull

// One frame: start, then done after `gpu_us`. Returns the action.
static enum og_action frame(struct og_policy *p, enum og_class c, uint64_t at, uint32_t deadline_us, uint32_t gpu_us)
{
  struct og_frame f;
  enum og_action a = og_frame_start(p, c, at, deadline_us, &f);
  og_frame_done(p, &f, gpu_us);
  return a;
}

static void levels(void)
{
  struct og_policy p;
  // A first frame (nothing known): the top. Easy frames at 120 Hz (0.3 ms at the top, 1.2 at the bottom): after 8 the governor.
  og_policy_init(&p, OG_MODE_DEADLINE);
  CHECK(frame(&p, OG_CLASS_RENDER, 1000 * MS, 0, 300) == OG_ACTION_TOP);
  uint64_t t = 1000 * MS;
  unsigned tops = 1;
  enum og_action a = OG_ACTION_NONE;
  for (unsigned i = 0; i < 7; i++) { t += 8333 * US; a = frame(&p, OG_CLASS_RENDER, t, 0, 300); tops += a == OG_ACTION_TOP; }
  CHECK(tops == 8 && p.level == OG_LEVEL_GOVERNOR);
  t += 8333 * US;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 1200) == OG_ACTION_GOVERNOR);       // the hold stops at once
  t += 8333 * US;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 1200) == OG_ACTION_NONE && p.level == OG_LEVEL_GOVERNOR);
  // A frame that nearly misses at the governor's clock (7 ms of 8.3, more than 80 %): the next ones at the top, and longer.
  t += 8333 * US;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 7000) == OG_ACTION_NONE && p.level == OG_LEVEL_TOP && p.misses == 1);
  t += 8333 * US;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 300) == OG_ACTION_TOP);
  CHECK(p.down_need == 32);
  for (unsigned i = 0; i < 31; i++) { t += 8333 * US; frame(&p, OG_CLASS_RENDER, t, 0, 300); }
  CHECK(p.level == OG_LEVEL_GOVERNOR);

  // Heavy frames at 120 Hz (3 ms at the top: 12 at the bottom, over 6.7): the top, for as long as they come.
  og_policy_init(&p, OG_MODE_DEADLINE);
  t = 5000 * MS;
  for (unsigned i = 0; i < 200; i++) { t += 8333 * US; frame(&p, OG_CLASS_RENDER, t, 0, 3000); }
  CHECK(p.level == OG_LEVEL_TOP && p.frames_top == 200 && p.late == 0);
  // The same frames at 60 Hz fit at the bottom (12 ms of 16.7): the governor's after 8.
  og_policy_init(&p, OG_MODE_DEADLINE);
  for (unsigned i = 0; i < 100; i++) { t += 16667 * US; frame(&p, OG_CLASS_RENDER, t, 0, p.level == OG_LEVEL_TOP ? 3000 : 12000); }
  CHECK(p.level == OG_LEVEL_GOVERNOR && p.frames_top < 12 && p.misses == 0 && p.interval_us > 16000 && p.interval_us < 17000);

  // After a pause the governor is at its bottom: a frame known to be easy there stays with it (a clock tick), a heavy one not.
  og_policy_init(&p, OG_MODE_DEADLINE);
  p.cold_us = 1000;
  t = 9000 * MS;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 1000) == OG_ACTION_NONE && p.level == OG_LEVEL_GOVERNOR);
  t += 1000 * MS;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 1000) == OG_ACTION_NONE);
  p.cold_us = 20000;
  t += 1000 * MS;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 3000) == OG_ACTION_TOP);
  // Unknown at the bottom but known at the top: x4 (KGSL's clock ratio) decides.
  og_policy_init(&p, OG_MODE_DEADLINE);
  p.top_us = 400;                                                            // 1.6 ms at the bottom: fits
  CHECK(frame(&p, OG_CLASS_RENDER, 20000 * MS, 0, 1600) == OG_ACTION_NONE);
  og_policy_init(&p, OG_MODE_DEADLINE);
  p.top_us = 2500;                                                           // 10 ms at the bottom: misses
  CHECK(frame(&p, OG_CLASS_RENDER, 20000 * MS, 0, 2500) == OG_ACTION_TOP);
  og_policy_init(&p, OG_MODE_DEADLINE);
  p.ratio_x16 = 16 * 903 / 231;                                              // KGSL's own list (903 and 231 MHz)
  p.top_us = 2000;
  CHECK(p.ratio_x16 == 62 && og_at_bottom(&p, 2000) == 7750);
  CHECK(frame(&p, OG_CLASS_RENDER, 20000 * MS, 0, 2000) == OG_ACTION_TOP);

  // The guest's deadline (the work interval's, from the CPU agent's signal) is the budget when it comes.
  og_policy_init(&p, OG_MODE_DEADLINE);
  p.cold_us = 12000;
  CHECK(frame(&p, OG_CLASS_RENDER, 30000 * MS, 30000, 12000) == OG_ACTION_NONE);   // 12 ms fits 30
  CHECK(frame(&p, OG_CLASS_RENDER, 31000 * MS, 8000, 12000) == OG_ACTION_TOP);     // not 8
  struct og_frame f;
  og_frame_start(&p, OG_CLASS_RENDER, 32000 * MS, 200, &f);
  CHECK(f.budget_us == 1000);                                                      // at least 1 ms
  og_frame_start(&p, OG_CLASS_RENDER, 32000 * MS + 10 * MS, 0, &f);
  CHECK(f.budget_us == 10000);                                                     // the cadence: 10 ms apart

  // Background contexts never count and never take the top; the lowest level for them once the screen is still 500 ms;
  // the first render-server frame after that gets the top (KGSL keeps the lowest until it expires otherwise).
  og_policy_init(&p, OG_MODE_DEADLINE);
  p.cold_us = 500;
  t = 40000 * MS;
  CHECK(frame(&p, OG_CLASS_RENDER, t, 0, 500) == OG_ACTION_NONE);
  CHECK(!og_background_min(&p, t + 100 * MS));
  CHECK(frame(&p, OG_CLASS_BACKGROUND, t + 100 * MS, 0, 50000) == OG_ACTION_NONE && p.frames == 1 && p.misses == 0);
  CHECK(og_background_min(&p, t + 600 * MS));
  CHECK(frame(&p, OG_CLASS_RENDER, t + 700 * MS, 0, 500) == OG_ACTION_TOP);
  CHECK(frame(&p, OG_CLASS_RENDER, t + 1500 * MS, 0, 500) == OG_ACTION_GOVERNOR);  // no background at the lowest since
  // Other contexts count within a second of the render server's last frame, and only then.
  og_policy_init(&p, OG_MODE_DEADLINE);
  CHECK(frame(&p, OG_CLASS_DEFAULT, 50000 * MS, 0, 30000) == OG_ACTION_NONE && p.frames == 0);
  frame(&p, OG_CLASS_RENDER, 50001 * MS, 0, 300);
  CHECK(frame(&p, OG_CLASS_FRONT, 50300 * MS, 0, 300) == OG_ACTION_GOVERNOR && p.frames == 2);   // easy: the hold goes
  CHECK(frame(&p, OG_CLASS_FRONT, 52000 * MS, 0, 300) == OG_ACTION_NONE && p.frames == 2);
  // The other modes.
  og_policy_init(&p, OG_MODE_HOLD);
  for (unsigned i = 0; i < 20; i++) CHECK(frame(&p, OG_CLASS_RENDER, 60000 * MS + i * 8333 * US, 0, 100) == OG_ACTION_TOP);
  CHECK(frame(&p, OG_CLASS_BACKGROUND, 61000 * MS, 0, 100) == OG_ACTION_NONE);
  og_policy_init(&p, OG_MODE_GOVERNOR);
  for (unsigned i = 0; i < 20; i++) CHECK(frame(&p, OG_CLASS_RENDER, 60000 * MS + i * 8333 * US, 0, 9000) == OG_ACTION_NONE);
}

// ---- before and after: the same streams under gpu-level=hold (0005/0006) and =deadline -------------------------------
// A stream: render-server frames at `hz` for `seconds` in bursts of `burst_ms` every `every_ms` (0: one burst), each frame
// `top_us` of GPU time at the top level and `ratio` times that at the governor's (its bottom, as after a pause).
typedef struct { const char *what; double hz, seconds, burst_ms, every_ms; uint32_t top_us; double ratio; } stream;

static void run(const stream *s, enum og_mode mode, double *held_pct, unsigned *late, unsigned *frames)
{
  struct og_policy p;
  og_policy_init(&p, mode);
  const uint64_t hold_ns = 150 * MS, period = (uint64_t)(1e9 / s->hz), end = (uint64_t)(s->seconds * 1e9), start0 = 100000 * MS;
  uint64_t held = 0, hold_until = 0;
  *late = *frames = 0;
  for (uint64_t at = 0; at < end; at += period) {
    if (s->every_ms > 0 && (double)(at % (uint64_t)(s->every_ms * 1e6)) >= s->burst_ms * 1e6) continue;
    const uint64_t now = start0 + at;
    struct og_frame f;
    enum og_action a = og_frame_start(&p, OG_CLASS_RENDER, now, 0, &f);
    if (a == OG_ACTION_GOVERNOR && hold_until > now) hold_until = now;
    if (a == OG_ACTION_TOP) {
      const uint64_t from = hold_until > now ? hold_until : now;
      held += now + hold_ns - from;
      hold_until = now + hold_ns;
    }
    // The frame runs at the top while the clock is held (this frame's or an earlier one's), else at the governor's bottom.
    const bool top = hold_until > now;
    const uint32_t gpu = top ? s->top_us : (uint32_t)(s->top_us * s->ratio);
    if (gpu > (period < 16667 * US ? period / 1000 : 16667)) (*late)++;
    (*frames)++;
    og_frame_done(&p, &(struct og_frame){ .level = top ? OG_LEVEL_TOP : OG_LEVEL_GOVERNOR, .cold = f.cold, .budget_us = f.budget_us }, gpu);
  }
  if (hold_until > start0 + end) held -= hold_until - (start0 + end);
  *held_pct = 100.0 * (double)held / (double)end;
}

static void before_after(void)
{
  static const stream streams[] = {
    { "still lock screen redrawing (45 frames/s, 0.4 ms each)", 45, 10, 0, 0, 400, 4 },
    { "clock tick (1 frame/s, 0.3 ms)", 1, 10, 1, 1000, 300, 4 },
    { "typing (a key every 150 ms: 3 frames at 120 Hz, 1 ms)", 120, 10, 25, 150, 1000, 4 },
    { "scrolling (120 frames/s, 3 ms each)", 120, 10, 0, 0, 3000, 4 },
    { "a light animation (60 frames/s, 0.8 ms)", 60, 10, 0, 0, 800, 4 },
  };
  printf("%-58s %22s %22s\n", "stream (10 s)", "hold: top / late", "deadline: top / late");
  for (unsigned i = 0; i < sizeof streams / sizeof streams[0]; i++) {
    double held[2];
    unsigned late[2], frames[2];
    run(&streams[i], OG_MODE_HOLD, &held[0], &late[0], &frames[0]);
    run(&streams[i], OG_MODE_DEADLINE, &held[1], &late[1], &frames[1]);
    printf("%-58s %10.1f %% %4u/%-5u %10.1f %% %4u/%-5u\n", streams[i].what, held[0], late[0], frames[0], held[1], late[1], frames[1]);
    CHECK(held[1] <= held[0] + 0.01);
    // Heavy frames keep the top; nothing is later than before by more than the one frame that teaches the policy.
    if (streams[i].top_us * streams[i].ratio > 8333 * 0.8 && streams[i].hz >= 100 && streams[i].every_ms == 0) CHECK(held[1] > 95);
    CHECK(late[1] <= late[0] + 1 + frames[1] / 40);
  }
}


// The lowest level's rules (og_lowest_for) and the top that ends a lowest level when other work comes (og_frame_start).
static void lowest(void)
{
  struct og_policy p;
  og_policy_init(&p, OG_MODE_DEADLINE);
  const uint64_t t = 70000 * MS;
  frame(&p, OG_CLASS_RENDER, t, 0, 500);
  CHECK(!og_lowest_for(&p, t + 100 * MS, 0, false, 0) && p.lowest_refused == 0);      // the screen is not still
  CHECK(!og_lowest_for(&p, t + 600 * MS, 1, false, 0) && p.lowest_refused == 1);      // another context's work is queued
  CHECK(!og_lowest_for(&p, t + 600 * MS, 0, true, 0) && p.lowest_refused == 2);       // another process reads its surfaces
  CHECK(!og_lowest_for(&p, t + 600 * MS, 0, false, t + 540 * MS) && p.lowest_refused == 3);   // its own fence waited 60 ms
  CHECK(p.last_bg_min_ns == 0);
  CHECK(og_lowest_for(&p, t + 600 * MS, 0, false, t + 590 * MS));                     // 10 ms: still young
  CHECK(og_lowest_for(&p, t + 650 * MS, 0, false, 0) && p.last_bg_min_ns == t + 650 * MS);
  // A widget's drawing at that still screen (a context that counts no frame: the render server drew 1.7 s ago): the top
  // ends the lowest, once; then nothing until the next lowest.
  CHECK(og_lowest_for(&p, t + 1700 * MS, 0, false, 0));
  CHECK(frame(&p, OG_CLASS_DEFAULT, t + 1750 * MS, 0, 1500) == OG_ACTION_TOP && p.lowest_undone == 1 && p.frames == 1);
  CHECK(frame(&p, OG_CLASS_DEFAULT, t + 1760 * MS, 0, 1500) == OG_ACTION_NONE && p.lowest_undone == 1);
  CHECK(frame(&p, OG_CLASS_BACKGROUND, t + 1770 * MS, 0, 1500) == OG_ACTION_NONE);
  CHECK(og_lowest_for(&p, t + 1800 * MS, 0, false, 0));
  CHECK(frame(&p, OG_CLASS_DEFAULT, t + 2100 * MS, 0, 1500) == OG_ACTION_NONE && p.lowest_undone == 1);   // 300 ms on: expired
  // Not in the other modes.
  og_policy_init(&p, OG_MODE_HOLD);
  CHECK(!og_lowest_for(&p, t, 0, false, 0));
  og_policy_init(&p, OG_MODE_GOVERNOR);
  CHECK(!og_lowest_for(&p, t, 0, false, 0));
}

// ---- starvation: a widget-like client beside other work, under #42's rules and now ---------------------------------
// A GPU in 50 us ticks, with KGSL's rules as this model takes them (assumptions, not measurements of the phone):
//  - ringbuffers by strict priority (KGSL priority / 4); one with work preempts a lower one at the next tick;
//  - within a ringbuffer a job runs to its end, and the contexts take turns, up to 5 jobs each (the dispatcher's burst);
//  - the clock: the top while a PWR_MAX is in force (to 150 ms after the frame that asked: the hold), else the lowest (a
//    job takes 4 times its top time) while a PWR_MIN is (to 80 ms after the background submission that set or renewed it;
//    a PWR_MAX ends it), else the governor's (2 times);
//  - a job's size is its GPU time at the top; each job is one fence.
// Under #42's rules the widget renderer is background by name (KGSL priority 12) and a background context's submission
// carries the lowest level whenever the screen is still (og_background_min); now its class comes from the policy (the
// render server reads its surfaces: `shown`) and the lowest from og_lowest_for.
enum { M_JOBS = 64, M_TICK_NS = 50000, M_CTX = 5 };
typedef struct { uint64_t submit_ns, left_ns; struct og_frame frame; } mjob;
typedef struct {
  const char *name;
  uint8_t hint, qos;
  bool shown, old_background;      // the render server reads its surfaces; #42's name list had it
  uint32_t every_ms, at_ms, jobs, apart_us, size_us;   // bursts: `jobs` of `size_us`, `apart_us` apart, every `every_ms` at `at_ms`
  bool loop;                       // instead: one job at a time, the next 1 ms after the last is done (a compute loop)
  int after;                       // instead (1 + a client's index): a burst 2 ms after that client's burst is done
  // the run's state
  enum og_class cls;
  unsigned ring, sent;
  uint64_t burst_ns, next_ns, last_done_ns;
  mjob q[M_JOBS];
  unsigned head, n;
  uint64_t fences, sum_ns, max_ns, over_slice, over_bound;
} mctx;

static void m_push(struct og_policy *p, mctx *all, unsigned nall, mctx *c, uint64_t now, bool old, uint64_t *t_max, uint64_t *t_min)
{
  if (c->n == M_JOBS) return;
  mjob *j = &c->q[(c->head + c->n++) % M_JOBS];
  *j = (mjob){ .submit_ns = now, .left_ns = (uint64_t)c->size_us * 1000 };
  const uint64_t undone = p->lowest_undone, bg_min = p->last_bg_min_ns;
  const enum og_level told = p->told;
  enum og_action a = og_frame_start(p, c->cls, now, 0, &j->frame);
  if (old && p->lowest_undone != undone) { p->lowest_undone = undone; p->last_bg_min_ns = bg_min; p->told = told; a = OG_ACTION_NONE; }
  if (a == OG_ACTION_TOP) { *t_max = now + 150 * MS; *t_min = 0; }
  if (a == OG_ACTION_GOVERNOR && *t_max > now) *t_max = now;
  if (c->cls != OG_CLASS_BACKGROUND) return;
  uint32_t others = 0;
  uint64_t oldest = 0;
  for (unsigned i = 0; i < nall; i++) if (all[i].cls != OG_CLASS_BACKGROUND) others += all[i].n;
  for (unsigned i = 0; i + 1 < c->n; i++) if (!oldest || c->q[(c->head + i) % M_JOBS].submit_ns < oldest) oldest = c->q[(c->head + i) % M_JOBS].submit_ns;
  const bool low = old ? og_background_min(p, now) : og_lowest_for(p, now, others, c->shown, oldest);
  if (low && *t_max <= now) *t_min = now + 80 * MS;                 // KGSL: a slower constraint never replaces a faster one
}


static void m_run(mctx *m, unsigned nm, bool old, double seconds, uint64_t *lowest_ns)
{
  struct og_policy p;
  og_policy_init(&p, OG_MODE_DEADLINE);
  const uint64_t start = 200000 * MS, end = start + (uint64_t)(seconds * 1e9);
  uint64_t t_max = 0, t_min = 0;
  int cur[4] = { -1, -1, -1, -1 };
  unsigned burst[4] = { 0 };
  bool busy[4] = { false };
  *lowest_ns = 0;
  for (unsigned i = 0; i < nm; i++) {
    mctx *c = &m[i];
    c->cls = old && c->old_background ? OG_CLASS_BACKGROUND : og_class_of(c->name, strlen(c->name), c->hint, c->qos, !old && c->shown, NULL, NULL);
    const unsigned prio = old ? (c->cls == OG_CLASS_RENDER || c->cls == OG_CLASS_FRONT ? 4 : c->cls == OG_CLASS_BACKGROUND ? 12 : 8) : og_kgsl_priority(c->cls);
    c->ring = prio / 4;
    c->sent = c->head = c->n = 0;
    if (c->after) c->sent = c->jobs;                 // armed by its client
    c->burst_ns = c->next_ns = start + (uint64_t)c->at_ms * MS;
    c->last_done_ns = 0;
    c->fences = c->sum_ns = c->max_ns = c->over_slice = c->over_bound = 0;
  }
  for (uint64_t now = start; now < end; now += M_TICK_NS) {
    // The clients' submissions.
    for (unsigned i = 0; i < nm; i++) {
      mctx *c = &m[i];
      if (c->loop) {
        if (!c->n && now >= c->next_ns) m_push(&p, m, nm, c, now, old, &t_max, &t_min);
        continue;
      }
      if (now >= c->burst_ns + (uint64_t)c->sent * c->apart_us * 1000 && c->sent < c->jobs) { m_push(&p, m, nm, c, now, old, &t_max, &t_min); c->sent++; }
      if (c->after) continue;
      if (c->sent == c->jobs && now >= c->burst_ns + (uint64_t)c->every_ms * MS - M_TICK_NS) { c->burst_ns += (uint64_t)c->every_ms * MS; c->sent = 0; }
    }
    // The GPU: the highest ringbuffer with work; in it, the job begun, else the next context in turn.
    const unsigned speed4 = t_max > now ? 4 : t_min > now ? 1 : 2;
    if (speed4 == 1 && now >= start + 1000 * MS) *lowest_ns += M_TICK_NS;
    int ring = -1;
    for (unsigned r = 0; r < 4 && ring < 0; r++)
      for (unsigned i = 0; i < nm; i++) if (m[i].ring == r && m[i].n) { ring = (int)r; break; }
    if (ring < 0) continue;
    if (!busy[ring] && (cur[ring] < 0 || !m[cur[ring]].n || burst[ring] >= 5)) {
      // The next context in turn with work (the same one again when it is the only one: its next burst).
      const unsigned from = cur[ring] < 0 ? 0 : (unsigned)cur[ring] + 1;
      for (unsigned k = 0; k < nm; k++) {
        const unsigned i = (from + k) % nm;
        if (m[i].ring == (unsigned)ring && m[i].n) { cur[ring] = (int)i; burst[ring] = 0; break; }
      }
    }
    mctx *c = &m[cur[ring]];
    mjob *j = &c->q[c->head];
    busy[ring] = true;
    const uint64_t step = (uint64_t)M_TICK_NS * speed4 / 4;
    if (j->left_ns > step) { j->left_ns -= step; continue; }
    const uint64_t done = now + M_TICK_NS, took = done - j->submit_ns;
    if (j->submit_ns >= start + 1000 * MS) {         // the first second teaches the policy: not counted
      c->fences++; c->sum_ns += took; if (took > c->max_ns) c->max_ns = took;
      if (took >= (uint64_t)OG_FENCE_SLOW_US * 1000) c->over_slice++;
      if (took >= (uint64_t)p.bg_bound_us * 1000) c->over_bound++;
    }
    const uint64_t from = j->submit_ns > c->last_done_ns ? j->submit_ns : c->last_done_ns;
    og_frame_done(&p, &j->frame, (uint32_t)((done - from) / 1000));
    c->last_done_ns = done;
    c->head = (c->head + 1) % M_JOBS;
    c->n--;
    busy[ring] = false;
    burst[ring]++;
    if (c->loop) c->next_ns = done + 1 * MS;
    if (!c->loop && !c->n && c->sent == c->jobs)
      for (unsigned i = 0; i < nm; i++)
        if (m[i].after == (int)(c - m) + 1) { m[i].burst_ns = done + 2 * MS; m[i].sent = 0; }
  }
}

static void starvation(void)
{
  // A home screen with widgets, still otherwise, for 10 s after a first second that is not counted: once a second the
  // widget renderer redraws its 6 surfaces (one fence each), and 2 ms after the last is done the render server composites
  // 3 frames of it (60 Hz); photoanalysisd computes all the time. The widget renderer is a non-UI process at utility QoS to
  // the guest (the worst its hint can say), and the render server reads its surfaces. Three cases: small widgets (1 ms of
  // GPU a surface), heavy ones (5 ms), and heavy ones while SpringBoard also draws a frame at each refresh.
  static const struct { uint32_t size_us; unsigned clients; const char *what; } cases[] = {
    { 1000, 3, "small widgets" }, { 5000, 3, "heavy widgets" }, { 5000, 4, "heavy widgets, SpringBoard draws too" } };
  for (unsigned s = 0; s < sizeof cases / sizeof cases[0]; s++) {
    uint64_t low[2];
    mctx runs[2][M_CTX];
    const unsigned nm = cases[s].clients;
    for (int old = 1; old >= 0; old--) {
      mctx m[M_CTX] = {
        { .name = "xnu:backboardd", .after = 2, .jobs = 3, .apart_us = 16667, .size_us = 1500 },
        { .name = "xnu:WidgetRenderer_Default", .hint = OG_HINT_VALID | OG_ROLE_NONUI, .qos = OG_QOS_UTILITY, .shown = true,
          .old_background = true, .every_ms = 1000, .at_ms = 0, .jobs = 6, .apart_us = 500, .size_us = cases[s].size_us },
        { .name = "xnu:photoanalysisd", .loop = true, .size_us = 8000 },
        { .name = "xnu:SpringBoard", .every_ms = 1000, .at_ms = 5, .jobs = 1, .apart_us = 0, .size_us = 2000 },
      };
      m_run(m, nm, old, 11, &low[old]);
      memcpy(runs[old], m, sizeof m);
    }
    printf("\n%-40s %-4s %7s %9s %9s %10s %10s\n", cases[s].what, "", "fences", "mean ms", "max ms", ">= 50 ms", ">= 100 ms");
    for (unsigned i = 0; i < nm; i++)
      for (int old = 1; old >= 0; old--) {
        const mctx *c = &runs[old][i];
        printf("  %-38s %-4s %7llu %9.1f %9.1f %10llu %10llu\n", c->name + 4, old ? "#42" : "now", (unsigned long long)c->fences,
               c->fences ? (double)c->sum_ns / (double)c->fences / 1e6 : 0.0, (double)c->max_ns / 1e6,
               (unsigned long long)c->over_bound, (unsigned long long)c->over_slice);
      }
    printf("  %-38s #42 %.1f %%, now %.1f %%\n", "time at KGSL's lowest level", 100.0 * (double)low[1] / 1e10, 100.0 * (double)low[0] / 1e10);
    // Now: no client's fence reaches one of Metal's 100 ms wait slices, the widget's stay inside bg_bound_us (50 ms), and
    // are faster than under #42's rules.
    for (unsigned i = 0; i < nm; i++) CHECK(runs[0][i].over_slice == 0 && runs[0][i].fences > 0);
    CHECK(runs[0][1].fences == 60 && runs[0][1].over_bound == 0 && runs[0][1].max_ns < 50 * MS);
    CHECK(runs[0][1].max_ns < runs[1][1].max_ns && runs[0][1].sum_ns < runs[1][1].sum_ns);
  }
}

int main(void)
{
  classes();
  pages();
  levels();
  lowest();
  before_after();
  starvation();
  printf("test_gpu_policy: %u checks, %u failed\n", checks, failed);
  return failed ? 1 : 0;
}
