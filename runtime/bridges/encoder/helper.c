// SPDX-License-Identifier: MIT
// oneios-encoder: the video encoder helper process (bin/oneios-encoder in the runtime package). MediaCodec is a shared
// library, so it cannot run inside the static xnuhost; this helper does, exactly as oneios-camera and oneios-audio do.
// It reads the device's messages (encoder_link.h) on the inherited socket (fd given as argv[1], default 3), runs the
// real encoder (backend_ndk on Android; backend_stub on the host, for the test), and streams the coded access units
// back. One session per virtio-media session; the session's frames are drained after each feed and after a drain.
#define _GNU_SOURCE
#include "encoder.h"
#include "encoder_link.h"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// The helper links the real backend directly (no socket inside it). ONEIOS_ENCODER_STUB=1 forces the stand-in.
void (*br_log_sink)(int level, const char *line);
void br_log(int level, const char *fmt, ...) { (void)level; (void)fmt; }

enum { HELPER_SESSIONS = 8, CODED_CAP = 8u << 20 };

static int write_all(int fd, const void *p, size_t n) {
  const uint8_t *b = p;
  while (n) { ssize_t w = write(fd, b, n); if (w <= 0) { if (w < 0 && errno == EINTR) continue; return -1; } b += w; n -= (size_t)w; }
  return 0;
}
static int read_all(int fd, void *p, size_t n) {
  uint8_t *b = p;
  while (n) { ssize_t r = read(fd, b, n); if (r <= 0) { if (r < 0 && errno == EINTR) continue; return -1; } b += r; n -= (size_t)r; }
  return 0;
}

static int send_msg(int fd, uint32_t type, uint32_t session, int64_t pts, uint32_t a, const void *payload, uint32_t len) {
  struct enc_msg m = { type, session, pts, a, 0, 0, len };
  if (write_all(fd, &m, sizeof m)) return -1;
  return len ? write_all(fd, payload, len) : 0;
}

int main(int argc, char **argv) {
  int fd = argc > 1 ? atoi(argv[1]) : 3;
#ifdef ONEIOS_ENCODER_STUB_ONLY
  const br_encoder_backend *backend = br_encoder_backend_stub();          // the host build: no MediaCodec on the box
#else
  const br_encoder_backend *backend = getenv("ONEIOS_ENCODER_STUB") ? br_encoder_backend_stub() : br_encoder_backend_ndk();
#endif

  struct { br_enc_session *enc; uint32_t id; } sess[HELPER_SESSIONS] = { { 0 } };
  uint8_t *coded = malloc(CODED_CAP), *nv12 = NULL;
  uint32_t nv12_cap = 0;
  if (!coded) return 1;

  if (send_msg(fd, ENC_MSG_HELLO, 0, 0, ENC_LINK_VERSION, NULL, 0)) return 1;

  for (;;) {
    struct enc_msg m;
    if (read_all(fd, &m, sizeof m)) break;
    if (m.len) {
      if (m.len > nv12_cap) { free(nv12); nv12 = malloc(m.len); nv12_cap = nv12 ? m.len : 0; }
      if (!nv12 || read_all(fd, nv12, m.len)) break;
    }
    unsigned slot = HELPER_SESSIONS;
    for (unsigned i = 0; i < HELPER_SESSIONS; i++) if (sess[i].enc && sess[i].id == m.session) { slot = i; break; }

    if (m.type == ENC_MSG_OPEN) {
      unsigned free_slot = HELPER_SESSIONS;
      for (unsigned i = 0; i < HELPER_SESSIONS; i++) if (!sess[i].enc) { free_slot = i; break; }
      br_enc_session *e = free_slot < HELPER_SESSIONS ? backend->open(m.a, m.b, m.c) : NULL;
      if (e) { sess[free_slot].enc = e; sess[free_slot].id = m.session; }
      send_msg(fd, ENC_MSG_OPEN_REPLY, m.session, 0, e ? 1 : 0, NULL, 0);
    } else if (m.type == ENC_MSG_FEED && slot < HELPER_SESSIONS) {
      backend->feed(sess[slot].enc, nv12, m.len, m.pts, (int)(m.a & 1));
      // Drain whatever came out.
      for (;;) {
        int key = 0, eos = 0; int64_t pts = 0;
        int bytes = backend->poll(sess[slot].enc, coded, CODED_CAP, &key, &pts, &eos);
        if (bytes <= 0 && !eos) break;
        if (bytes > 0) send_msg(fd, ENC_MSG_CODED, m.session, pts, (uint32_t)(key | (eos << 1)), coded, (uint32_t)bytes);
        if (eos) break;
      }
    } else if (m.type == ENC_MSG_DRAIN && slot < HELPER_SESSIONS) {
      backend->drain(sess[slot].enc);
      for (unsigned waited = 0; waited < 1000; ) {               // at most ~2 s for the codec to flush
        int key = 0, eos = 0; int64_t pts = 0;
        int bytes = backend->poll(sess[slot].enc, coded, CODED_CAP, &key, &pts, &eos);
        if (bytes > 0) { send_msg(fd, ENC_MSG_CODED, m.session, pts, (uint32_t)(key | (eos << 1)), coded, (uint32_t)bytes); continue; }
        if (eos) { send_msg(fd, ENC_MSG_EOS, m.session, 0, 0, NULL, 0); break; }
        struct timespec ts = { 0, 2000000 }; nanosleep(&ts, NULL); waited++;
        if (waited == 1000) send_msg(fd, ENC_MSG_EOS, m.session, 0, 0, NULL, 0);
      }
    } else if (m.type == ENC_MSG_CLOSE && slot < HELPER_SESSIONS) {
      backend->close(sess[slot].enc);
      sess[slot].enc = NULL;
    }
  }

  for (unsigned i = 0; i < HELPER_SESSIONS; i++) if (sess[i].enc) backend->close(sess[i].enc);
  free(coded);
  free(nv12);
  return 0;
}
