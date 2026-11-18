# Audio: speaker and microphone

No source here. The kernel side is UniXNU-droidvm's (f9121cd): `package/DroidVMAudio.cpp` (two `IOAudio2Device`s,
"Speaker" and "Mic") over `DroidVMVirtioSnd.c` (virtio-snd: 48 kHz S16, stereo out, mono in, 240-frame periods).
They are built into the hosted kernel by its audio archive, `oneios-tool kernel gen audio`
(since 2026-11-12; before, they were not built at all and iOS had no audio device): the transport unchanged,
`DroidVMAudio.cpp` staged with boot arguments for its UIDs and its output latency, and `OneiOSAudio.c`, the start
that `oneios_drivers_start` calls.

The device it drives is `runtime/bridges/devices/virtio_snd.c` at `/arm-io/virtio-snd` (`droidvm,virtio-snd`),
backed by the AAudio helper `oneios-audio`. Everything else (boot arguments, log lines, measurements):
`docs/hardware.md` section 6.

Since 2026-11-12 (media) the archive also stages a third device, UID "Actuator" (Core Haptics, UI and keyboard
taps): mono, the speaker's engine, its periods to `../haptics/oneios_haptic.c` (`oneios_haptic_actuator_play`), not to
virtio-snd. `docs/hardware.md` section 5.

Since 2026-11-13 (fix-haptics) the Actuator's user clients also report to `drivers/haptics`
(`oneios_haptic_actuator_event`, kinds `OH_EV_*` of `drivers/bridge/oneios_bridge.h`): an open and a close, each
selector, a refused selector (also a `SetStreamCurrentFormat` of the wrong size), the format asked (channels and rate),
each mapping. Log: `ONEIOS HAPTIC: Actuator: user client N opened by ...` and the `ONEIOS HAPTIC: counters:` line.
The Speaker and Mic are not changed.

Since 2026-11-15 (fix-audio-latency) the staged source also changes the speaker's latency and the ring's wrap
(`tools/oneios-tool/crates/oneios-kernel/src/staged/audio.rs`, the block after the Actuator's):
- the speaker's ring is `oneios-audio-ring=FRAMES` (default 1920 frames): CoreAudio's HAL caps a client's I/O buffer
  at 0.375 of a device's `io buffer frame size`, and the 8192-frame ring let iOS take 3072 frames (64 ms) for
  system sounds; now at most 720 (15 ms);
- the speaker hands each period on as it starts (its output safety offset is the period, 240 frames, not 480), so
  its device clock is the handoff and its output latency is the host's: `oneios-audio-latency`, which xnuhost now
  sets from the helper's measured latency (default 720 frames without it);
- every device: a period that runs over the ring's end goes through a bounce buffer (8192 is not a whole number of
  240-frame periods: the engine read and zeroed, and the microphone wrote, 208 frames past the ring's buffer once a
  ring, and never played its first 208 frames), and the zero time stamp moves at each wrap (before: every 245760
  frames, when a period happened to end on one);
- the speaker engine's timing for `OneiOSAudio.c`'s report: `ONEIOS AUDIO: speaker engine: N periods, taken late by
  N us on average (max N); the HAL wrote N..N frames ahead (N on average, N ms: iOS's I/O buffer and safety offset),
  N periods tight (ring N frames)`, and at attach `DroidVMAudio: speaker: ring N frames (iOS's I/O buffer at most N),
  safety offset 240, each period handed on as it starts; output latency N frames`.
The Mic keeps its 8192-frame ring and 480-frame safety offset; the Actuator its ring and timing (only the wrap fix
reaches it). `docs/hardware.md` section 6, "Latency end to end".
