// SPDX-License-Identifier: MIT
// What xnuhost and the stub inside every iOS task process share: the layout of the stub's region
// of the task's address space and the pages both sides read and write.
#ifndef XNUHOST_STUB_ABI_H
#define XNUHOST_STUB_ABI_H

#include <stdint.h>
#include "xnuhost/hostcall.h"

// The region: 1 GiB at the top of what iOS may not use (xh_info.user_va_limit is its base).
#define XS_REGION_SIZE    0x40000000ull
#define XS_TEXT_MAX       0x10000ull     // the stub's code, read + execute
#define XS_CONFIG_OFF     0x10000ull     // struct xs_config, private
#define XS_BOOTSTACK_OFF  0x20000ull     // the service thread's stack
#define XS_BOOTSTACK_SIZE 0x10000ull
#define XS_SERVICE_OFF    0x100000ull    // struct xs_service_page (a slot of the trap memfd)
#define XS_THREADS_OFF    0x200000ull    // thread areas
#define XS_AREA_SIZE      0x40000ull     // per thread: trap page, guard, signal stack, guard
#define XS_SLOT_SIZE      0x4000ull      // a slot of the trap memfd
#define XS_SIGSTACK_OFF   0x8000ull
#define XS_SIGSTACK_SIZE  0x34000ull
#define XS_MAX_THREADS    ((XS_REGION_SIZE - XS_THREADS_OFF) / XS_AREA_SIZE)

#define XS_FILTER_MAX 64
struct xs_filter_insn { uint16_t code; uint8_t jt, jf; uint32_t k; };

struct xs_config {
  uint64_t base;                       // of the region
  int32_t  ram_fd, trap_fd;
  uint32_t service_slot;               // this task's service page in the trap memfd
  uint32_t kick_signal;                // the signal xnuhost sends to force a trap
  int32_t  uid;                        // setresuid/setresgid target, -1 = keep
  uint32_t filter_length;              // 0 = no seccomp (hosts without it; svc is then not trapped)
  struct xs_filter_insn filter[XS_FILTER_MAX];
};

struct xs_op {
  uint32_t op, prot;
  uint64_t va, size, offset;
  int64_t  result;                     // 0 or a negative errno
};

// One per iOS thread, shared (trap memfd slot): the first page of the thread's area in the task.
// The hand-off each way (docs/booter.md 2.6a): the side that waits spins on its word for a while (xnuhost: spin-host;
// the stub: spin_ticks, which xnuhost sets), then says it sleeps (host_sleeping, task_sleeping) and waits in a futex;
// the side that posts stores the word, then issues FUTEX_WAKE only if the other says it sleeps. Both use a full
// barrier between their store and their load (store word / load flag; store flag / load word), so one of them
// always sees the other's store.
struct xs_trap_page {
  uint32_t to_task;                    // futex: xnuhost sets 1 to release the thread
  uint32_t to_host;                    // futex: the stub sets 1 when the thread has trapped (or its maps are done)
  uint32_t tid;                        // Linux thread id; the kernel zeroes it when the thread exits
  uint32_t command;                    // XS_RUN or XS_EXIT, read by the stub when released
  uint32_t pac_set, pac_enabled;       // apply PR_PAC_SET_ENABLED_KEYS before running
  uint32_t booted, reserved;
  uint32_t host_sleeping;              // xnuhost waits for to_host in a futex (else it spins or is not waiting)
  uint32_t task_sleeping;              // the stub waits for to_task in a futex
  uint32_t slept;                      // the stub: its last wait for a release ended in a futex sleep
  uint32_t nops;                       // xnuhost: maps the released thread carries out before it runs (XS_OPS_OFF)
  uint32_t ops_done;                   // the stub: set when they are done (results in each op), then to_host is woken
  uint32_t reserved2;
  uint64_t spin_ticks;                 // xnuhost: counter ticks the stub spins for its release before it sleeps
  uint64_t t_enter, t_woken;           // the stub: CNTVCT_EL0 when its handler began, when it saw its release
  struct xh_trap trap;
  struct xh_user_state state;
};
#define XS_RUN  0u
#define XS_EXIT 1u
// Deferred maps and unmaps (XS_OP_MAP, XS_OP_UNMAP) after the trap page, in the same slot.
#define XS_OPS_OFF      0x1000ull
#define XS_THREAD_OPS   256u
#define XS_OP_MAP      1u              // mmap(va, size, prot, MAP_SHARED|MAP_FIXED, ram_fd, offset)
#define XS_OP_UNMAP    2u
#define XS_OP_PROTECT  3u
#define XS_OP_CLONE    4u              // new thread: area index in va, trap memfd offset in offset
#define XS_OP_EXIT     5u              // exit_group
#define XS_OPS_MAX     320             // the task's deferred maps and unmaps (up to 256) and the order itself

