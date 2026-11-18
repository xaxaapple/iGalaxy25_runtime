// SPDX-License-Identifier: MIT
// When the guest is up: the rule behind session/ready and the `guest ready: first frame` line (docs/booter.md 2.8).
// Pure and unit-tested (tests/test_xnuhost.c); main.c holds the state under a lock and feeds it the frames the
// virtio-gpu device shows and its 100 ms tick.
//
// The guest's first frame of its own (not the boot console's: virtio_gpu.c's frame_shown leaves those out) is
// backboardd's, and it is black: on the phone (main 3233fed, 2026-11-15) ready came at +133 s at that frame, the
// OneOS boot screen ended there, and the panel stayed black until iOS drew its Apple logo. So the guest is ready at
// the first frame of its own that shows something (xh_pixels_lit). An iOS that only ever draws black is still ready
// `ready-content-wait` seconds after its first frame (0: at the first frame, the rule before 2026-11-15), so the
// switch's dead-man timer and the boot screen do not wait for ever on it.
#include "xnuhost.h"

void xh_ready_init(xh_ready_state *st)
{
  st->first_ms = 0;
  st->frames = 0;
  st->ready = false;
}

xh_ready_why xh_ready_on_frame(xh_ready_state *st, bool lit, int wait_s, uint64_t now_ms)
{
  if (st->ready) return XH_READY_ALREADY;
  if (!st->frames++) st->first_ms = now_ms;
  if (lit) { st->ready = true; return XH_READY_CONTENT; }
  if (wait_s <= 0) { st->ready = true; return XH_READY_ANY_FRAME; }
  return xh_ready_on_tick(st, wait_s, now_ms);
}

xh_ready_why xh_ready_on_tick(xh_ready_state *st, int wait_s, uint64_t now_ms)
{
  if (st->ready) return XH_READY_ALREADY;
  if (!st->frames || wait_s <= 0 || now_ms < st->first_ms || now_ms - st->first_ms < (uint64_t)wait_s * 1000u) return XH_READY_NOT;
  st->ready = true;
  return XH_READY_BLACK_TIMEOUT;
}

xh_ready_why xh_ready_on_call(xh_ready_state *st)
{
  if (st->ready) return XH_READY_ALREADY;
  st->ready = true;
  return XH_READY_CALL;
}
