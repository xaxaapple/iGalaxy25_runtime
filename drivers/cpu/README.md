# CPU: P and E cores for XNU

No source here: the kernel side is UniXNU-droidvm's, unchanged (pin f9121cd, `external/UniXNU-droidvm`).

- `ipados27/patches/xnu-12377/droidvm-amp.patch`: `__ARM_AMP__` for the machine configuration, so XNU builds
  processor sets per cluster type, runs the Edge scheduler and answers `hw.nperflevels` = 2,
  `hw.perflevel0.*` (P), `hw.perflevel1.*` (E). It applies on the current series tree (`git apply --check` in
  `out/kernel/series/xnu`). The series leaves it out today; with it **every cpu node must carry `cluster-type`**
  (the SMP type panics in `cluster_type_to_pset_type()`), which booter's tree already does.
- `package/DroidVMCpufreq.c`: thread groups' preferred clusters (what CLPC would do) and each vCPU's frequency
  request to the host. It compiles with our kernel records. `droidvm_cpufreq_attach()` is its start.

Device tree, from the host's plan (`runtime/bridges/cpu`, `br_cpu_plan_make`):

| Node | Property | Value |
|---|---|---|
| `/cpus/cpuN` | `cluster-type` | `plan.v[N].type` (`"E"` or `"P"`); E vCPUs first |
| | `l2-cache-size`, `l3-cache-size` | `plan.v[N].l2_bytes`, `l3_bytes` when not 0 |
| `/arm-io/cpufreq` | `reg` | base, `plan.nvcpu * 0x1000`; the region's reads and writes go to `br_cpufreq_read/write` |

On the S24 Ultra with 8 vCPUs: cpu0-1 E (the two Cortex-A520), cpu2-7 P (five Cortex-A720 and the Cortex-X4).

Since 2026-11-15 (fix-host-overhead) each vCPU's window of `/arm-io/cpufreq` (its frequency table, and the capacity its
request is sized against) is the core cpu-pin puts it on (vCPU *i* on host CPU *i* on the S24 Ultra: vCPU 7 is the X4,
vCPU 2-4 the 3.15 GHz A720s, 5-6 the 2.96 GHz A720s), with the boost frequencies of `scaling_boost_frequencies` in its
table; before, every P vCPU was given the fastest P core's table. The kernel (BCM2837 config, no `HAS_CLUSTER` or
`HAS_MCORE`) makes exactly two clusters, E and P, from `cluster-type`: `docs/booter.md` 2.6b says what it reads and why
the 2.96 GHz A720s are P. A request reaches the iOS thread that runs on the vCPU, not only the vCPU thread
(`docs/booter.md` 2.6a).
