# codec/: the video encoder and decoder (hevc-encoder, 2026-11-15)

Owner: the encoder agent. The host side: `runtime/bridges/encoder`. The whole path and the route: `docs/hardware.md`,
section 7g ("Video encoder").

**State (2026-11-15):** the **host side is done and proven on the PC** (the virtio-media device, the oneios-encoder
helper, a stand-in encoder; `runtime/bridges/encoder`). This guest archive is **written but not yet built into a
`kernel.kc` and not yet run** — it stages existing, upstream-proven UniXNU-droidvm code, and its generator
(`oneios-tool kernel gen codec`) and build-hosted wiring are the next step (below). The weak hook
(`oneios_codec_start` in `drivers/bridge/OneiOSBridge.c`) and xnuhost's `droidvm-vt-paravirt=1` boot argument
(added when `encoder=auto`) are in place, so a kernel built with this archive starts it; a kernel without it is
unchanged (the weak default does nothing).

## Why this route

iOS's VideoToolbox in 24A446 carries a whole **paravirtualized** encoder and decoder
(`ParavirtualizedVideoEncoder_*`, read in the 24A446 shared cache) and uses them for every eligible codec once
`VTParavirtualizationIsRunningInGuest` — i.e. once the service **`AppleVideoToolboxParavirtualizationDriver`** is
published (string in `VideoToolboxParavirtualizationSupport` 71.0.14). CMPhoto's HEIC writer takes that path: its
`_CMPhotoSupportsHEVCHWEncode` block calls `VTSelectAndCreateVideoEncoderInstance` with
`kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder` true and reports HEVC hardware support when a
hardware encoder answers (disassembled, 2026-11-15). The paravirtualized encoder answers
`UsingHardwareAcceleratedVideoEncoder` true, so CMPhoto sees hardware HEVC and `CMPhotoCompressionSessionAddImage`
with `codec: 'hvc1'` stops failing.

So the camera's plug-in route is **not** used here: no AVE2 user-client emulation (AVE2's firmware command ABI is
Apple's and not serveable by another encoder), no `.videoencoder` bundle of ours. We publish Apple's own
paravirtualization service, exactly as the Metal and camera plug-ins are iOS-side code built on Apple's own loaders.
[F] the cache reads; [I] that the first session takes the device as it is (the next session's log confirms).

## The three staged sources (UniXNU-droidvm f9121cd, `external/UniXNU-droidvm/package`, unchanged)

| File | What |
|---|---|
| `DroidVMVirtioMedia.c` | the virtio-media transport (V4L2 stateful codec over virtio-mmio): `/arm-io/virtio-media-0`, `media-host-pool`. The device is xnuhost's (`runtime/bridges/encoder/media_device.c`), not crosvm's. `-DDROIDVM_DEVICE_DYNAMIC=1`, Mach universe, as its iPadOS 27 board builds it |
| `DroidVMVTHost.cpp` | the session logic: `encs`/`+enc`/`esta`/`efrm`/`Eemt`/`ecfr`; the IOSurface lookup (IOCoreSurfaceRoot), NV12 copy into the OUTPUT buffer, hvcC/avcC assembly, the sample-buffer atoms. Runs in the guest kernel over the virtio-media device |
| `DroidVMVTParavirt.cpp` | Apple's `AppleVideoToolboxParavirtualizationDriver` user client + the `IODataQueue` transport; publishes the service with `droidvm-vt-paravirt=1` |

`OneiOSCodec.c` (this folder, OneiOS's own) is the start: `oneios_codec_start` → `droidvm_media_start()` +
`droidvm_vt_paravirt_attach()`. It is the archive's only non-upstream source.

## What the Mac must build and stage (the remaining step)

1. Add the generator `oneios-tool kernel gen codec` (crate `oneios-kernel`, modelled on `build_oneios_audio.rs`:
   stage the three files + `OneiOSCodec.c`, compile from the first kernel pass's records, archive
   `build/oneios-codec/liboneioscodec.a`, require `_oneios_codec_start`, `_droidvm_media_start`,
   `_droidvm_vt_paravirt_attach`, and the two `__GLOBAL__sub_I_*` module loads taken out of `__mod_init_func`).
2. Add `codec` to `kernel/build-hosted.sh`'s archive list and `CONFIG_XNU_EXTRA_LD_ARCHIVES` (after `audio`).
3. Build `kernel.kc` (`kernel/build-hosted.sh`), stage it, boot with `encoder=auto`.

Then the host side (already built) provides `/arm-io/virtio-media-0` and `bin/oneios-encoder`, and
`CMPhotoCompressionSessionAddImage(codec: 'hvc1')` reaches our MediaCodec.

## What the next session's logs must show

Kernel (`xnuhost.log`): `encoder: on: mediacodec-helper backend, media pool 48 MiB at ...`; `ONEIOS CODEC: 1
virtio-media device(s), 1 up; VideoToolbox paravirtualization registered (stage 2)`.

iOS's log (`oneios-tool ioslog`):
- **no** `CMPhotoCompressionSessionAddImage ... (codec: 'hvc1')` / `kCMBaseObjectError_UnsupportedOperation` errors;
- WidgetRenderer_Default **without** `Content load failed: unable to find or unarchive file for key: ...systemSmall...`;
- `oneios-gpu-pixels: WidgetRenderer_ ... drawn`;
- a screenshot saved as HEIC (its first bytes `....ftypheic`).