// A map is mmap(MAP_SHARED | MAP_FIXED | XS_MAP_POPULATE): the page tables are filled in the same call, so the toucher's
// retried instruction does not take a second (Linux) fault on the page it was just given.
#define XS_MAP_POPULATE 0x8000u

// The deferred queue (task.c, map-defer) joins `op` to `last`, the entry before it, where one host call does what the two
// did, in the same order (host-overhead, 2026-11-15). Returns 1 and rewrites `last` when they join, else 0:
//   map + map     the next page of the same file range with the same protection: one longer map;
//                 the same range again: the later map (MAP_FIXED replaces);
//   unmap + unmap adjacent or overlapping ranges: one unmap of both;
//   unmap + map   of the same range: the map alone (MAP_FIXED replaces the old mapping in one step);
//   map + unmap   of the same range: the unmap alone (nothing ran between them that could use the map).
// Anything else (a protect, a clone, ranges that only overlap) stays as two entries.
static inline int xs_queue_join(struct xs_op *last, const struct xs_op *op)
{
  const uint64_t end = last->va + last->size, op_end = op->va + op->size;
  const int same = last->va == op->va && last->size == op->size;
  if (op->op == XS_OP_MAP && last->op == XS_OP_MAP) {
    if (same) { *last = *op; return 1; }
    if (last->prot == op->prot && end == op->va && last->offset + last->size == op->offset) { last->size += op->size; return 1; }
    return 0;
  }
  if (op->op == XS_OP_UNMAP && last->op == XS_OP_UNMAP) {
    if (op->va > end || op_end < last->va) return 0;
    const uint64_t lo = op->va < last->va ? op->va : last->va, hi = op_end > end ? op_end : end;
    last->va = lo;
    last->size = hi - lo;
    return 1;
  }
  if (same && ((op->op == XS_OP_MAP && last->op == XS_OP_UNMAP) || (op->op == XS_OP_UNMAP && last->op == XS_OP_MAP))) {
    *last = *op;
    return 1;
  }
  return 0;
}

// One per task, shared: orders for the task's service thread (its first Linux thread).
struct xs_service_page {
  uint32_t request;                    // futex: xnuhost sets 1 when ops are ready
  uint32_t done;                       // futex: the stub sets 1 when they are done
  uint32_t ready;                      // futex: the stub sets 1 once the task process is set up
  uint32_t count;
  int32_t  pid, setup_error;
  struct xs_op ops[XS_OPS_MAX];
};

// The access a SIGSEGV or SIGBUS of a task thread was, as XH_PROT_* (0: unknown), from the syndrome in the signal
// frame's esr_context (for a user address Linux passes the hardware's ESR whole: arch/arm64/mm/fault.c
// set_thread_esr sanitizes kernel addresses only) and, without one, from whether the fault is at pc.
// A data abort with ISS.CM = 1 is a cache maintenance instruction by address (dc cvau, dc cvac, dc civac, dc cvap,
// ic ivau: what JSC's sys_icache_invalidate runs on the read-execute alias of its JIT region). The hardware reports it
// with WnR = 1, but it needs read access only (Linux's is_write_abort and XNU's inspect_data_abort take it as a read):
// a write here made the kernel refuse the fill of the read-execute page and fail the fault (docs/booter.md 2.6).
#define XS_ESR_EC_SHIFT 26
#define XS_ESR_WNR      (1ull << 6)
#define XS_ESR_CM       (1ull << 8)
static inline uint32_t xs_fault_access(uint64_t esr, uint64_t far, uint64_t pc)
{
  unsigned ec = (unsigned)(esr >> XS_ESR_EC_SHIFT) & 0x3f;
  if (ec == 0x20 || ec == 0x21) return XH_PROT_EXEC;                       // instruction abort (lower EL, same EL)
  if (ec == 0x24 || ec == 0x25) return (esr & XS_ESR_WNR) && !(esr & XS_ESR_CM) ? XH_PROT_WRITE : XH_PROT_READ;
  if (far == pc) return XH_PROT_EXEC;
  return 0;
}

_Static_assert(sizeof(struct xs_trap_page) <= XS_OPS_OFF, "the trap page is one 4 KiB page");
_Static_assert(XS_OPS_OFF + XS_THREAD_OPS * sizeof(struct xs_op) <= XS_SLOT_SIZE, "a thread's deferred maps fit its slot");
_Static_assert(sizeof(struct xs_service_page) <= XS_SLOT_SIZE, "the service page fits a slot");
_Static_assert(sizeof(struct xs_config) <= 0x1000, "the config fits a page");

#endif
