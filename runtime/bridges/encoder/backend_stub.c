// SPDX-License-Identifier: MIT
// The PC stand-in encoder (encoder.h): the box has no MediaCodec, so instead of really compressing it emits, for each
// NV12 frame fed, one structurally valid Annex-B access unit -- VPS/SPS/PPS + an IDR slice for HEVC, SPS/PPS + an IDR
// for H.264 -- whose NAL headers and types are what DroidVMVTHost.cpp's splitter and CMPhoto read, and whose IDR body
// carries the frame's dimensions and a checksum of the NV12, so a host test can confirm the real pixels reached the
// encoder and the bitstream layout is right. It compresses nothing: it checks the path.
#include "encoder.h"
#include <errno.h>
#include <stdlib.h>
#include <string.h>

enum { STUB_RING = 16 };

struct coded {
  uint8_t *bytes;
  uint32_t size;
  int64_t pts_us;
};

struct br_enc_session {
  uint32_t codec, width, height;
  struct coded ring[STUB_RING];
  unsigned head, tail;
  bool draining, eos_done;
};

static uint32_t fnv1a(const uint8_t *p, uint32_t n) {
  uint32_t h = 2166136261u;
  for (uint32_t i = 0; i < n; i++) { h ^= p[i]; h *= 16777619u; }
  return h;
}

// A NAL unit with a 4-byte start code into *at, growing it. hevc: 2-byte header (type<<9), else 1-byte (type).
static uint32_t emit_nal(uint8_t *out, uint32_t at, bool hevc, unsigned type, const uint8_t *body, uint32_t body_len) {
  out[at++] = 0; out[at++] = 0; out[at++] = 0; out[at++] = 1;
  if (hevc) { out[at++] = (uint8_t)(type << 1); out[at++] = 1; }    // nuh_layer_id 0, temporal_id_plus1 1
  else out[at++] = (uint8_t)(0x60 | type);                          // nal_ref_idc 3
  memcpy(out + at, body, body_len);
  return at + body_len;
}

static br_enc_session *stub_open(uint32_t codec, uint32_t width, uint32_t height) {
  br_enc_session *s = calloc(1, sizeof *s);
  if (!s) return NULL;
  s->codec = codec;
  s->width = width;
  s->height = height;
  return s;
}

static int stub_feed(br_enc_session *s, const uint8_t *nv12, uint32_t len, int64_t pts_us, int force_key) {
  (void)force_key;
  if (s->head - s->tail >= STUB_RING) return -ENOSPC;
  bool hevc = s->codec == MEDIA_PIXFMT_HEVC;
  // Parameter sets: a VPS (hevc), an SPS padded so its profile_tier_level offsets exist, a PPS.
  uint8_t sps[24];
  memset(sps, 0, sizeof sps);
  sps[0] = 1;                                                       // sps_video_parameter_set_id etc (opaque here)
  sps[1] = 0x01; sps[2] = 0x60;                                     // general_profile_space/tier/idc: Main, main tier
  sps[8] = 0x5a;                                                    // a general_level_idc (level 3)
  // The IDR body: a marker the host test reads (magic, w, h, nv12 checksum), then a little filler.
  uint8_t idr[32];
  memset(idr, 0, sizeof idr);
  memcpy(idr, "ONEI", 4);
  uint32_t w = s->width, h = s->height, crc = fnv1a(nv12, len);
  memcpy(idr + 4, &w, 4);
  memcpy(idr + 8, &h, 4);
  memcpy(idr + 12, &crc, 4);
  memcpy(idr + 16, &len, 4);

  uint32_t cap = 4 + 64 + 4 + sizeof sps + 4 + 8 + 4 + 6 + sizeof idr + 16;
  uint8_t *buf = malloc(cap);
  if (!buf) return -ENOMEM;
  uint32_t at = 0;
  if (hevc) { uint8_t vps[8] = { 0 }; at = emit_nal(buf, at, true, 32, vps, sizeof vps); }   // VPS
  at = emit_nal(buf, at, hevc, hevc ? 33 : 7, sps, sizeof sps);                               // SPS
  uint8_t pps[4] = { 0 };
  at = emit_nal(buf, at, hevc, hevc ? 34 : 8, pps, sizeof pps);                               // PPS
  at = emit_nal(buf, at, hevc, hevc ? 19 : 5, idr, sizeof idr);                               // IDR slice
  s->ring[s->head % STUB_RING] = (struct coded){ buf, at, pts_us };
  s->head++;
  return 0;
}

static int stub_drain(br_enc_session *s) { s->draining = true; return 0; }

static int stub_poll(br_enc_session *s, uint8_t *buf, uint32_t cap, int *key, int64_t *pts_us, int *eos) {
  *key = 0; *eos = 0; *pts_us = 0;
  if (s->tail != s->head) {
    struct coded *c = &s->ring[s->tail % STUB_RING];
    if (c->size > cap) return -ENOSPC;
    memcpy(buf, c->bytes, c->size);
    *key = 1;                                                       // every still is an IDR
    *pts_us = c->pts_us;
    free(c->bytes);
    c->bytes = NULL;
    s->tail++;
    return (int)c->size;
  }
  if (s->draining && !s->eos_done) { s->eos_done = true; *eos = 1; return 0; }
  return 0;
}

static void stub_close(br_enc_session *s) {
  while (s->tail != s->head) { free(s->ring[s->tail % STUB_RING].bytes); s->tail++; }
  free(s);
}

static const br_encoder_backend stub = {
  .name = "stub", .open = stub_open, .feed = stub_feed, .drain = stub_drain, .poll = stub_poll, .close = stub_close,
};

const br_encoder_backend *br_encoder_backend_stub(void) { return &stub; }
