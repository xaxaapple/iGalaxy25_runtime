// SPDX-License-Identifier: BSD-2-Clause-Patent
//
// The hosted kernel's video codec: oneios_codec_start, called once by oneios_drivers_start
// (drivers/bridge/OneiOSBridge.c; a weak default there for a kernel without this archive). It brings up
// UniXNU-droidvm's virtio-media transport (DroidVMVirtioMedia.c: xnuhost's /arm-io/virtio-media-0, the host end
// runtime/bridges/encoder) and publishes iOS's paravirtualized VideoToolbox driver (DroidVMVTParavirt.cpp +
// DroidVMVTHost.cpp), so VideoToolbox encodes HEVC/H.264 through the phone's hardware encoder and CMPhoto writes
// HEIC (chronod's widget snapshots, screenshots, the photo library). See docs/hardware.md, "Video encoder".
//
// droidvm-media (default 1) brings up the transport; droidvm-vt-paravirt=1 (xnuhost adds it when encoder=auto)
// publishes AppleVideoToolboxParavirtualizationDriver. oneios-codec=0 leaves the whole archive out at boot.
#include <stdbool.h>
#include <stdint.h>
#include <pexpert/pexpert.h>

extern int printf(const char *, ...);
extern void droidvm_media_start(void);                 // DroidVMVirtioMedia.c
extern int droidvm_vt_paravirt_attach(void);           // DroidVMVTParavirt.cpp
extern volatile uint32_t droidvm_media_found, droidvm_media_up;
extern volatile uint32_t droidvm_vtpv_stage;

void oneios_codec_start(void) {
    uint32_t enabled = 1;
    PE_parse_boot_argn("oneios-codec", &enabled, sizeof enabled);
    if (!enabled) { printf("ONEIOS CODEC: off (oneios-codec=0)\n"); return; }

    droidvm_media_start();                             // the virtio-media encoder/decoder transport
    const int paravirt = droidvm_vt_paravirt_attach(); // the VideoToolbox paravirtualization classes + service

    printf("ONEIOS CODEC: %u virtio-media device(s), %u up; VideoToolbox paravirtualization %s (stage %u)\n",
           droidvm_media_found, droidvm_media_up,
           paravirt == 0 ? "registered" : "not published", droidvm_vtpv_stage);
}
