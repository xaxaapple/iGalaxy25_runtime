// SPDX-License-Identifier: MIT
// The host video encoder bridge (docs/hardware.md, "Video encoder"): xnuhost's "droidvm,virtio-media" device
// (media_device.c) backed by a real encoder. iOS's VideoToolbox paravirtualized encoder (the guest kernel archive
// codec/, UniXNU-droidvm's DroidVMVTParavirt.cpp + DroidVMVTHost.cpp + DroidVMVirtioMedia.c) drives a stateful V4L2
// encoder; this bridge is the host end of that V4L2 codec. It feeds the NV12 frames VideoToolbox gives it to the
// backend and returns the HEVC/H.264 bitstream (Annex-B NAL units), so CMPhoto writes HEIC.
//
// Backends (both samsung and treble use the same path: Android's MediaCodec):
//   br_encoder_backend_ndk    the NDK's AMediaCodec (video/hevc -> c2.qti.hevc.encoder, video/avc), surface or
//                             ByteBuffer input; the phone and `encctl`
//   br_encoder_backend_stub   the PC stand-in: no MediaCodec on the box, so it emits a structurally valid HEVC/H.264
//                             Annex-B access unit (VPS/SPS/PPS + one IDR slice) sized to the frame for the host test
#ifndef ONEIOS_ENCODER_H
#define ONEIOS_ENCODER_H

#include <stdint.h>
#include <stddef.h>
#include "bridges/bridges.h"
#include "xnuhost.h"
#include "media_proto.h"

// One encoder session on the backend. The device owns at most one per virtio-media session.
typedef struct br_enc_session br_enc_session;

typedef struct {
  const char *name;
  // Open a session for `codec` (MEDIA_PIXFMT_HEVC or MEDIA_PIXFMT_H264) at width x height, tightly packed NV12 input.
  // NULL on failure.
  br_enc_session *(*open)(uint32_t codec, uint32_t width, uint32_t height);
  // Feed one NV12 frame (w*h luma + w*h/2 chroma, tightly packed). pts in microseconds. force_key requests an IDR.
  // 0 or -errno. The backend copies what it needs; the caller may reuse the buffer at once.
  int (*feed)(br_enc_session *s, const uint8_t *nv12, uint32_t len, int64_t pts_us, int force_key);
  // Ask the backend to finish every frame fed so far (end of stream).
  int (*drain)(br_enc_session *s);
  // One coded access unit, Annex-B (start codes), into buf: the bytes, 0 if none is ready yet, or -errno. *key is set
  // for an IDR/keyframe, *pts_us the frame's pts, *eos once the last buffer has come out after a drain.
  int (*poll)(br_enc_session *s, uint8_t *buf, uint32_t cap, int *key, int64_t *pts_us, int *eos);
  void (*close)(br_enc_session *s);
} br_encoder_backend;

const br_encoder_backend *br_encoder_backend_ndk(void);
const br_encoder_backend *br_encoder_backend_stub(void);
// The device-side backend that runs the real encoder in the oneios-encoder helper process (encoder_link.h): xnuhost
// is static and MediaCodec is a shared library, so the NDK backend lives in a helper, as the camera's does. `path` is
// the helper binary (runtime/bin/oneios-encoder).
const br_encoder_backend *br_encoder_backend_helper(const char *path);

// The "droidvm,virtio-media" encoder device on xnuhost's transport. `pool_host` is the host mapping of the media pool
// (guest physical `pool_gpa`, `pool_size` bytes) in which the MMAP'd V4L2 buffers live; the guest maps the same pool
// through its device-tree media-host-pool property. NULL when the device could not be made.
xh_virtio *br_encoder_create(uint64_t base, unsigned spi, const br_encoder_backend *backend,
                             void *pool_host, uint64_t pool_gpa, uint64_t pool_size);

// Counters for the host rig and `encctl` (read under the device lock is not needed; monotonic).
typedef struct {
  uint64_t sessions, frames_in, frames_out, errors;
} br_encoder_stats;
void br_encoder_get_stats(xh_virtio *dev, br_encoder_stats *out);

#endif
