// SPDX-License-Identifier: MIT
// The arithmetic of the oneios-cpu window (OneiOSAdreno.cpp, cpu::report), plain C so that runtime/xnuhost's host tests
// (tests/test_xnuhost.c, test_cpu_window) check the same code the kernel runs.
//
// Why a window could say more than its CPUs (the phone: "oneios-cpu: 10 s: ... (all 1441%)" on 8 vCPUs). Two causes,
// both in the old code:
//   1. The divisor was the nominal 10 s while the report thread woke late in a stall (fixed 2026-11-13: the measured span).
//   2. A task's times (TASK_POWER_INFO, recount) grow only when one of its threads changes state: a context switch, a
//      return to or a trap from user mode. A thread that ran for a long stretch without one (a long user stay, a kernel
//      thread in a long host call) has all of it added at once, in the window where it finally switched, so one window
//      gets time from earlier ones. The sum of the tasks' deltas ("all") had no bound at all.
// Now (host-overhead, 2026-11-15) "all" is the processors' busy time: the span times the CPUs, less the time they idled,
// which recount keeps up to date (recount_processor_usage, hosted 0075's hosted_processors_idle_mach). It can never be
// more than the CPUs had. A process's share is capped at that busy time; its lump still lands in one window, but never
// past what the machine did in it.
#ifndef ONEIOS_CPU_WINDOW_H
#define ONEIOS_CPU_WINDOW_H

// uint32_t and uint64_t come from the includer (the kernel's headers in OneiOSAdreno.cpp, <stdint.h> in the host test).

// The span to divide by: the measured one, or the nominal one when there is no measurement (the first window, or a
// measurement under a second, which only a clock fault gives).
static inline uint64_t oneios_cpu_span_ns(uint64_t measured_ns, uint32_t nominal_s)
{
    return measured_ns < 1000000000ull ? (uint64_t)nominal_s * 1000000000ull : measured_ns;
}

// The CPUs' busy time in a window: span x CPUs less their idle time in it (both in ns). 0 when the idle time covers it.
static inline uint64_t oneios_cpu_busy_ns(uint64_t span_ns, uint32_t ncpu, uint64_t idle_ns)
{
    const uint64_t all = span_ns * ncpu;
    return idle_ns >= all ? 0 : all - idle_ns;
}

// A process's time in the window as it is reported: never more than the CPUs were busy (when that is known: busy_ns 0 with
// ncpu 0 means it is not).
static inline uint64_t oneios_cpu_capped_ns(uint64_t used_ns, uint64_t busy_ns, uint32_t ncpu)
{
    return ncpu && used_ns > busy_ns ? busy_ns : used_ns;
}

// Percent of one CPU.
static inline uint64_t oneios_cpu_percent(uint64_t ns, uint64_t span_ns)
{
    return span_ns ? ns * 100 / span_ns : 0;
}

#endif
