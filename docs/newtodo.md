# Paused work (2026-11-13; stopped again 2026-11-15; resumed 2026-11-16): what is done, what is next, how to resume

The owner paused the work on 2026-11-13 at about 15:00 IST. This file is the hand-over. Two machines worked on it:
the Mac (Xcode, the phone on its USB, the phone sessions) and a Linux PC (CachyOS, x86_64, 4 CPUs: the kernel, the
runtime, `oneios-tool`, PC boots under qemu-aarch64). Read this with `README.md` "Status", `docs/agents.md` and the
docs each item names. **Proven** = ran on the phone; **built** = compiled and host-tested, not run on the phone.

## 2026-11-16, phone fix: #60 is merged and iOS 27 reaches the first real frame again

Main `ddcb356` includes PR **#60**. The follow-up work on top (not yet committed at the time of this note):
0080's shared-region readiness guard, the fork's bare-mode `shmem_enabled` save/restore, and oneios-tool's macOS
scheduler-class guard. Build: `HOSTED_OUT=phonefix kernel/build-hosted.sh`; collection
`out/kernel/phonefix-24A446/kernel.kc`, sha256 `f171b71e8b1e2c58785c8f060e7a06a3021ef33c8f9ebab927baa3e8255d25d4`,
96,813,056 bytes; prelink audit 2,712 paired, 92 function stubs, 9 data unpaired, **0 failures**.

- The previous phone panic was `newfs_apfs` SIGSEGV after a DYLD unnest, then `offline scratch Data mount failed: 5`.
  The original 0080 also halted during early VM bootstrap because its shared-region reporting path ran too early. Now
  `vm_shared_region_create` sets `hosted_shared_region_ready` only after a region exists; both 0080's vm_map shortcut
  and `hosted_top_note_protect` are inert before then. On the phone the new kernel passed VM bootstrap, `newfs_apfs`
  completed, the new seed verified (6,030 files, 14,816 folders), launchd ran, and xnuhost logged `guest ready` and
  made `session/ready`. Evidence: `out/session-logs/2026-11-16/phonefix-xnuhost.log`. `ram=auto` selected 6,912 MiB;
  2 MiB host pages and the shmem advise mode were active. No PC A/B run has been done. `oneios-top` showed unchanged
  pmap entries kept mapped, but **zero shared-region no-ops**, so the no-op optimisation and typing comparison remain.
- The previous return failed Gate A because the switch saved `always within_size advise [never] deny force` and the
  return wrote that annotated line back (`EINVAL`), causing a failsafe reboot. Both scripts now save and compare only
  the bracketed bare mode (`never`); the phone's switch logged `shmem_enabled: never -> advise`, its baseline contains
  `never`, and the staged scripts pass preflight. The full `OneiosScriptTest` Gradle task passes, including an
  annotated-line restore test and an already-restored/no-write test. **Proven at the real return on 2026-11-16:**
  `shmem_enabled back: never`, Gate A passed, Gates B/C passed and One UI restored in place; `return.outcome` is
  `inplace 1794844068 ...`, not a failsafe/reboot.
- The first successful boot ended xnuhost without its normal `exit N` or guest-panic lines during interaction; that
  stop's cause is not established (it did not reboot the phone, and the return repaired One UI in place). A second
  switch on the identical collection and image reached ready again. Live GPU logging showed Metal clients opening
  successfully and, in one 10 s interaction window, 127 submits, 103 fenced, 72 frames at 7,952 us mean, no fence
  timeouts and no refusals; backboardd's submit-to-retire mean was 5,430 us. The reported "no Metal" slowdown was therefore
  not reproduced by that second boot. Before/after logs: `phonefix-xnuhost-end-full.log`, `phonefix-2/live.log`.
  Keep watching for the unlogged host-side exit; do not mislabel it as a guest panic.
- The complete oneios-tool release workspace now builds on macOS; `oneios-common` passes 16 tests. It staged the
  kernel/config and the fork scripts, checked the image, started the session and read its logs. `ioslog health` against
  the new Data store remains for the post-return checks.

## Resumed 2026-11-16 (the Linux PC): the merge day's last PRs landed

The 2026-11-15 stop ended on a provider rate limit; the work resumed here the next day and the three open merge-day
PRs are in. Nothing in this section has run on the phone.

- **#49 merged** (`99617ef`): `oneios-tool` ships in the runtime package (`bin/oneios-tool` + its `.sha256`,
  `check-phone-tool.sh` in CI; the release job still has not run, no tag), and `oneos/interface.md` 9 is the
  fork-tool contract (pause, resume, net).
- **#61 merged** (`1ef92a8`): the tool's downloads for a mobile network. `pause`/`resume` requests, the `net` event
  (`waiting`/`resumed`, the seconds to the next try), tries with backoff and jitter, parallel range connections
  (`connections` and `give_up_s` request keys), a paused or resumed download keeps its part and its state file for a
  later run (SHA-256 and CRC-32 checked), `failed` codes `network` and `changed`. Serve tests 21 -> 27 (7 of them
  downloads on a bad network); 153 cargo tests and 14 phone stand-in checks green on the PC before the push. Its
  first CI run failed `runtime/bridges/battery` (2 of 83 checks): a runner flake, not the PR (the battery bridge is
  untouched by it, 83/83 passed locally, the re-run passed). The branch's last round (the finished "Downloads on a
  mobile network", 360 lines) sat uncommitted in its agent's worktree after the agent died on the rate limit; it was
  committed as `8d21547` after a local build and test.
- **OneOS #10 merged** (`bb1ef38`): the app's side of the tool. Pause/resume/net UI as the tool's README has them,
  POST_NOTIFICATIONS asked once (Android 13+), the `dataSync` foreground-service type (tests assert both),
  KernelSU's `su -c` runs the staged `bin/oneios-tool` (its path and hash from `runtime.sha256`), cancel goes to the
  tool as a request (the tool is root's, out of the app's reach), a flow resumed after the app was ended. Proven by
  CI-test **#63** (`app/` at `e38648f`): all 5 checks, Linux and macOS gradle.
- **This commit**: main's `app/` pin moves to `bb1ef38`.

**Decided 2026-11-16 (the owner: "do those"):**
- The root tweaks to Rust (`tweaks`), `build.sh`'s order and checks (`make-image`) and the AEA decryption: **started;
  blocked on agents**. The Fable 5.2 agent was launched twice and died both times on the Claude provider's rate limit
  (Opus is on the same limit; it resets 2026-11-21 2:30am IST). It waits for a working agent slot, or the owner says
  the coordinator ports it directly.
