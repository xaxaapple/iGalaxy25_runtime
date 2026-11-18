// SPDX-License-Identifier: MIT
// The wire layout of xnuhost's "droidvm,virtio-media" device: a stateful V4L2 video encoder (and, later, a
// decoder) over virtio-mmio, as UniXNU-droidvm's guest transport DroidVMVirtioMedia.c drives it and
// DroidVMVTHost.cpp's session logic expects. It stands in for crosvm's --virtio-media device: the guest kernel's
// transport is unchanged, so the device must answer the same commands and events.
//
// Two queues:
//   commandq (0, driver -> device): OPEN -> session id; CLOSE; IOCTL(session, the _IOC_NR of a VIDIOC_*) with the
//            ioctl payload out (for _IOW/_IOWR) and back (for _IOR/_IOWR), a response header in front; MMAP(session,
//            offset) -> an offset into the host pool; MUNMAP. A response status is 0 or a Linux errno.
//   eventq   (1, device -> driver): the guest posts writable event buffers; the device fills one per event: DQBUF
//            (a struct v4l2_buffer and one struct v4l2_plane), EVENT (a struct v4l2_event), ERROR (the session is gone).
//
// Config space (offset 0 of the virtio config, i.e. MMIO 0x100): device_caps (u32), device_type (u32), card (32 bytes).
// All little-endian (the guest's own byte order); every structure matches the 64-bit Linux UAPI DroidVMVTHost reads.
#ifndef ONEIOS_ENCODER_MEDIA_PROTO_H
#define ONEIOS_ENCODER_MEDIA_PROTO_H

#include <stdint.h>

#define MEDIA_VIRTIO_DEVICE_ID 48          // VIRTIO_ID_MEDIA, as DroidVMVirtioMedia.c checks at MMIO 0x08

enum {
  MEDIA_QUEUE_COMMAND = 0,
  MEDIA_QUEUE_EVENT = 1,
  MEDIA_NQUEUES = 2,
};

// commandq command codes (cmd[0]); the response's status is at response[0].
enum {
  MEDIA_CMD_OPEN = 1,                      // -> response[2] = session id
  MEDIA_CMD_CLOSE = 2,                     // cmd[2] = session
  MEDIA_CMD_IOCTL = 3,                     // cmd[2] = session, cmd[3] = ioctl nr; out payload, then in payload back
  MEDIA_CMD_MMAP = 4,                      // cmd[2] = session, cmd[3] = writable, cmd[4] = offset; -> at(u64)@8, size(u64)@16
  MEDIA_CMD_MUNMAP = 5,                    // cmd[2..3] = the pool offset (lo, hi)
};

// eventq event kinds (event[0]); event[1] = session.
enum {
  MEDIA_EVT_ERROR = 0,
  MEDIA_EVT_DQBUF = 1,                     // event[2..] = struct v4l2_buffer + one struct v4l2_plane
  MEDIA_EVT_EVENT = 2,                     // event[2..] = struct v4l2_event (its first u32 is the type)
};
#define MEDIA_EVENT_BYTES 640              // >= 8 + sizeof(v4l2_buffer) + 8*64 (DroidVMVirtioMedia.c MEDIA_EVENT_SIZE)

// V4L2 ioctl numbers, by _IOC_NR, the values DroidVMVTHost.cpp passes (and DroidVMVirtioMedia.c's ENUM_FMT).
enum {
  MEDIA_VIDIOC_ENUM_FMT = 2,
  MEDIA_VIDIOC_G_FMT = 4,
  MEDIA_VIDIOC_S_FMT = 5,
  MEDIA_VIDIOC_REQBUFS = 8,
  MEDIA_VIDIOC_QUERYBUF = 9,
  MEDIA_VIDIOC_QBUF = 15,
  MEDIA_VIDIOC_STREAMON = 18,
  MEDIA_VIDIOC_STREAMOFF = 19,
  MEDIA_VIDIOC_G_CTRL = 27,
  MEDIA_VIDIOC_ENUM_FRAMESIZES = 74,
  MEDIA_VIDIOC_ENCODER_CMD = 77,
  MEDIA_VIDIOC_SUBSCRIBE_EVENT = 90,
  MEDIA_VIDIOC_DECODER_CMD = 96,
};

// V4L2 buffer types and memory.
enum {
  MEDIA_BUF_TYPE_VIDEO_CAPTURE_MPLANE = 9,
  MEDIA_BUF_TYPE_VIDEO_OUTPUT_MPLANE = 10,
  MEDIA_MEMORY_MMAP = 1,
};

// V4L2 device_caps bits (config space).
enum {
  MEDIA_CAP_VIDEO_M2M_MPLANE = 0x4000,
  MEDIA_CAP_STREAMING = 0x04000000,
};

// V4L2 buffer flags (struct v4l2_buffer .flags).
enum {
  MEDIA_BUF_FLAG_KEYFRAME = 0x8,
  MEDIA_BUF_FLAG_ERROR = 0x40,
  MEDIA_BUF_FLAG_LAST = 0x00100000,
};

// V4L2 events and encoder commands.
enum {
  MEDIA_EVENT_EOS = 2,
  MEDIA_EVENT_SOURCE_CHANGE = 5,
  MEDIA_ENC_CMD_START = 0,
  MEDIA_ENC_CMD_STOP = 1,
};

// FourCC pixel formats (little-endian, as V4L2 keeps them).
#define MEDIA_FOURCC(a, b, c, d) ((uint32_t)(a) | (uint32_t)(b) << 8 | (uint32_t)(c) << 16 | (uint32_t)(d) << 24)
#define MEDIA_PIXFMT_NV12 MEDIA_FOURCC('N', 'V', '1', '2')
#define MEDIA_PIXFMT_H264 MEDIA_FOURCC('H', '2', '6', '4')
#define MEDIA_PIXFMT_HEVC MEDIA_FOURCC('H', 'E', 'V', 'C')

// struct v4l2_buffer followed by one struct v4l2_plane (DroidVMVTHost.cpp V4l2Buffer, 88 + 64 bytes).
struct media_v4l2_buffer {
  uint32_t index, type, bytesused, flags, field, pad0;
  int64_t sec, usec;
  uint8_t timecode[16];
  uint32_t sequence, memory;
  uint64_t m;
  uint32_t length, reserved2, request_fd, pad1;
  struct { uint32_t bytesused, length; uint64_t m; uint32_t data_offset, reserved[11]; } plane;
};

// struct v4l2_format with v4l2_pix_format_mplane (DroidVMVTHost.cpp V4l2Format, 208 bytes).
struct media_v4l2_format {
  uint32_t type, pad;
  uint32_t width, height, pixelformat, field, colorspace;
  struct { uint32_t sizeimage, bytesperline; uint16_t reserved[6]; } planes[8];
  uint8_t num_planes, flags, ycbcr, quantization, xfer, reserved[7];
  uint8_t rest[200 - 192];
};

#endif
