// SPDX-License-Identifier: MIT
// Messages between xnuhost's audio back end (audio/backend.c) and the AAudio helper process
// (audio/aaudio_helper.c) on their socket pair (SOCK_SEQPACKET: one message per packet).
#ifndef ONEIOS_AUDIO_PROTO_H
#define ONEIOS_AUDIO_PROTO_H
#include <stdint.h>
// AU_PROBE (xnuhost -> helper, before the guest boots): open the speaker in silence and measure its latency;
// AU_LATENCY (helper -> xnuhost): its answer, value = frames at 48 kHz (0: not measured); AU_TOLD (xnuhost ->
// helper): the output latency the guest publishes to iOS, value = frames (for the helper's end-to-end line).
enum { AU_START = 1, AU_STOP = 2, AU_PCM = 3, AU_STATE = 4, AU_PROBE = 5, AU_LATENCY = 6, AU_TOLD = 7 };
struct au_header {
  uint8_t  kind;          // AU_*
  uint8_t  output;        // 1 = the speaker stream, 0 = the microphone stream
  uint8_t  channels;
  uint8_t  reserved;
  uint32_t value;         // START: sample rate; PCM: frames that follow (S16LE interleaved); STATE: 0 ok, else errno
};
#define AU_MAX_FRAMES 960u
#endif
