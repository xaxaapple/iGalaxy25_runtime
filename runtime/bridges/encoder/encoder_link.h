// SPDX-License-Identifier: MIT
// The link between xnuhost's end of the video encoder (the static process: media_device.c + backend_helper.c) and the
// oneios-encoder helper process (helper.c, which runs the NDK's MediaCodec -- a shared library, so it cannot be in the
// static xnuhost, exactly as the camera and audio helpers are separate processes). A length-framed message protocol
// over a UNIX stream socket; NV12 frames and coded bitstream are copied through it (widget-snapshot stills are small).
#ifndef ONEIOS_ENCODER_LINK_H
#define ONEIOS_ENCODER_LINK_H

#include <stdint.h>

#define ENC_LINK_VERSION 1

enum {
  ENC_MSG_HELLO = 1,        // helper -> device at start: a = ENC_LINK_VERSION
  ENC_MSG_OPEN = 2,         // device -> helper: a = codec FourCC, b = width, c = height
  ENC_MSG_OPEN_REPLY = 3,   // helper -> device: session (0 = refused)
  ENC_MSG_FEED = 4,         // device -> helper: session, pts (us) in pts, payload = NV12
  ENC_MSG_DRAIN = 5,        // device -> helper: session
  ENC_MSG_CLOSE = 6,        // device -> helper: session
  ENC_MSG_CODED = 7,        // helper -> device: session, pts, flags (bit0 key, bit1 eos), payload = Annex-B AU
  ENC_MSG_EOS = 8,          // helper -> device: session (the drain finished, no more coded buffers)
  ENC_MSG_ERROR = 9,        // helper -> device: session
};

// Every message: this header, then `len` payload bytes.
struct enc_msg {
  uint32_t type;
  uint32_t session;
  int64_t pts;
  uint32_t a, b, c;         // small scalars (codec/width/height, flags)
  uint32_t len;             // payload bytes that follow
};

#endif