- The two caps: **the uclamp cap is dropped** (this commit): a background or utility hinted thread gets no floor and
  no ceiling of ours (the 512 ceiling of fix-host-overhead slowed chronod's widget extensions, #57). The background
  GPU class stays as fix-gpu-widgets narrowed it.
- `shmem_enabled=advise`: **merged** (OneOS #11, `d189691`; the fork's CI built the APK with it): the switch captures the mode
  with One UI whole and writes `advise` for the 2 MiB host pages (One UI never advises, so it changes nothing for One
  UI), the return writes the old value back before gate A and gate A failsafes if it did not; OneiosScriptTest covers
  the restore and the untouched case.
- The empty `app/`: **stays as #46 has it**: passed over with the note. A fresh worktree's empty `app/` must fall
  through to the main checkout's `app/` (that is how an agent worktree finds the fork); a path named by hand already
  stops with the fix command.
- The token: **pushes fire the fork's CI** (the `oneios` branch's run for `bb1ef38` ran green 2026-11-16, the
  prerelease APK built); only a hand `workflow run` dispatch is refused (HTTP 403: the PAT has no Actions: write). Add
  it only if hand dispatch is wanted.
- **The version is automatic** (the owner, 2026-11-16): the app's versionCode/versionName read the fork's git
  history (`build.gradle.kts`, the commits since OneiOS 0.1.10's fork commit; the fork's Version step reads the
  same), so a release needs only its tag — the CI caches the NDK, Rust and cargo between runs (`docs/releases.md`).
  v0.1.11 is the first release to ride it. The pin push of `470d71f` recorded the old `app/` head (its submodule
  checkout lacked the fetch and the pipe to `tail` hid the failure; its `v0.1.11` run dies at the deleted tag's
  checkout and no release comes from it) — this pin is the real `68d48b1`.
- **Other phones: warned, not refused** (the owner, 2026-11-16): the switch's two model refusals are one warning
  block that goes on; the app's gate warns until the confirm's third box (`oneios_not_this_phone_ack`); the setup
  flows as usual. **Merged** (OneOS #12, `e5004eb`; its CI's one test needed the script text joined — `68d48b1`, the
  head this pin takes, also has the Version step reading the git count).
- **Tag `v0.1.10` is cut** (the release job's first run ever): plan, runtime, apk and apk-macos all green — the
  runtime package, `bin/oneios-tool` (14,854,456 bytes) and the fork's APK (0.5.14, code 143, at `bb1ef38`) all
  built; the release job itself failed and published nothing: its checksums step `cd out/release` and wrote
  `../release-files.txt` (into `out/`), the Release step read it from the root (#65 fixed it, merged). The tag had
  made no release, so it is deleted and made again at the fixed main, and the release comes out.

The tool's README ("Downloads on a mobile network") lists what a phone run must still measure: a lookup and a
connect on a phone without a network, and the stops. Still waiting on the phone (it is on the Mac): the real-device
proofs (a pause and a resume mid-download, airplane mode mid-download, the screens opening under KernelSU), and the
8644ae9 and 8644ae9-b session logs copied to this box.

## Stopped 2026-11-15 (evening): what is done, what is open, what is next

The owner stopped all work on 2026-11-15. Every agent pushed its branch; finished and CI-green work is merged,
half-done work stays in **draft PRs, not merged**. Nothing in this section has run on the phone unless it says so.
The last phone sessions were 8644ae9 and 8644ae9-b (Mac: `out/session-logs/2026-11-15/`; **not on the Linux PC**:
copy them over before the next round, several open items need iOS's log from them).

### Merged on 2026-11-15 (main)

| PR | What | Proven where |
|---|---|---|
| #37 | Wi-Fi scan reuse (8 s), POWER_CHANGED only on a change, per-reader SCAN_RESULT cursor, `ONEIOS WIFI: 60 s:` line | PC (scans 20 -> 5 a minute); phone 8644ae9: scans reused, Preferences' CPU **not** lower (the lag was hapticd, below) |
| #39 | Fixed, locked RAM by default (`ram-giveback=0 ram-prefault=1 ram-lock=1`), the fit with 768 MiB headroom, the valve on the main loop | PC (no lock there); phone: not yet run with `ram=auto` (8644ae9 ran `ram=10G`, unlocked, and swapped) |
| #45 | 2 MiB host pages possible (memfd offset 0 on a 2 MiB boundary), `config.ram_lock_fit`, page-table lines. **No RAM gain**: the 575 MiB of page tables are the iOS tasks', not xnuhost's | PC boot to SpringBoard; the phone needs `shmem_enabled=advise` (the fork's switch, not done) |
| #46 | `oneios-tool app` finds the fork at `app/` first (after `--oneos`, `ONEOS_DIR`); `app dir` | cargo 134, stand-in suites |
| #47 | GPU: no name list for what the user sees, background at KGSL priority 8, no lowest level while others wait, `gpu: 10 s fences over 100 ms` per process | `test_gpu_policy` model; not on the phone. Drop `gpu-priority=0` from xnuhost.conf with it |
| #48 | HEVC/HEIC encoder, host side (`runtime/bridges/encoder`, `encoder=auto`, off by default) | host tests; four faults in it are fixed in draft #59 |
| #52 | `oneios-tool serve` (one root process per flow, JSON lines, cancel, idle exit), `--jobs`/`--nice`, the thermal/battery stop, local `sums`/`log`/`cpu` | cargo 175, suites; Android build 14.6 MB, not run on the phone |
| #53 | `oneios-tool ioslog health` (hangs, watchdog kills, XPC timeouts, jetsam, crashes, encode failures), written by `cycle --only collect` | cargo 142; not checked against 8644ae9-b (on the Mac) |
| OneOS #6 | the switch's shell moves to the root cpuset (all 8 cores) | not on the phone |
| #51 | Audio latency: speaker LOW_LATENCY with MMAP exclusive first (no RT thread of ours), speaker ring 1920 frames (iOS's I/O buffer <= 720), the measured output latency told to iOS, the ring-wrap overrun fixed for every device, `end-to-end N ms` line, `bridgectl audio-loop` | host tests and CI; predicted ~230 -> ~40-45 ms iOS-mix-to-speaker; not on the phone (does its HAL give MMAP exclusive?) |
| #43 | One workflow (`release.yml`; `ci.yml` folded in): `plan` -> `runtime`, `apk`, `apk-macos` in parallel -> `release` (needs runtime + apk). The fork as submodule `app/` (`branch = oneios`, built at its head with `--remote`); `check-apk.sh` refuses set files, firmware/image names and entries over 64 MiB | CI; **the release job has not run yet** (no tag or dispatch since) |
| OneOS #8, #9 (#7 in #9) | the fork's CI doc (its own `oneios-apk.yml` stays for pushes to `oneios`); core:root tests pass on macOS (PATH for `direct()`, `LC_ALL=C`, base64/cat stand-ins, a fake clock without `%N`) | macOS runner: 270 core:root tests, 2 skipped (run 43722560928) |

### Open, not merged (draft or failing): where each stands and its next step

| PR | Branch | Done | Next step |
|---|---|---|---|
| #51 | `fix-audio-latency` | Audio ~230 ms -> ~40 ms predicted: MMAP exclusive low latency (no RT thread of ours), speaker ring 1920 (iOS I/O <= 720 frames), measured latency told to iOS, ring-wrap overrun fixed; `end-to-end N ms` line; kernel built, audit 0 | CI fails `test_audio_helper.c:738-739` (the target's settle after a late guest) since the start-gap fix 78c186f; fix that, then a phone session |
| #55 | `fix-mach-rpc` | IPIs and the timer/IPI interrupt by host calls instead of trapped GIC registers (hosted **0076**); bench round trip 485 -> 184 us; `rpc:`/`wake:`/`syscalls:` lines | **renumber the patch** (0076 is also #54's), build the committed patch, a PC boot with `oneios-fast-irq=2` (A/B in one boot), then the phone |
| #56 | `fix-wifi-late-enable` | Cause from configd/bootp source: only IPConfiguration sets IFF_UP, after every configd plugin loads; the driver now sets IFF_UP 50 ms after en0 attaches (`oneios-wifi-up=0` off); per-disk flush line | a PC boot `wifi=sim`; why configd itself starts at +125 s still needs 8644ae9's log. Its disk line overlaps #57's: keep one |
| #57 | `fix-widget-kill-loop` | Disk flushes (fdatasync of data.img) moved off the vCPU onto the disk's thread (`VIRTIO_BLK_F_LATE_FLUSH`, `blk-late-flush=0` off); `disk N: 60 s:` and `tasks: pid N took T ms to die` lines; kernel built, audit 0 | a PC boot to the widgets (none has reached them yet), then 8644ae9-b's iOS log for what each killed extension waited on |
| #58 | `fix-hapticd` | **Cause proven** (iOS's log): hapticd loads `/Library/Audio/Tunings/<model>/Haptics/hapticengineconfig.plist`, the model is `oneios,xnuhost`, no file, the server bails, every client waits for its XPC timeout (the keyboard lag). Hosted **0078** links `oneios,xnuhost -> V54` in a read-only union over `/Library/Audio/Tunings` (`oneios-audio-tunings=0` off); kernel built, audit 0 | the PC boot was killed at 10 s (a panic at the kill, before 0078 runs; not proven unrelated): boot B/A with `keyboard-typing.script`, then the phone (a key tap felt, no `Initial XPC call to server timed out`) |
| #59 | `hevc-guest` | `kernel gen codec`, the codec archive in `kernel.kc` (audit 0, stubs 666), four host-device faults of #48 fixed, a decodable PC stand-in, `encoder=sim`; PC: `VideoToolbox paravirtualization registered` | a PC boot to the home screen with and without `encoder=auto` (the PC may never reach a widget snapshot), then the phone: `encctl hevc`, a session with `encoder=auto` |
| #54 | `fix-ios-compressor` | **Read in the code:** iOS's compressor is on, as on an iPhone18,2 (mode 32); the pageout daemon takes file pages first, so no PC boot compressed anything (stock behaviour). Fixed: `target-type` V54 (runningboardd found no jetsam properties), `kern.max_task_pmem` 3376 MB; hosted **0076** lines `oneios-mem: compressor: ... ratio R` and `jetsam: ... kills since boot`; `ram-view=SIZE|auto|off` (off; `auto` 12 GiB via `memSizeActual`, scaled limits, `compressor_pool_multiplier`). `hw.memsize` cannot exceed the RAM (`max_mem = boot_args.memSize`) | the committed 0076 (with `oneios-compress-sweep`) was never built; build it, a PC boot to check no `Can't find jetsamproperties`, a sweep boot for a measured ratio, then the phone with `ram=auto ram-view=auto`. **0076 clashes with #55**: renumber one |
| #60 | `fix-dyld-objc` | The DYLD shared-region unnest is ours: `vm_map_protect` unnests before checking for a change, and dyld toggles its protected memory ~90 times per process; `pmap_protect` flushed (unmapped) its whole range even when nothing changed. Hosted **0080**: flush only changed entries (`oneios-protect-flush-all=1` old), a no-change protect of the nested region stays nested (`oneios-sr-protect-noop=0` old); `oneios-top=N` per-process lines. The objc, `findImageMappedAt` and image-insert samples are iOS's normal paths (read, not measured) | its first PC boot **halted 1 s in** (after `VM bootstrap`, exit 4): find that first (`out/run-dyo/before/boot.log` in its worktree), then before/after boots with `input-typing.script` |
| #49 | `app-oneios-tool` | `runtime/package.sh` builds `oneios-tool` for Android arm64 into the package (`bin/oneios-tool`, 14.7 MB) and beside it with its `.sha256`; `check-phone-tool.sh`; the release uploads it. Why the package, not the APK: root runs only the staged runtime's hashed `bin/` files, and the tool updates with the runtime | **conflicts with main** after #43 and #51: merge main, rerun CI (its APK job failed on the old `check-apk.sh` of #43's earlier commit) |
| OneOS #10 | `app-oneios-tool` | The app's side: `oneios_root.sh tool work\|read` and `untool`; `OneiosToolSession` (one `su` + one `serve` per flow, cancel, close, no process left), `OneiosToolJob`/`Activity` in the foreground service: Make iOS image, Check every file (`sums`), CPU, session logs, iOS's log. Linux CI `./gradlew test assembleFullDebug` passed | half done: Make iOS image only shows the tool's refusals (no `make-image`, `tweaks` not ported); the screens never opened; macOS not proven; nothing on the phone. Needs from the tool: `make-image`, the tweaks in Rust, build.sh's order and the AEA decryption, `receive` from a FIFO |

### Owner decisions waiting

- Port the root tweaks (`userland/roottweak`, Python) to Rust, with a byte-for-byte seed comparison? Without it, plus `scripts/image/build.sh`'s steps and the AEA decryption in the tool, the app's "Make iOS image" cannot run fully on the phone (`serve` answers `tweaks` with `not-ported`).
- The two caps against "no caps of ours on the GPU / all cores": #40's `uclamp.max` 512 for background-hinted threads (it may also slow chronod's widget extensions, #57), and (fixed in #47) the background GPU class.
- The fork's switch writing `shmem_enabled=advise` (restored at the return) so that #45's 2 MiB pages take effect.
- An empty `app/` (submodule not initialised): skipped with a note (#46, as now), or a hard stop?
- The token: rerunning CI needs Actions: Read and write.

### The next phone session

- `ram=auto` (not `ram=10G`: it swapped up to 17.7 MB/s at SoC 76-86 C); with #54, `ram-view=auto` for the 12 GB view.
- Remove `gpu-priority=0` (fixed in #47).
- With #58 in: keyboard haptics on; check `Initial XPC call to server timed out` is gone and a tap is felt.
- Read `oneios-tool ioslog health` of the session (#53) for hangs, watchdog kills and XPC timeouts before and after.

Closed, CI-test only: #44 (proved fork #9 on macOS), #50 (fork #10's branch; its APK job failed).

**Hosted patch numbers in the open PRs:** #54 and #55 both use **0076**: renumber one before the second merges.
#58 uses 0078, #60 uses 0080. #56 and #57 both add a per-disk flush log line to xnuhost's disk code: keep one.

**Housekeeping:** the main checkout's `.git/config` got `submodule.app.url` and `submodule.app.active` from
`git submodule add` (#43): correct now that `app/` is on main. Each agent's worktree is under `.claude/worktrees/`.

## The next phone session (2026-11-15, main 70af164 and OneOS oneios 820253b): one `xnuhost.conf`

Everything below is on `main` (and the fork's `oneios`), built and host-tested, **not run on the phone yet**. Build
the runtime and the APK from these two, then:

- **Remove** `owner-hide-vmm=bluetoothd,BlueTool` and `bt-transport-override=10` (both proven harmful: item #32 and
  "The virtual transport is bluetoothd's own", `docs/hardware.md` 4). xnuhost now logs an error line for either.
- **RAM:** `ram=auto` alone. Since #39 (fix-ram-locked) the default is `ram-giveback=0 ram-prefault=1 ram-lock=1`, and
  `ram=auto` takes (MemAvailable − 768 MiB − 256 MiB) / 1.08 when the lock can work: about 6912 MiB on this phone.
  Writing `ram-giveback=0 ram-prefault=1 ram-lock=1` out does the same. Lines that prove it: `CAP_IPC_LOCK yes`,
  `the guest gets 6912 MiB ... less 768 MiB headroom with the lock`, `memory: fixed RAM: held and locked`,
  `oneios-mem: free pages given back to the host: no`, no `the host swaps iOS's RAM` line, no `unlocked for the rest
  of the session`, swap in/out near 0 in `thermal: 30 s:`, `stopping:` right after the return's SIGTERM.
- **CPU:** OneOS #6 moves the switch's `--inner` shell (and so xnuhost) into the root cpuset: the switch log should
  say `oneios inner: cpuset / (cpus 0-7)`. `oneios-tool phone cpu` (read only) shows the cores, clocks, cpusets and
  xnuhost's threads with their uclamps; `phone cpu --trace 10` while iOS is busy. Hosted CLPC (0075) and the uclamp of
  released iOS threads are on by default (`docs/booter.md`).
- **GPU:** the default `gpu-level=deadline`. One A/B against `gpu-level=hold` if frames feel late; `gpu-priority=0`
  turns the per-context priorities off (`docs/gpu.md` 2.2b).
- **Wi-Fi:** `oneios-log-persist=com.apple.WiFiManager` (a file in the kept Data store: delete it later). In the
  session tap Control Center's Wi-Fi button once to turn auto-join back on (`docs/hardware.md` 3). Read the
  `ONEIOS WIFI: 60 s:` lines (#37): `started` about one per 8 s or fewer while locationd scans, `events 10xN` = scans
  asked − refused; and Preferences' `oneios-cpu:` with the password sheet open (was 106-111 %).
- **Buttons:** `oneios-hid-buttons-builtin=1 oneios-button-haptics=1` (boot B in `docs/hardware.md` 5), with
  `oneios-ubt=SpringBoard,backboardd,configd,wifid oneios-ubt-at=240 oneios-ubt-every=60 oneios-ubt-count=150`.
- **Optional:** `pen=auto` (S Pen as Apple Pencil, `docs/hardware.md` 7f); `hw-backend=treble` for one session to try
  the GSI/Treble paths (samsung stays the default; `hw-backend-<bridge>=` per bridge, `docs/hardware.md` 7e).
- In the session: Safari (WebContent must stay up: no new `.ips` with `sys_icache_invalidate`), the brightness slider,
  lock and wake with the power key, Bluetooth on, the Wi-Fi password sheet.

**Before it, on the Mac:** `./gradlew test assembleFullDebug` on the fork's `oneios` (core:root: 245 tests, 0 failed,
2 skipped expected) and OneOS #5's click-through. **The owner:** start the fork's "OneOS APK (oneios)" workflow by hand
(it has not run for a push since 2026-11-12); rerunning CI needs the token's Actions: Read and write.

## Update 2026-11-15 (fix-gpu-perf): the GPU by deadline and by process, and the background storms

Built and host-tested on the Linux PC, **not run on the phone**. Details: `docs/gpu.md` 2.2b.

- **The GPU clock (the phone's `context 4: power constraint at the highest level`, `clock hold: the top level from the
  render server's submissions to 150 ms after them`).** Cause: drm2kgsl 0005/0006 put every render-server submission at
  KGSL's top level and held it 150 ms after each, and any client within a second extended it. Fix: xnuhost decides the
  level per frame from its deadline (the work interval's, when the CPU agent's signal is there; else the cadence, 8.3 ms
  at 120 Hz) and the frames' own GPU time from KGSL's timestamps: the top only for a frame that would miss at the
  governor's clock, the governor's otherwise, background contexts at KGSL's lowest level at a still screen. drm2kgsl
  patch 0008; `gpu-level=hold` is the old rule. The PC's model of it (`test_gpu_policy`): time held at the top, before ->
  after: a still lock screen redrawing 100 % -> 3 %, typing 100 % -> 2 %, a light animation 100 % -> 3 %, a 120 Hz scroll of
  3 ms frames 100 % -> 100 %, no frame later than before.
- **Per-context priority.** Each queue's KGSL priority is its owner's class (render server and front 4, default and
  background 8 since fix-gpu-widgets; background was 12), from the name (`xnu:backboardd`; a name list of six daemons that
  never draw what the user sees: photoanalysisd, mediaanalysisd, assetsd, searchd, analyticsd, suggestd), whether the
  render server reads the context's surfaces (then never background), and the guest's hint (the task's RunningBoard role,
  DarwinBG, the thread's QoS: `oneios_gpu_hint` in the GPU archive, sent in CTX_CREATE and every fenced SUBMIT_3D).
  `gpu-priority=0` turns it off.
- **Widgets' GPU starvation (fix-gpu-widgets, 2026-11-15; phone, 8644ae9 with v0.1.9).** WidgetRenderer_Default was
  background by the name list (KGSL priority 12, the lowest level at a still screen) although the guest said foreground:
  `other fences 85, mean 181112 us`, `fence timeouts +5`. Fixed in xnuhost alone (`docs/gpu.md` 2.2b, "The widgets' GPU
  starvation"). The white widgets themselves are chronod's snapshots (no HEVC encoder: `fix-widget-snapshots`). Lines
  to read on the next session: `gpu: context N xnu:WidgetRenderer_Default: default` (or `... the render server reads its
  surfaces: default now, not background`), `drm2kgsl: queue N of xnu:WidgetRenderer_Default: default, KGSL priority 8`,
  `gpu: 10 s host: ... other fences N, mean` under 10000 us, `oneios-gpu-report: ... fence timeouts +0`, and the new
  `gpu: 10 s fences over 100 ms: ...` line absent or naming only a listed daemon.
- **The storms (iconservicesagent 89-98 %, PosterBoard 48 %, MercuryPosterExtension 38 %, assetsd 37 %, searchd,
  analyticsd): what is ours, traced in the code.** No session folder was on this PC, so each candidate was read in the
  code, not in the session's lines:
  - Brightness: every request re-published `IODisplayParameters`/`brightness-nits` and sent the bridge a level, also the
    same level again (a drag: ~550 a second, each restarting the host's fade). **Ours, fixed**: only a change goes on
    (`drivers/brightness`, `docs/hardware.md` 7a). `setProperty` sends no IOKit notification, so this did not by itself
    make iOS redraw.
  - The display's mode, scale, layers and colour: set once at the display's start (`NativeLegacyDisplay::publishOneLayer`,
    the probe's `/chosen display-scale`; the colour mode is fixed for the boot by `display-format=`); the 120 Hz vsync
    tick runs only while iOS has a vsync client registered (Apple's own registration). Not a re-publish.
  - The wallpaper's inputs: the EDR headroom is held at 1 by the plug-in (`debug_values[21]`), the backlight publishes no
    `MaxNitsEDR`: nothing of ours changes them during a session.
  - Battery: a change message only when a value changes, temperature, voltage and current at most every 30 s
    (`br_battery_changed`, `oba_publish`). Thermal: `kIOMessageServicePropertyChange` only at a new level or step.
  - The GPU's device properties: set once at the accelerator's registration (`DroidVMGpu.cpp`).
  - IOSurface and memory pressure: the plug-in drops its imports at XNU's own pressure rises; nothing of ours raises the
    pressure level by itself (0058 gives back free pages that stay counted free; `ram-giveback=0` is the owner's rule).
  - Repeated Metal device or context creation: one device per process by Metal's discovery [I]; xnuhost now logs one
    `gpu: context N xnu:NAME: CLASS ...` line per context, so a process making contexts again and again shows on the phone.
  - So, from the code, the storm processes' CPU is iOS's own work (first-boot icon drawing, the photo library's and
    Spotlight's indexing, analytics) [I: not proven without the session's lines]. What is ours now: their GPU work runs at
    KGSL priority 12, never holds the clock, and at the lowest level once the screen is still (since fix-gpu-widgets:
    priority 8, and only assetsd, searchd and analyticsd of them stay background by name; iconservicesagent and the
    posters are classed by role and by whether the render server reads their surfaces). Their CPU placement is
    the CPU agent's.
- **Tests:** `make -C runtime/xnuhost test` (the new `test_gpu_policy` 116 checks; `test_xnuhost` with the hint checks;
  `test_drm2kgsl`'s new priority scenario built, skipped here: `/dev/udmabuf` is root:kvm), `drivers/tests/host.sh`
  (`test_bl_request` 2044), `cargo test -p oneios-kernel --test golden` (the staged hint edits), the hosted kernel
  `HOSTED_OUT=fixgpu` built (800 s) and prelinked (audit 0 failures, stubs 666 good, `kernel.kc` 49f718cb), a PC boot
  to SpringBoard (+364 s) and 10 minutes with no panic (no 3D under qemu: the hint and the level did not run). The Metal plug-in is not changed (the hint is
  the kernel's), so the Mac builds nothing new for this.
- **The phone test (the Mac).** The runtime and `kernel.kc` of this branch, the image's `xnuhost.conf` as the baseline
  (`gpu-level` and `gpu-priority` at their defaults). In the session: the lock screen still for a minute, unlock, the
  home screen still for a minute, scroll Settings for 30 s, type in a text field for 30 s, open and close three apps, then
  return. Then one session with `gpu-level=hold` for the same steps (the before). Read and compare:
  - xnuhost's log: `gpu: level:` (once: the clocks and the mode), `drm2kgsl: clock hold: ... only for frames that need
    it`, `gpu: context N xnu:NAME: CLASS` (every context's class and priority; since fix-gpu-widgets assetsd, searchd,
    analyticsd background 8, WidgetRenderer and the posters not background), `drm2kgsl: queue N of xnu:NAME: CLASS, KGSL priority P` (no
    `refused` line), `gpu: 10 s level:` (frames, at the top, missed, over budget, background at the lowest), `gpu: 10 s
    host:` (`clock held +N times, keep-alives +M`: far fewer at a still screen than with `hold`), `drm2kgsl: context N:
    power constraint at the lowest level`, `ONEIOS BACKLIGHT: N requests repeated the level last sent` after a slider
    drag, `thermal: 30 s:` (SoC and battery temperature), `oneios-cpu:` (the storm processes' share), the guest's
    `oneios-gpu-report:` (frames' `<8/<16/<32 ms` buckets while scrolling and typing).
  - GPU busy and clock, as root from One UI's side during each step (read-only nodes, no writes):
    `cat /sys/class/kgsl/kgsl-3d0/gpubusy /sys/class/kgsl/kgsl-3d0/gpuclk` once a second (a loop in `adb shell su -c`),
    and `/sys/class/kgsl/kgsl-3d0/gpu_available_frequencies` once.
  - Expect: at a still screen `gpuclk` at the governor's low levels (not the top) and `at the top` near 0 in the 10 s
    line; while scrolling the top and `over budget` near 0; the SoC cooler than the `hold` session at the same steps.
    `missed at the governor's` above a few per 10 s while typing would mean the model's ratio is too optimistic there.

## Update 2026-11-14 (the 9f6dc38 session on the phone, and the round that followed)

**Proven on the phone** (main 9f6dc38, 2026-11-14): first frame at +135 s, no panic; USB adb held the whole boot
(`usb: ... UDC a600000.dwc3 configured`); #22's scan TIMESTAMP and 63-channel list in use; #25's chunked
`pull-data` over USB (128 chunks, 11 copied, 0 retried, 271 s); #13's ioslog on the copy (checkpoint xid 2350).
Found there and fixed since (all on `main`, built and host-tested, **not run on the phone yet**):

| PR | Fault | Fix |
|---|---|---|
| f7bb1c3, #28 | the display helper's `.so` lacked libnativewindow (panel stayed on the OneOS splash) | `-lnativewindow`; `-Wl,-z,defs` on every Android `.so`, and `runtime/package/elf-check.sh` (NEEDED and symbols against the NDK's API 30 stubs) in the build, the package and CI |
| #31 | no CLI for the session's by-hand fixes | `phone display-restart [--lib --dex]`, `phone touch status\|wake`, `phone backlight status\|set N` |
| #29 | the slider reads 0 (IOKit truncates floats); a brightness of 0 slept the touch IC | the slider's 16.16 `DisplayBrightnessSliderPosition` and `brightness-nits-cap` read; hosted **0069** passes iOS's real display power (`IOMobileFramebufferRequestPowerChange`) so the IC sleeps only at a real off and wakes at every on and power-key press; the bridge's close always writes One UI's level |
| #26 | the power key does nothing (LockButton reaches UISensor, then nothing) | off by default: `oneios-hid-buttons-builtin=1` (buttons published Built-In, Consumer Control: backboardd listed them as an external keyboard), `oneios-button-haptics=1` (a stand-in `AppleSPUHapticsAudioDriver`) |
| #30 | the host swapped 2.1 GiB of iOS's RAM (Settings watchdog-killed, heat) | `ram=auto` fits: (MemAvailable at start − 1 GiB − 256 MiB) / 1.08; hosted **0067** in the series; the warning `the host swaps iOS's RAM: N MiB ...`; `ram-giveback=0` stays opt-in (the default since fix-ram-locked, 2026-11-15: item 5) |
| #27 | "it runs damn hot" | a `thermal: 30 s:` line (host CPU, swap rates, battery temperature and current, xnuhost's CPU); `oneios-cpu-sys:` kernel time per process |
| #33 | every WebContent process died in `sys_icache_invalidate` (a cache-maintenance fault taken as a write) | the stub and hosted **0071**: a fault with ISS.CM is a read; `test_jit_icache` |
| #34 | Wi-Fi never joins: auto-join "disabled by user" (a client's request, likely Control Center's Wi-Fi button) | PHY_MODE and AGE in the scan record; hosted **0070** `oneios-log-persist=` keeps a subsystem's info-level log |
| #37 (3233fed session) | Preferences at 106-111 % with the Wi-Fi password sheet open; auto-join still off | the driver posts POWER_CHANGED only on a change, refuses a scan with no radio instead of an empty SCAN_DONE, keeps a SCAN_RESULT cursor per reader, answers a scan within 8 s of the last good one from its records (locationd asked for two scans every 6 s on the PC, location off, no fix); a `ONEIOS WIFI: 60 s:` rate line. Auto-join: tap Control Center's Wi-Fi button once (`docs/hardware.md` 3) |
| #32 | bluetoothd crash-loops with `owner-hide-vmm` (proven: the hide caused it) | the hide dropped; 0062 logs `kern.hv_vmm_present` reads; xnuhost warns if the boot arguments hide the VM from bluetoothd |
| OneOS #2 | an old session's `display.log` could end the boot screen early | the switch moves the old log aside |

Integration of #26-#34 (one tree): kernel `xnu.macho` fe21f390, prelink audit 0 failures (2711 paired),
`stubs-check` 666 good, `kernel.kc` b37e1e88; `runtime/host-tests.sh` every part (links included); `cargo test`
122 passed; stand-in phone 200, app 65, cycle 103 in both /proc modes.

**The next phone session** (build from `main`), `xnuhost.conf`:
- Remove `owner-hide-vmm=bluetoothd,BlueTool`. Do not add `bt-transport-override=10`: tried in the 3233fed session,
  it is bluetoothd's own simulated controller inside its own process (nothing reaches ours; exit(1) every ~56 s;
  `docs/hardware.md` 4, "The virtual transport is bluetoothd's own"); xnuhost warns about it since fix-bt-virtual.
- `ram=auto` (it now fits; or keep `ram=6G`); `ram-giveback` at its default first, then one session with
  `ram-giveback=0` (look for `CAP_IPC_LOCK yes`, `locked (mlock: the host cannot swap it)`, no `the host swaps iOS's
  RAM` line, and `stopping:` right after the return's SIGTERM). (2026-11-15: the 3233fed session ran the default and
  Linux swapped iOS's RAM; since fix-ram-locked `ram-giveback=0` with the lock is the default: item 5.)
- `oneios-log-persist=com.apple.WiFiManager` (writes a file in the kept Data store: delete it later).
- `oneios-hid-buttons-builtin=1 oneios-button-haptics=1` for the power key (boot B in `docs/hardware.md` 5; split
  them if it works), with `oneios-ubt=SpringBoard,backboardd,configd,wifid oneios-ubt-at=240 oneios-ubt-every=60
  oneios-ubt-count=150`.
- In the session: re-enable Wi-Fi auto-join (Control Center's Wi-Fi button again, or tap a network), open Safari,
  drag the brightness slider, lock and wake with the power key, flip Bluetooth.
- Read: the `thermal: 30 s` and `oneios-cpu-sys:` lines; `ONEIOS BACKLIGHT: brightness-nits-cap`, `... display off:
  backboardd[N] asked its framebuffer`; `oneios-hid: buttons: Consumer Control, Built-In published`; the swap line;
  WebContent staying up (no new `.ips` with `sys_icache_invalidate`); the iOS-log searches of `docs/hardware.md` 3-5
  and 7a.

**Waits for the owner:** the fork's CI never ran after OneOS #1 (start "OneOS APK (oneios)" by hand, or check that
Actions is on for the fork); rerunning a failed CI job needs the token's Actions: Read and write.

## State at the pause

- `main` holds everything below. Every open branch was merged in one integration tree, its kernel was built and
  prelinked once (audit 0 failures, stubs 666 good), and the runtime host tests ran on it, then it went to `main`.
- **Off by default on `main`** (merged so that nothing is lost, inert until turned on): the fixed RAM
  (`ram-giveback=0`), the graceful return (`return-graceful-ms=`), the wide display path (`display-format=rgba16f`),
  the bigger HID queue (`oneios-hid-queue-kib=`), the GPU read wait (`oneios-gpu-read-wait=1`), the stand-in
  actuator (`haptics=sim`), the fork's USB repairs (the file `/data/local/tmp/oneios/usb-repair`), AAudio's low
  latency (`ONEIOS_AUDIO_PERF=low`; since 2026-11-15 LOW_LATENCY and MMAP exclusive are the default, `ONEIOS_AUDIO_PERF=none` the old path).
- **On by default and new on `main`, none of it run on the phone yet:** the audio helper's lower latency (#21),
  `pull-data` in chunks with resume and per-chunk sums (#25), the Wi-Fi scan TIMESTAMP and channel list (#22),
  xnuhost's `usb:` lines (#24), the haptics, widget and display counters (#16, #17, #15), sRGB-tagged frames (#15).
- **The phone:** on the Mac's side; last session `main` `c2d281a` (kernel.kc `1be6e0c9`, runtime 0.1.9-dev
  `b2b59858`), the Data store kept (do not stage a new seed: a new seed starts the store over). Root is not
  persistent: after a restart the owner roots it again.
- **Machines:** the Linux PC has the NDK, Rust, libdispatch (kernel builds), qemu-aarch64, apfs-fuse, the IPSW in
  `inputs/24A446`, a PC image set in `out/image-pc/24A446`. It has no Xcode (iOS-side code builds on the Mac only)
  and no phone. Box-wide queues on the PC: kernel builds through `flock /tmp/oneios-kbuild.lock`, PC boots through
  `flock /tmp/oneios-qemu.lock` with `--ram 4G` or less.

## Done on 2026-11-12/13 (all on `main`)

| PR | What |
|---|---|
| #1, #2 | the Rust port: `oneios-tool` (sym, log, image, ioslog, prelink; later phone, app, cycle) |
| #3 | `sym kernel` and `log panic`: kernel panic frames named (per-segment kc→xnu.macho, physical aperture, DWARF) |
| #4, #11 | the kernel patch generators, `build-record`, drivers, prelink helpers, package check, image pack/verify/fetch, rangeserve in Rust; the Python retired (1:1, goldens). CLAUDE.md: no new Python |
| #5 | WebKit's crash loop: diagnosis (the CODE SIGNING line is normal; a919ff3 turned a kernel panic into a per-process crash) and a JIT test |
| #6, #13, #25 | the sparse copy; ioslog reads a used Data store (checkpoint fallback, block checks); `pull-data` in chunks with resume |
| #7 | battery: the first state carries no change message (the chime's real cause is elsewhere: item 4) |
| #8, #10, #22 | Wi-Fi: a reserved unit for en0, the DHCP log fix, 0061 `oneios-ubt-count`, the scan TIMESTAMP at 0x8d0 and CHANNELS_INFO |
| #9, #20 | Bluetooth: 0062 IOKit probe trace, `bluetooth=sim`, the reading of bluetoothd's VM path |
| #12 | 0060: the lsd recovery file a killed session left is removed |
| #14 | 0066: free-page runs; the thermal step no longer stacks P vCPUs; the GPU memory counters |
| #15-#25 | the paused faults below (each partly done, safe, most of it off by default) |
| OneOS #1 | the fork: USB state recorded at the switch, a USB watcher in the session, restored at the return |

## Open work, in order (the detail of each is in the doc it names, and in "Fault detail" below)

1. **The next phone boot** (one session answers most of the items below). Build from `main`. Boot arguments, in
   the image's `xnuhost.conf`, beside the default ones:
   `oneios-ubt=configd,wifid,logd,Spotlight,kbd,backboardd,Preferences,InputUI oneios-ubt-at=20 oneios-ubt-every=10
   oneios-ubt-count=150 oneios-iouc=kbd,Spotlight,WidgetRenderer_Default,chronod oneios-sysfail=configd,kbd,Spotlight
   oneios-hid-queue-kib=256 owner-hide-vmm=bluetoothd,BlueTool`. In the session: Wi-Fi with a known network in
   range, type a Wi-Fi password for a minute, stay on a widget page, flip Bluetooth, tap keys with click sounds on,
   plug and unplug USB, then return. After it: `oneios-tool phone pull-data data.img` and the searches of each item.
1a. **The heat** (`docs/hardware.md` 7d "It runs damn hot", branch `fix-thermal-log`): read the new
   `thermal: 30 s: host CPU ...` line (host sys/user, swap KiB/s, xnuhost, battery and charge current, case, SoC) and
   `oneios-cpu-sys:` (backboardd's kernel time) with `oneios-display: the compositor swaps format` and `half-float
   frames:`: a half-float frame narrowed by the CPU in backboardd's swap would show as kernel time; user time is Core
   Animation's and Metal's (name it from `oneios-ubt=backboardd` with `oneios-tool sym`). Spotlight's indexer A/B
   (one boot): `oneios-disable-jobs=com.apple.accessibility.axassetsd,com.apple.searchd,com.apple.spotlight.IndexAgent`,
   undone by `oneios-enable-jobs=com.apple.analyticsd,com.apple.searchd,com.apple.spotlight.IndexAgent`; costs search
   results while off. Charging: the line shows it; a limit would be the fork's (not asked yet).
2. **Wi-Fi joins** (`docs/hardware.md` 3): look for `ONEIOS WIFI: scan record 1 ... TIMESTAMP` a few ms before now,
   `channels info: N channels (the host's list)`, then `join:`; in iOS's log `AUTO-JOIN: Scan Results (N>0)`.
   Then configd's 22 s between `*STACK*` and en0 from the `oneios-ubt` samples, and wifid's 17 fault entries.
3. **The keyboard stops after 3-4 characters** (`docs/newtodo.md` 12, PR #23): with `oneios-hid-queue-kib=256`,
   `oneios-hid: touch screen: event queue 256 KiB` and `N dropped` 0; whether the whole UI or only the keys stop;
   the MobileAsset lock line's errno, the error after InputUI's `Failed to access`. A/B boot:
   `oneios-enable-jobs=com.apple.analyticsd,com.apple.coreduetd,com.apple.siriknowledged,com.apple.modelmanagerd,com.apple.modelcatalogd,com.apple.intelligenceplatformd`.
4. **The chime** (`docs/hardware.md` 7c, PR #19): the host side of the graceful return is done. **Write the guest
   power-off** behind `oneios_power_request()` (`drivers/bridge/OneiOSBridge.c`): a hosted patch (0059 is free)
   that starts iOS's clean power-off as a power-button shutdown does, a halt, never a reboot. Then a PC boot with
   `--set return-graceful-ms=10000`, then the phone; only then make 10000 the default.
5. **iOS's RAM must fit, then be held fixed** (`docs/booter.md` 2.2 "What fits" and "Fixed RAM", PRs #18 and
   fix-ram-fit). The phone (2026-11-14, `ram=auto` 9,472 MiB of MemTotal 11,084): Linux swapped 2,126 MiB of iOS's RAM to
   zram, Settings froze, and the swap traffic (about 9.5 MB/s in, 8 MB/s out) was also the main heat. fix-ram-fit:
   `ram=auto` now takes no more than what fits beside the session's needs (MemAvailable at the start less 1 GiB
   headroom, 256 MiB for KGSL and 8 % of the guest for page tables: about 6.4-7.2 GiB on this phone, by its start
   MemAvailable); an error line when `ram=` by hand is more than fits; `memory: the host swaps iOS's RAM: N MiB ...` with
   the host's swap rates when it happens; kernel 0067 in the series; no prefault without the lock (the PC boot B swapped 841 MiB of a prefaulted, unlocked guest). PC results and the lines to read:
   `docs/booter.md` 2.2 "What fits". GPU memory is the guest's own RAM, so no separate pool is needed (`docs/gpu.md` 2).
   **2026-11-15 (main `3233fed` on the phone):** with `ram-giveback=1` at a fitted 6,656 MiB (start MemAvailable
   8,561 MiB) Linux still swapped iOS's RAM (138 -> 370 -> 604 MiB, swap-outs up to 37.9 MB/s) with about 1.9 GB of
   other page cache there (Samsung's swappiness); the SoC went 59.4 -> 70.6 -> 83.8 C with host sys 200 %. Only the
   lock stops that. **fix-ram-locked (this PR): the default flips to fixed, locked RAM** (`ram-giveback=0`,
   `ram-prefault=1`, `ram-lock=1`; `ram-giveback=1` flips it back); `ram=auto` takes the most that fits with the lock
   (the headroom 768 MiB where the lock can work, so (8,561 - 768 - 256) / 1.08 -> **6,912 MiB** on this phone, else
   1 GiB as before); a lock that fails part way keeps what it locked (EPERM or the PC's 8 MiB RLIMIT_MEMLOCK at the first
   step: nothing prefaulted, a clean line; PC boots C and D, `docs/booter.md` 2.2: a fallback to give-back there swapped
   the same, 961 against 910 MiB in 15 min, so it was not kept); the valve runs in the main loop at nice 0, is never under 128 MiB with the
   lock, and what fits leaves it 256 MiB more; the prefault stops if MemAvailable falls under the valve's level; the 5 s
   swap check reads nothing while all is locked and the mincore covers only the unlocked part; the thermal step only at
   a critical temperature (the case or the cell at 50 C), all cores otherwise. The next boot (the owner's `ram=auto
   ram-giveback=0 ram-prefault=1 ram-lock=1`, now the defaults) should show `CAP_IPC_LOCK yes`, `memory: the guest's RAM
   is held: N MiB prefaulted in T ms, locked`, no `the host swaps iOS's RAM`, `oneios-mem: free pages given back to the
   host: no`, the `thermal: 30 s:` line without swap traffic, and `stopping:` right after the return's SIGTERM.
   **fix-ram-hugepages (2026-11-15):** the memfd's offset 0 now sits at a 2 MiB boundary (before, no 2 MiB page could be
   mapped with one entry) and xnuhost's window gets `MADV_HUGEPAGE` where `shmem_enabled` allows (`ram-huge=0` off).
   That is the window's TLB reach and its own page tables (size / 512, 14 MiB at 7 GiB); the 575 MiB of page tables are
   the iOS tasks', which map 16 KiB at a time, so the fit's 8 % stays (`docs/booter.md` 2.2, "2 MiB pages"). The fork
   would have to set `shmem_enabled` to `advise` at the switch and restore it at the return (`oneos/interface.md` 4):
   not done. Phone lines: `memory: 2 MiB host pages: yes|no (shmem_enabled ...)`, `memory: 2 MiB pages (the prefault
   done): xnuhost maps N of M MiB ... with 2 MiB entries`, `page tables P MiB (xnuhost's own V MiB)`.
6. **Haptics and the power key** (`docs/hardware.md` 5, PR #17): the counters show where a tap stops. PC boot with
   `--set haptics=sim`; phone: `Actuator: user client N opened by audiomxd`, audiomxd's `actuator port` lines; the
   power-key boot with `oneios-actuator=0`. **Power key (fix-power-key, 2026-11-14; confirmed broken in the 9f6dc38
   session):** a stand-in for the button haptics service ButtonResolver waits for, `oneios-button-haptics=1` (off by
   default; built, not on the phone) and, since a PC boot showed backboardd taking the buttons for an external keyboard
   (`eventSource: externalKeyboard`, `destinations:<none>`), `oneios-hid-buttons-builtin=1` (off by default); the 15:35
   boot also brought `BacklightMode.inc` (`oneios-bl-mode=0` is the test of that). The A-E boots and the lines to read are in `docs/hardware.md` 5, "The power
   key: a stand-in for the button haptics service".
7. **Blank widgets** (`docs/gpu.md` 5.1, PR #16): the `oneios-gpu-surfaces/pixels/locks/sync` lines and the host's
   `gpu: KGSL imports` say where the image is lost; the four guesses and how to read them are there. The likely
   fix (guess 1) is in the Metal plug-in: the Mac builds it, with a new trust list and seed.
8. **Bluetooth** (`docs/hardware.md` 4, PR #20): with `owner-hide-vmm=bluetoothd,BlueTool`, does `Booting chipset`
   appear; then what the non-VM path needs (likely an `AppleOLYHAL` with `HWIdentifiers`). Also the PC's
   root-device stall with the Bluetooth driver on.
9. **Display colour and blur** (`kernel/patches/oneios-hosted/display/README.md`, PR #15): on the phone
   `displayctl test --format rgba16f` (a photo of the P3 patches), the compositor's swap format lines; the Mac
   builds `userland/metal/DisplayWide.inc`; open: 10-bit `w40a`, EDR keys for the backlight, an `identity.adt` in
   the image set. Ask the owner for real iPhone 18 Pro screenshots (Control Center over the Home Screen, Settings'
   navigation bar over content).
10. **Audio** (`docs/hardware.md` 6, PR #21): `latency about N ms` (about 100 expected, was 130-137), the owner's
    click-recording test, then `oneios-audio-latency=` set from it and made the default.
11. **USB and adb in a session** (`oneos/interface.md` 6a, PR #24, OneOS #1): first check the cable and the Mac's
    port with One UI whole; a session with repairs off (plug, unplug, plug), then one with the `usb-repair` file.
    The fork's three new app tests run first in its CI.
12. **`pull-data` over wireless adb** (PR #25): the chunked copy has run on the stand-in only; check busybox's `dd`
    and `sha256sum` on the phone, `--verify end` against `--verify chunk`, a break and a resume.
13. **Slow Home Screen** (0060): `oneios-ls-state:` and `oneios-icons-state:` lines; lsd's fixed start work stays.
14. **WebKit** (`docs/newtodo.md` 10): **cause found and fixed (fix-webkit-icache, 2026-11-14; PC only).** The crash
    report's fault was JSC's i-cache flush (`dc cvau`) on the read-execute JIT alias, taken as a write because the
    stub and the kernel dropped ESR ISS.CM. Fixed in the stub and in hosted 0071. On the phone (new runtime and
    `kernel.kc`): no `.ips` with `sys_icache_invalidate`, WebContent stays up, Safari loads a page.

## Decisions waiting for the owner

- Two caps of ours remain against the rule "all CPU cores, no caps of ours on the GPU" (found by fix-ram-locked):
  #40 sets `uclamp.max` 512 on iOS threads the kernel hints as background, and #42 runs background GPU contexts at
  KGSL's lowest level at a still screen (`gpu-level=hold` or `gpu-priority=0` turn it off). Both are named in
  `runtime/bridges/thermal/README.md`. Keep them, or remove them?
- OneOS #6 moves the switch's shell into the root cpuset (a cgroup v1 hierarchy of its own; the cpu and memory
  cgroups are unchanged). If the rule "never leave its cgroup" is meant to cover the cpuset too, revert it.
- `scripts/image/imagetool.py` and `scripts/fetch/remotezip.py` are unused on `main` (the Rust commands replace
  them): delete them?
- `userland/roottweak` (Python group 4): port it to Rust? A mistake changes the seed, so only with a byte-for-byte
  seed comparison.
- `scripts/ios/{ios-sign,macho-check,make-tbds}.py` are still Python (not ported yet).
- The token for `gh`: rerunning a failed CI job needs **Actions: Read and write**.
- The WebKit agent of 2026-11-12 ran on Opus 4.8, not 5.5 (PR #5: diagnosis and a test only). Redo it on 5.5? (2026-11-14: done with the crash report: fix-webkit-icache on 5.5 found the cause and fixed it, item 14.)

## Fault detail (written by each fault's agent; newest notes first in each)


1. **Memory pressure stalls the UI** (15:35 boot, fresh store). The guest has 4,070 MiB; GPU blobs hold about
   1.6 GB and backboardd's share grows 760 -> 930 MiB in 30 s; swap does not work ("Failed to open swap file",
   errno 2 and 30); `VM_pageout_scan` takes ~95% of a vCPU; every page it frees fans out unmaps to almost every task
   (1.47 million round trips per 30 s, ~1.5 vCPUs); page fills ~660 us. Fixes, in order:
   - **Deferred unmaps: built and host-tested (2026-11-12), not on the phone.** A task with no running thread gets
     its unmaps queued (up to 256 with the maps) and carried out by the stub before its next run. The WIP
     (`a97687f`) let another thread of the task run while a release or an order was still carrying the queued
     unmaps out; fixed (`task.unmaps_out`), with a two-vCPU check in the test kernel (`docs/booter.md` 2.6). The PC
     benchmark (qemu): the `task_unmap` call 17-18 us -> 1.0-1.2 us; a fault, fill and unmap ~53 -> ~36 us. On the phone:
     read the `perf:` line's `task_unmap N queued, M ordered` and the `VM_pageout_scan` share.
   - **RAM: the owner's decision: iOS gets at least 90% of the phone's RAM. `ram=auto` built and host-tested
     (2026-11-12), the default; not on the phone.** Guest = min(90% of MemTotal, MemTotal - 1.5 GiB) in 256 MiB steps
     (about 9.5 GiB on this phone); `ram-percent=`, `ram-host-floor=`, `host-mem-log=` (30 s), also in the image set's
     `xnuhost.conf`; the choice and the host's MemAvailable in the log (`docs/booter.md` 1). Checked: the window,
     `/chosen dram-size`, `boot_args.memSize`, `xh_info.ram_size` (test kernel); the GPU pool is fixed (8 MiB of
     response buffers; blobs are guest pages). **A large guest boots (the PC, 2026-11-12):** `ram=auto` on the PC
     (MemTotal 15,872 MiB) gave 14,080 MiB; XNU set up its ranges (`zones` 24G, the same at 2 GiB) and booted through
     launchd to SpringBoard in 420 s under qemu with no panic. OOM score unchanged.
   - **Free pages back to the host: hosted 0058, built and run on the PC (2026-11-12), not on the phone.** The goal: iOS
     keeps all of its ram=auto memory; the host gets back only pages iOS has freed (the owner's decision: no pressure
     feedback, no cache cap, XNU's targets, memorystatus and jetsam unchanged). What filled the memfd was not the file
     cache: stock XNU's free queue is FIFO, so the boot walked through every never-touched page and the host kept all of
     them (control boot at `ram=auto`, 14,080 MiB: 12,266 MiB held at 990 s while XNU had 9,048 MiB free; the PC's
     MemAvailable fell under 1 GiB and the run was stopped). 0058 enters freed pages at the free queue's head and
     never-touched or given-back ones at its tail, and gives back free pages above the free target + 32 MiB with
     hysteresis (`ram_discard`, a hole in the memfd). Same boot with it: 4,973 MiB held at 990 s, XNU's view the same
     (free 9,084 MiB; active, inactive, speculative, wired within 15 MiB of the control's), launchd 292 s and SpringBoard
     333 s (control 306 s and 361 s), no panic; `hosted_mapcheck=1` boot: launchd, SpringBoard and the daemons, no panic, no `hosted check:` line, 893 MiB given back with
     `oneios-free-report-stress=1` (4 GiB). Cost: a page given back and
     used again costs the host's refill (+5.7 us per 16 KiB under qemu); on by default, `oneios-free-report=0` turns it
     off (`kernel/patches/oneios-hosted/README.md`, 0058). On the phone: the `memory:` line's `holds` against
     `oneios-mem:`'s in-use figures; if `host MemAvailable` still falls under ~300 MiB, lower `ram-percent` in the
     image's `xnuhost.conf`; check `/sys/kernel/mm/transparent_hugepage/shmem_enabled` (a shmem huge page setting
     may keep punched pages).
   - **Fixed RAM (fix-fixed-memory, 2026-11-13; the default since fix-ram-locked, 2026-11-15, item 5 above; this note
     is as written then): built and host-tested, OFF by default (`ram-giveback=0` turns it on),
     not run in a PC boot of iOS or on the phone.** The owner's requirement: RAM and GPU memory set once at the start.
     xnuhost prefaults the whole memfd on a nice-19 thread, `mlock`s it where Linux allows and it fits in MemAvailable
     less `ram-session-reserve` (1 GiB; `ram=auto` gets the same bound), refuses `ram_discard`, unlocks once if the host's
     MemAvailable falls under `ram-unlock-below` (256 MiB), and logs a third `memory: fixed RAM:` line (locked, refused,
     host swap-ins/outs, major faults, KGSL's per-process memory). Kernel 0067 (the kernel told not to give back, order
     kept) is written but not in the series. No separate GPU pool: the blobs are guest pages of the same memfd
     (`docs/gpu.md` 2, "No separate GPU pool"). Details, safety and log lines: `docs/booter.md` 2.2 "Fixed RAM". Next: a PC
     boot before/after (`--ram 4G`, same Data store; then `--set ram-giveback=0`), kernel 0067 built, then the phone with
     `ram-giveback=0` in the image's `xnuhost.conf`.
   - **Swap: built (the Linux PC, 2026-11-12), not on the phone.** Hosted 0057 makes `/private/var/vm` root:wheel 0755 before
     launchd in every Data store and, on each swap file failure, logs which volume the folder was on and whether it
     was read-only; the roottweak's `data_layout` puts the folder in new seeds. errno 30 is not settled (the sealed
     root before the Data mount, or a Data volume gone read-only: `kernel/patches/oneios-hosted/README.md`, 0057).
     **Settled on the PC:** both are early tries before the Data mount (errno 2: the freezer's budget probe before the
     root is mounted; errno 30: the creation thread's first `swapfile0` on the sealed root's read-only
     `/private/var/vm`); 0057's folder is then there on the Data volume. No swap file was made in 20 min at 2 GiB on
     the PC (no swap-out asked). **Swap serves only frozen apps on iOS (read in xnu-12377 and seen on the PC):** iOS's
     kernel (`CONFIG_FREEZE`) defaults to `VM_PAGER_FREEZER_WITH_SWAP` (`vm_compressor_mode is 32` in the PC's log): the
     compressor is on, `swap_is_active` is off and `freezer_swap_is_active` on, so a general compressed segment is never
     queued for swap-out (`C_SWAPOUT_REG` needs `VM_CONFIG_SWAP_IS_ACTIVE`); only the freezer's segments
     (`C_SWAPOUT_FREEZER`, frozen apps), donated early segments (`C_SWAPOUT_DONATE`) and, with the boot argument
     `vm_compressor_scavenger_enabled=1` (off), old ones (`C_SWAPOUT_RIPE`) go to swap. So swap cannot relieve the
     pageout scan of the 15:35 session; [I] it helps only once iOS's freezer freezes apps (userland decides that; not
     seen on the PC). The device tree's `/defaults kern.vm_compressor` could set another mode (not set in the PC's
     image; the phone's log line `vm_compressor_mode is N` says). On the phone: a `Failed to open swap file` after the
     Data mount, or `swapfile0`. Compiled and prelinked on the Linux PC (`HOSTED_OUT=pc`,
     `kernel.kc` `ffbe95f7...`; `kernel/prelink/README.md`, "A Linux-built kernel against the list").
   - **backboardd's GPU memory growth** (gpu): read, not fixed (2026-11-12). Upstream NativeMetal (at our pin)
     already frees the runtime's empty 64 MiB blocks every 2 s even while backboardd draws (`watchIdleMemory`),
     drops held surface imports and every kept block on a memory-pressure rise and keeps no spare room while it stays
     raised (`watchImportPressure`, `NativeBridgeMetalFrontend.mm`). metal2vulkan keeps up to 2 empty blocks a pool
     and frees a stale one only when a placement of that pool is returned (`memory.rs` `Pool::release`/`stale`), which
     the 2 s timer covers. So the 170 MiB per 30 s is one of: IOSurface imports CoreAnimation still retains
     ("wired"), partly used blocks, or blobs the guest released that the host still holds. **On the phone:** the
     kernel's GPU report's clients line (own vs wired MiB for backboardd) and sizes line ("64+" count) every 10 s
     across the 30 s of growth, and `m2v-mem.txt` in backboardd's temporary folder on the Data volume (copied with the
     logarchive procedure below); then the fix goes where the number is (iOS side: built on the Mac).
     **2026-11-13 (hang, branch `fix-ui-hang-memory`): the holder is in backboardd itself, which of its memory is not
     known yet.** The phone's `blobs +182 closed +17` per 10 s means the client does not close them (the kernel frees
     what is closed, at once or after the client's fenced work; the host frees at `RESOURCE_UNREF`): read end to end,
     `docs/gpu.md` 5.1 "Who keeps a blob, and who frees it". Wired memory falling back when the UI settles fits the
     plug-in's 2 s trim of empty blocks, so the growth while busy is not yet a leak. New counters, so the next log names
     the kind: `oneios-gpu-kinds:` and `oneios-gpu-ages:` per client (own blobs in the task or the kernel, mappable or
     not; wired by the mapping's VM tag: IOSurface, malloc, other; sizes under 1 MiB; ages; closed blobs still held and
     the oldest wait), and xnuhost's `gpu: 10 s: host blobs N (M MiB), made +A unref'd +B` against the guest's live count.
     No plug-in change is written: which plug-in object to change waits for those lines.
2. **The power key does nothing in iOS** since the haptics change (15:35 boot): the presses reach backboardd (34 HID
   events, 0 dropped) but no `request_power_change`. **Read in 24A446 (2026-11-12, `docs/hardware.md` 5, "The power
   key since the Actuator"):** ButtonResolver's AOP interface waits for Haptics.framework's button haptics loader and
   the SPU haptics driver, and libMobileGestalt reads `/product/haptics closed-loop`; [I] with closed-loop = 1 the
   resolver is never ready. Waits for the phone: boot with `oneios-actuator=0`, press the side key (lock, wake), read
   ButtonResolver's `HasHaptic` lines; the fix candidates are written there. Hypothesis (not proven): with closed-loop haptics, backboardd's
   BKButtonHapticsController (its queue waits forever for button "click states") now sits in the button path. Quick
   test: boot with `oneios-actuator=0`. Fix: satisfy the click-state setup, or tell iOS the side buttons have no
   haptics. Not started.
   **Read further (fix-haptics, 2026-11-13, `docs/hardware.md` 5, "Haptics do nothing"):** the node sets two
   MobileGestalt answers, `DeviceSupportsHaptics` (the node exists) and `DeviceSupportsClosedLoopHaptics` (its
   `closed-loop`); ButtonResolver and Haptics.framework read neither (ButtonResolver: `HomeButtonType` and
   `MGIsDeviceOneOfType` only), so the node does not change whether ButtonResolver gets ready. backboardd's
   `BKButtonHapticsController` exists in every boot and blocks any caller of `playHapticForClickState:clickSpeed:`
   (MIG `_BKHIDXXPlayHaptic`) until ButtonResolver is ready. So the haptics fault and this one are not proven to share
   a cause; [I] the node may add a caller of that path or a lock path that waits on audiomxd's haptic server. The test
   is still `oneios-actuator=0`, now with the `oneios-ubt=SpringBoard,backboardd` lines after a press (the stacks to
   look for are in hardware.md).
2a. **Haptics and vibration do nothing** (23:45 boot): every layer "on", no request seen anywhere. Counters in every
   layer, the PC's stand-in actuator (`haptics=sim`) and what 24A446 reads: built 2026-11-13 (fix-haptics),
   `docs/hardware.md` 5, "Haptics do nothing"; the next boot's `ONEIOS HAPTIC: counters:` and xnuhost's `haptics:
   counters:` lines show the layer where a tap stops.
3. **The display helper dropped after 7 frames** (dark panel while iOS drew; restarted by hand). Fixed in `2c7bbb5`
   (xnuhost keeps the socket on transient send errors and logs; the helper logs why its read ended); host tests were
   started, not read; not on the phone.
4. **Brightness:** requests with the key `DisplayBrightness` were ignored ("no brightness in it"). **Handled (built,
   host-tested decoder, the driver compiled into the Linux PC's kernel of 2026-11-12, not on the phone):** 24A446's CoreBrightness sends the slider's position 0..1 (a
   float) or `{Brightness, Commit, Period}`; the driver takes it when unambiguous and logs what came
   (`drivers/brightness/README.md`, "DisplayBrightness"). If the log says `the integer 0 or 1`, the float was lost in
   IOKit's serialization and backboardd's side must send `brightness-nits`. Then check whether the
   slider moves the panel; auto-brightness: CoreBrightness refuses the light sensor (`AppleUSBALS: couldn't find the
   device version`).
5. **Camera:** try the Camera app; the log lines to read are in `drivers/camera/README.md` / `runtime/bridges/camera/README.md`.
   Waits for the phone. Steps: in One UI first `bin/camctl` (`runtime/bridges/camera/README.md`, "On the phone") to
   see the host's cameras; then a session with the image set that has the plug-in (`data.seed` with
   `H9ISP.mediacapture`, `oneios-camera-addon: ... stage 5, status 0` in the kernel's log), open Camera, and read
   cameracaptured's lines in iOS's log (`fcc_probeCameraDriverService` finding AppleH9CamIn, the plug-in's
   `H9ISPCaptureDeviceCreate`, its first frame).
6. **Audio latency** was ~130-140 ms on the host path (AAudio's fast/MMAP paths need a realtime thread, which is
   forbidden). **Built and host-tested 2026-11-13 (fix-audio-latency), not on the phone:** the stage shares and the
   changes are in `docs/hardware.md` section 6, "Latency per stage". The helper's level now follows the server's
   pulls (the burst of 1600 is the mixer's 33 ms period [I, AOSP's sizing rules]), its target comes back down after
   underruns, a backlog's silence is cut, AAudio's buffer is a burst plus the target plus 5 ms; LOW_LATENCY was
   studied (a blocking writer gets no FAST track and no realtime thread: AOSP source). In the host test's harness the
   guest-send-to-server-read time went 73 -> 33 ms (the phone's shape) and 268 -> 22 ms after a 200 ms start stall;
   predicted host path about 98-106 ms (Android's mixer and HAL after the read, 65-73 ms, are unchanged). The
   reported latency is unchanged (6480; 5040 predicted, set by boot argument after the phone measures; the kernel build of
   that default was stopped at the hand-off), LOW_LATENCY is opt-in (`ONEIOS_AUDIO_PERF=low`). On the phone: read the lines that section lists (`the server
   pulls N frames every M ms`, `margin`, `latency about N ms`) and time a key click against a recording.
   **Since 2026-11-15 (fix-audio-latency, the 8644ae9 session's 105-123 ms on the host side):** LOW_LATENCY with MMAP
   exclusive by default (no callback, no realtime thread), the speaker's ring 1920 frames (iOS's I/O buffer at most
   720 frames instead of 3072), each speaker period handed on as it starts, the ring's wrap fixed (every device),
   the output latency iOS is told measured by the helper at each boot, `end-to-end N ms` and `speaker engine:` lines,
   `bridgectl audio-loop`. On the phone: `docs/hardware.md` section 6, "Latency end to end", "On the phone".
7. **Bluetooth:** the uart.sk transport (e73c11d) has not been tried in iOS. Waits for the phone. Steps: a session,
   Settings > Bluetooth on, then the expected lines of `docs/hardware.md` 4 ("Expected lines, in order"), from
   `ONEIOS BT: dev: bluetoothd[N] opens /dev/btwake` to `bluetooth: the controller hears devices`; if they stop, the
   last `bluetoothd: command 0x....` line names the command.
8. **Hotel Wi-Fi:** iOS has an address; check that the captive-portal page shows and the internet works.
9. **CLAUDE.md's side-key rule** names event0/event2/event4, but the numbers move between boots (in the 15:35 boot
   event4 was the vibration motor). Proposed wording, by name: never grab `gpio-keys`, `pmic_pwrkey`, `pmic_resin`.
   Waits for the owner.
10. **WebKit content processes crash in a loop; Safari hangs** (2026-11-12 23:45 boot). Each `com.apple.WebKit`
    content process (pids 300-331, ~20 in 20 s) logs, in order, `triggered unnest of range ... of DYLD shared
    region`, then `CODE SIGNING: ... vm_map_entry_reinit_code_signing_after_copy: curprot cannot be write+execute.
    removing execute access`, then crashes; after ~20 corpses iOS says `Too many corpses being created`. MobileSafari
    sits at 70-101 % CPU (spinning on the content process it keeps losing).
    - **Cause found and fixed 2026-11-14 (fix-webkit-icache, PR #33; built, tested and booted on the PC to SpringBoard with `hosted_selftest=1` all passed; not on the phone).** The crash
      report (phone, main `9f6dc38`, `com.apple.WebKit.WebContent-2026-11-14-060324.ips`): `EXC_BAD_ACCESS / SIGBUS,
      KERN_PROTECTION_FAILURE at 0xc998c0000`, esr "(Data Abort) byte write Permission fault", thread 0 in
      `libsystem_platform sys_icache_invalidate + 40` <- `JSC::LinkBuffer::finalizeCodeWithoutDisassemblyImpl` <-
      `JSC::LLInt::createJSGateThunk` <- `JSC::LLInt::initialize()` <- `JSC::initialize()` <- `WebKit::InitializeWebKit2()`.
      So JSC's first JIT thunk dies in its i-cache flush on the read-execute alias of the JIT region. Proven from the
      code: the `dc cvau` is that alias's first access (the hosted pmap maps a page into the task only when it faults),
      so it takes a data abort with ISS.WnR = 1 and ISS.CM = 1; Linux passes that ESR on whole; the stub read WnR
      alone (`XH_PROT_WRITE`); the kernel's fill refused a write to the read-execute page, and its ESR (0002's
      `hosted_trap_fault_esr`: a permission fault with WnR, CM dropped) made `inspect_data_abort` ask `vm_fault` for
      write: KERN_PROTECTION_FAILURE, exactly the report's "byte write Permission fault". `test_jit` passed because
      its RX alias was mapped before the thread ran (no fault), and qemu-user never faults on cache maintenance. The
      host mapping of the RX alias was readable all along (`hosted_pte_prot` gives `READ | EXEC`). Fixed on both
      sides: the stub (`xs_fault_access`: CM is a read; host test `test_fault_access`) and hosted **0071** (the same in
      the kernel, also for the kernel's own faults; XNU gets CM and WnR as the hardware set them with an access flag
      fault, so its own CM rule makes it a read; checked by `hosted_selftest=1`). Test: `test_jit_icache` in the test
      kernel (`docs/booter.md` 2.6 and 6). Either half alone fixes it (a new runtime with the old kernel, or the new
      kernel with the old runtime). **Proof on the phone** (a runtime and `kernel.kc` with the fix): no new
      `com.apple.WebKit.WebContent-*.ips` with `sys_icache_invalidate` (or any `KERN_PROTECTION_FAILURE` in
      `JSC::LLInt::initialize`) under `/private/var/mobile/Library/Logs/CrashReporter`; `oneios-tool ioslog show
      data.img --proc com.apple.WebKit.WebContent` shows one long-lived content process per tab, not ~20 in 20 s, and
      no `Too many corpses being created`; MobileSafari is not at 70-101 % CPU; Safari loads a page (a screenshot with
      the page drawn). The `CODE SIGNING: ... removing execute access` line stays (benign, below). If WebContent still
      dies, read its new `.ips`: the next fault is a different one.
    - **Investigated 2026-11-12, not fixed then; the host side was shown sound for a resident RX alias, the cause
      needed the crash report.** What was ruled in/out:
    - The **CODE SIGNING line is benign** and matches a real device: `vm_map_entry_reinit_code_signing_after_copy`
      strips execute from the **copy** of a `used_for_jit` entry (not the executable one) on every iOS map, because
      `VM_MAP_POLICY_ALLOW_JIT_COPY` is always false on iOS (`VM_MAP_IS_ALIEN` is unconditionally true). It fires
      during WebKit's own `mach_vm_remap` of its JIT mirror and during corpse `vm_map_fork`. It is a marker, not the
      fault.
    - **a919ff3 reveals, it does not cause.** Before it, a task `brk` (a WebKit `RELEASE_ASSERT`/`__builtin_trap`,
      e.g. `brk #0xbb08`, or a PAC `brk #0xc471`) reached iOS as a kernel physical-aperture data abort and panicked
      the whole kernel (the 23:15 tcp_input panic was one such brk). After it, a task's `brk` becomes a clean
      `EXC_BREAKPOINT`, so each content process now dies on its own and respawns — the loop we see. The same CODE
      SIGNING line was on the 23:15 boot, i.e. the JIT path was exercised before too.
    - **SPRR/APRR fast JIT permissions are not used.** WebKit (`JSC::initializeJITPageReservation`) reads a commpage
      byte (`com.apple`'s fast-permissions version) to choose the inline `msr sprr_uperm_el0` path (which would be an
      illegal instruction on the S24's standard Arm cores) versus the separated-W^X-heaps path. Our kernel's commpage
      leaves that byte zero (open-source `commpage.c` never writes it), so WebKit takes separated heaps. The CODE
      SIGNING line (from the remap in that path) confirms it.
    - **The host carries separated-heaps JIT.** New test-kernel case `test_jit` (`runtime/xnuhost/tests/testkernel.c`,
      `make check-arm64`, passed under qemu-aarch64 2026-11-12): a W+X `task_map`, the same physical page mapped
      read-execute at a second address, a thread writing code through the read-write alias and running it through the
      read-execute alias with i-cache maintenance. So the hosted pmap/host-mmap W^X aliasing is not the cause.
    - **Open, phone-only:** (a) whether One UI's kernel (SELinux/KernelSU) permits a W+X mapping of the RAM memfd for
      a task process — qemu-user has no SELinux, so `test_jit` cannot cover it (`docs/booter.md` 7); if it refuses,
      `task.c` logs `tasks: pid N: a deferred map (va 0x..., prot 7) failed`. (b) a WebKit JIT assert under 0027's
      keys-off PAC on a 39-bit user VA (the `jitCagePtr`/arm64e JIT cage, same 39-bit class as the libobjc bug 0027
      fixed). The next boot needs the WebContent crash report to decide:
      `oneios-tool ioslog show data.img --proc com.apple.WebKit.WebContent` and `--proc MobileSafari`, plus
      ReportCrash's `.ips` under `/private/var/mobile/Library/Logs/CrashReporter` for `com.apple.WebKit.WebContent`:
      read the **exception type** and faulting **pc/address**. `EXC_BAD_INSTRUCTION` at an `msr sprr_uperm_el0`
      means the commpage byte is non-zero on the phone (force it to 0); `EXC_BAD_ACCESS` at an address with bits
      39-46 set means the keys-off/39-bit-VA pointer-tagging class; `EXC_BREAKPOINT` at a WebKit `RELEASE_ASSERT`
      names the failing check (file/line are in the `brk` operands). Also read xnuhost.log for the `deferred map ...
      prot 7 failed` line. The `IOUC RootDomainUserClient failed MACF in process pid 282, MobileSafari` line is a
      separate sandbox/entitlement denial for the IOPMrootDomain user client; harmless to JIT, noted for the hang.
11. **iOS hangs while the keyboard is up (typing a Wi-Fi password, 2026-11-13 10:2x-10:47, main `c1223f8`).** The
    guest ran on; frames stopped (`30 s: flushes 10`), backboardd's GPU memory grew 783+147 -> 1232+202 MiB in 80 s,
    the host had 672 MiB available, the thermal step was 2 (trapping), Spotlight, kbd, Preferences and backboardd
    were the busiest. Read and changed on `fix-ui-hang-memory` (built and tested on the PC; not on the phone):
    - **The thermal step stacked three P vCPUs on a core** (proven from the code and the log's `0x60`): step 2 lost the
      E cores it means to add, and cpu-pin put six P vCPUs on two cores. Fixed: `0x63`, and no pinning when a kind
      has fewer cores than vCPUs (`runtime/bridges/thermal/README.md`, "Fixed 2026-11-13"). The cores each step allows
      are unchanged.
    - **`oneios-cpu:` divided by the nominal 10 s** (proven from the code): a late report thread (a stall) inflated
      every share [inference: the four busiest added up to 1,440 % of a CPU on eight vCPUs at +1119]. It now divides by
      the measured time and says the seconds.
    - **kernel_task's share is likely mostly idle threads** [inference] (oneios-perf on the PC: `oneios-perf: kernel_task: ... 686% of a vCPU;
      most: idle #4 ...`, 198 of 206 s idle threads): the phone's 340 % is not work until the perf line's `most:`
      names a thread that is not `idle`.
    - **0058's give-back took 131 s in the guest for 205 MiB** (0.57 s of it in xnuhost): every run was one 4 KiB
      page. Hosted 0066 takes long runs and larger batches and splits the time
      (`kernel/patches/oneios-hosted/README.md`, 0066).
    - **The host's memory**: a second `memory:` line says how much of the guest's RAM the host swapped out (zram),
      the host's swap and page tables [guess, not measured: with 672 MiB available, Linux swapping guest pages
      would give the 0.4-0.95 s faults and interrupts of perf at 1174].
    - Guessed, not proven: kbd and Spotlight spin on work this guest cannot do fast (the keyboard's language model
      and Spotlight's queries without the Neural Engine) or on GPU work that waits behind backboardd's; the keyboard
      not responding (fault 12) is the same load. **On the phone next:** the boot arguments and log searches of
      `docs/gpu.md` 5.1 "The next phone boot (hang)".
12. **The keyboard often does not come up or does not answer** (23:45 and 23:15 boots: kbd 24-40 %, Spotlight 96-102 %,
    `5105 HID events queued ... 6 dropped`). Not read past fault 11's load; the same samples and searches decide.
    **2026-11-13, main `c2d281a`: in Settings > Wi-Fi's password field the keys stop after the 3rd or 4th character.**
    Read on branch `fix-keyboard` (iOS's log of the `c1223f8` hang session, the 24A446 images and shared cache, a PC
    boot's Data store and iOS log):
    - **Changed (ours):** backboardd's HID event queue was Apple's 16 KiB default and drops events that do not fit; a
      dropped lift leaves a touch down for good, and a stuck touch plus the next finger is the keyboard's two-finger
      trackpad mode, where keys type nothing [hypothesis, not reproduced]. The touch screen and buttons can now get a
      256 KiB queue with `oneios-hid-queue-kib=256`, off by default until a boot has run it (`kernel/patches/oneios-hosted/hid/README.md`, "backboardd's event queue").
    - **The MobileAsset lock error is not a symlink fault** (read in the cache): `SYMLINK` is MobileAsset's name for
      opening an asset set's lock through its `latest` link under `/private/var/MobileAsset/AssetsV2/locks`;
      `com.apple.linguisticdata` is a UAF auto-asset that is only downloaded from Apple. This Data store has never been
      online (Wi-Fi was being joined) and the PC's store has no `AssetsV2/locks` at all. The keyboard's base data
      (`/System/Library/LinguisticData`, `TextInput`) is in the OS image. The line's `errno:N` decides: 2 is "not
      downloaded" (expected offline); 1, 13 or 62 would be ours (sandbox, permissions, a link loop). No seed change.
    - **`volume does not support data protection` is by design** (sqlite, every process, the PC too): the Data
      volume is mounted without `MNT_CPROTECT` (no Secure Enclave; keybagd says `mount is with NO CPROTECT`), sqlite
      drops its protection flags and opens the store; per-file classes are kept as an attribute (hosted 0022
      `owner-dataprotect`). A real iPhone mounts with `MNT_CPROTECT`. Not changed.
    - **InputUI's arbiter error** is `runOperations: %s Failed to access %@ remote service: %@` (UIKitCore): its XPC
      proxy to the focused client (Preferences) failed. The error after it (connection interrupted, invalidated, or a
      sandbox refusal) decides; not readable from the summary.
    - **PSGInputSuggester** is kbd's ProactiveInputPredictions: the "triggering endpoint" is a local FST language model
      and lexicon, the "serving endpoint" suggestd's `com.apple.proactive.input.suggester` (suggestd is enabled).
    - **AttentionAwareness's `AWDigitizerButtonKeyboardAttentionEvent` is one class for digitizer, button and keyboard
      events** (its `validateMask` in the cache): one event per touch is iOS's design, not our digitizer looking like a
      keyboard. About 900 events times 6 clients is little load.
    - **Disabled jobs the keyboard's processes may reach** (their entitlements' service names against the daemon
      policy): kbd: coreduetd (`coreduetd.knowledge`), aned, assistantd, maild; InputUI: modelmanagerd, modelcatalogd,
      siriknowledged (`com.apple.siri.uaf.service`, one of the two UAF endpoints), assistantd; Spotlight: coreduetd,
      duetexpertd, routined, watchlistd, familycircled. With coreduetd off nobody makes `/var/mobile/Library/CoreDuet`
      (PC: `Non-priveleged client unable to find knowledge directory, calling out for help`, then errno 2). Not changed
      without evidence: the A/B for the next boot is the existing `oneios-enable-jobs=` (hosted 0044).
    - **Next phone boot** (after the `c2d281a` session's lines are read): add `oneios-hid-queue-kib=256` and add
      `Preferences,InputUI` to `oneios-ubt=`; type in the password field and note whether the whole UI or only the keys
      stop. Read: `oneios-hid: touch screen: event queue 256 KiB` (with no `already open`), the state line's
      `N dropped` (0 expected), `input: touch:`; in iOS's log (`oneios-tool ioslog show data.img --boot last ...`):
      `--grep 'Unable to obtain shared lock'` (its `errno:`), `--proc InputUI --grep 'Failed to access'` (the error
      after it), `--grep 'KeyboardInputScene'`, `--proc kbd --grep 'warmUp|ICPredictionManager|timeout'`,
      `--grep '[Tt]rackpad|two.finger|UIKeyboardTrackpad'` (trackpad mode), `--proc backboardd --grep 'TouchEvents|path'`.
      A second boot as the A/B of the disabled jobs: `oneios-enable-jobs=com.apple.analyticsd,com.apple.coreduetd,
      com.apple.siriknowledged,com.apple.modelmanagerd,com.apple.modelcatalogd,com.apple.intelligenceplatformd`
      (the kernel's buffer is 256 characters; analyticsd must stay in the list).
13. **Every widget is blank or white** (23:45 boot) while `WidgetRenderer_Default[174]` has a Metal device (plug-in and
    Turnip loaded, `AGXAccelerator` opened 4 times, `WidgetRenderer_ 28+3 MiB`). **Read, instrumented, not fixed
    (2026-11-13, branch `fix-widgets`).** Read in 24A446, the plug-in and the kernel: the widget renderer hosts the
    widgets' SwiftUI views; RenderBox draws them with Metal in that process into IOSurfaces (or lets Core Animation
    draw), and backboardd composites the surfaces with its own device; each process imports a surface's own pages
    (`CREATE_BLOB_USER`: wired, no copy, its own host resource and udmabufs over the same RAM range), and nothing on the
    path copies, clears or zeroes a surface; the IOSurface mapping XNU makes for a second task shares the object
    (`mach_make_memory_entry_share`). So no fault on our side was found by reading. New lines show where the image is
    lost: the kernel's `oneios-gpu-surfaces` (who writes which IOSurface, whether the write finishes, whether
    backboardd reads it after), `oneios-gpu-pixels` (what the pages of each process's IOSurface imports hold: zero,
    white, flat, drawn; whether backboardd's imports are the widget renderer's pages), `oneios-gpu-locks`,
    `oneios-gpu-sync`; xnuhost's `gpu: KGSL imports ...` (whether KGSL keeps the guest's memory IO-coherent; drm2kgsl
    patch 0007). Guesses, in the order to test them: RenderBox drawing widgets into its own two-plane 4:4:4 surfaces
    that NativeMetal does not render into (Droid-VM's iPad showed that until the plug-in told RenderBox
    `MTLGPUFamilyApple2`), seen as `Unsupported ... 444` in the widget renderer's log with its surfaces `zero`;
    widget content never reaching the renderer (`placeholder` in chronod's and the renderer's log: not the GPU);
    backboardd sampling a widget's surface while it is still being drawn (`read while written`: this plug-in pin
    never asks the kernel's `SurfaceAwaitWriters`; test switch `oneios-gpu-read-wait=1`, off by default). **Left, in
    order:** (1) a whole hosted kernel build with the branch, the prelink (audit 0 failures, stubs check) and a PC boot
    with `oneios-gpu-metal=1` (no panic; the work paused before the link); (2) the Mac's runtime and kernel for the
    phone; (3) **on the phone:** the boot arguments, lines and searches of `docs/gpu.md` 5.1 "The next phone boot
    (widgets)"; (4) the fix where those lines put the loss (the plug-in's RenderBox family answer, chronod, the read
    order, or cache maintenance if KGSL drops coherence).

## Later

- **Rust tools:** `tools/oneios-tool/todo_rust.md` section 7 lists the Python that is left and why; ioslog in
  parallel waits for macos-unifiedlogs to open its oversize-string module.
- The board's L1-L3 items (a direct KGSL path after measuring, `min_runtime` in the image).
- Turnip sub-allocation and batched pipeline-cache writes (a plug-in rebuild with a new trust list).
- The PC's root-device stall with the Bluetooth driver on (`docs/hardware.md` 4).

## How to resume

1. Read this file, then `docs/agents.md`'s last notes. Saved logs (git-ignored): `out/session-logs/2026-11-12/`:
   the xnuhost logs of every session of the day (`combo1`, `diag`, `c2`, `c3`, `c5` = the 15:35 boot), iOS's own
   log of the 14:12 session (`ios3.logarchive`, parsed `ios3-log.csv`), the baseline config and `phone-baseline.txt`.
2. iOS's own log of a session: copy the Data store after the session (read only, no session running), fast:
   `oneios-tool phone pull-data data.img` (`--ioslog` then reads the log out of the copy without a mount).
   **Over wireless adb** the first port's one gzip stream broke twice on 2026-11-13 ("incomplete deflate stream"
   after 1.6 and 0.6 of 32 GiB, about 1.8 MB/s, adb still connected) and began at zero each time. It now copies in
   chunks of 256 MiB (`--chunk`), each one adb stream of a range of the one named file (`busybox dd ... skip=
   count= | busybox gzip -1 -c`), checks each against the phone's SHA-256 of the same range (a chunk of zeros is
   not copied at all), tries a broken chunk again (`--retries 5`), and keeps `data.img.part` and
   `data.img.part.progress`: **when it stops, is killed or gives up, run the same command again and it goes on
   at the first chunk that is not done** (`--restart` begins again; a store that changed since is refused). The
   last line gives the time, the bytes over adb and the bytes written. This copy in chunks **has not run on the
   phone yet** (stand-in test: `tools/oneios-tool/tests/phone/pull-resume.sh`); what to check there is in
   `tools/oneios-tool/README.md` "The copy that goes on". The one stream by hand (no resume, no sums) is
   `adb exec-out "su -c '/data/adb/ksu/bin/busybox gzip -1 -c /data/local/tmp/oneios/data/data.img'" |
   oneios-tool phone sparse-write data.img --gunzip`. `sparse-write` replaced `gunzip -c | python3 -I
   sparsewrite.py data.img` (the script of 2026-11-12, kept as `tools/oneios-tool/tests/phone/sparsewrite.py`:
   6 min, 2.2 GB of blocks for the 32 GiB store): the same bytes, a quarter of the CPU, and fewer blocks on
   Linux's file systems; its macOS part (APFS) has not run yet
   (`tools/oneios-tool/README.md` "The sparse copy"). The way of 2026-11-12, by hand on the Mac: attach the copy
   read-only
   (`hdiutil attach -readonly -noverify -nobrowse -imagekey diskimage-class=CRawDiskImage -nomount`), mount the
   Data volume `mount -t apfs -o rdonly,nobrowse,noowners`, copy `db/diagnostics/{Persist,Special,Signpost,
   HighVolume,timesync}` and `db/uuidtext/*` into a `.logarchive`, and parse it with Mandiant's macos-unifiedlogs
   (`unifiedlog_iterator -m log-archive -i ARCHIVE -f csv -o OUT`).
3. Build the set from `main` in a clean worktree (`git worktree add --detach out/parent-wt origin/main`, link
   `external/`), on the Mac or the Linux PC (both build the kernel; Rust is a prerequisite: the scripts build
   `oneios-tool` when it is missing): `scripts/bg.sh k flock /tmp/oneios-kbuild.lock env HOSTED_OUT=prelink
   XNU_FILESET_REFERENCE=1 kernel/build-hosted.sh`, then `env REFERENCE=out/kernel/current-hosted-prelink/reference.macho
   KERNEL=out/kernel/current-hosted-prelink/xnu.macho kernel/prelink/build.sh` (the audit must show 0 failures;
   `oneios-tool prelink stubs-check` 666 good), `runtime/package.sh VERSION`; on the Mac only, the Metal and camera
   plug-ins (`userland/metal/build.sh`, `userland/camera/build.sh`, with `out/ios27-tbd` linked); and the image
   (`env INPUTS=<main>/inputs/24A446 OUT=out/image scripts/image/build.sh --no-pack PROFILE`) only when the seed or
   the trust list changes (a new seed starts the phone's Data store over).
4. Stage and run through `oneios-tool` (CLAUDE.md's rule; `docs/building.md` sections 6-7): `oneios-tool phone
   stage runtime DIR` / `stage image DIR`, `phone sums`, `app stage-scripts` when the app is not used, then
   `phone switch`, `phone log --follow --save-auto`, `phone return` (or the whole of it: `oneios-tool cycle`).
   The manual forms (`scripts/phone/stage.sh`, `oneios_root.sh switch <versionCode> 1`,
   `touch .../session/return.request`) are the fallback when the CLI fails. On the phone read named files only,
   never a tree walk of sysfs or /proc (one panicked the phone's kernel on 2026-11-12).
