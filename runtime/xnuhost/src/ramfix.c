// SPDX-License-Identifier: MIT
// The guest's RAM, held fixed for the session (docs/booter.md 2.2, "Fixed RAM"; the owner's decision of 2026-11-13: iOS's
// RAM must not grow and shrink during a session; the default since fix-ram-locked, 2026-11-15: iOS's RAM is never swapped).
// xnuhost takes the whole memfd once, at the start: a thread of its own at nice 19 prefaults it in steps (mlock, which
// also locks it, where Linux allows; where it does not the prefault stops), and nothing is given back until the process
// ends (ram_discard is refused; kernel 0067 does not ask). The main loop runs the valve. The exit frees all of it at once (the
// memfd's last mapping and descriptor go with the session's processes). Portable: the host tests run it on memfds of
// their own, with a failing mlock put in.
//
// Never: a realtime class, a lower OOM score, another cgroup, a write outside the process (docs/booter.md 2.9). Read only:
// /proc/meminfo, /proc/vmstat, /proc/self/{status,stat,cgroup}, the cgroup's memory files, KGSL's per-process files.
#define _GNU_SOURCE 1
#include "xnuhost.h"
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

xh_ramfix g_ramfix;
int (*xh_ramfix_mlock)(const void *, size_t) = mlock;
int (*xh_ramfix_munlock)(const void *, size_t) = munlock;

static uint64_t ms_now(void)
{
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000u + (uint64_t)t.tv_nsec / 1000000u;
}

// One step: locked (and so in the host's RAM) when the lock still holds, else read once a page through the mapping (a
// read of a hole through a shared mapping fills it with a zeroed page: shmem has no shared zero page). Neither changes a
// byte the guest wrote. Not fallocate: the pages it allocates stay "not up to date" until first touched, so mincore (the
// `memory:` line's "in the host's RAM") does not count them, and the guest's first touch still takes a fault.
// False when the prefault should stop here: the lock was asked for and failed. A page prefaulted and not locked is one
// iOS has not used yet, so it is the coldest page there is and the first the host swaps out (the PC, 2026-11-14: 841 MiB
// of a prefaulted, unlocked 4 GiB guest swapped out in 15 min, against 6 MiB faulted in as used); the rest is faulted
// in as iOS uses it.
// What was locked before the failure stays locked (fix-ram-locked, 2026-11-15; before, it was unlocked again, "all or
// nothing"). Unlocking it only made those pages prefaulted-and-unlocked, the very pages the host swaps first; the part
// that could not be locked is faulted in as used either way. Where the lock fails at the first step (EPERM: no
// CAP_IPC_LOCK; ENOMEM: an RLIMIT_MEMLOCK under one step, the PC's 8 MiB) nothing was locked and nothing is prefaulted.
// Where it fails part way because the host is short, the valve (MemAvailable under ram-unlock-below) decides as for a
// whole lock.
static bool step(xh_ramfix *r, uint8_t *base, uint64_t at, uint64_t length)
{
  if (r->locking) {
    if (xh_ramfix_mlock(base + at, (size_t)length) == 0) { __atomic_store_n(&r->locked, at + length, __ATOMIC_RELAXED); return true; }
    r->lock_error = errno ? errno : EINVAL;
    r->locking = false;
    return false;
  }
  long page = sysconf(_SC_PAGESIZE);
  for (uint64_t i = 0; i < length; i += (uint64_t)page) (void)*(volatile uint8_t *)(base + at + i);
  return true;
}

void xh_ramfix_prefault(xh_ramfix *r, uint8_t *base, uint64_t size)
{
  uint64_t chunk = r->step ? r->step : XH_RAMFIX_STEP, t0 = ms_now();
  r->size = size;
  r->locking = r->lock;
  r->lock_error = 0;
  for (uint64_t at = 0; at < size && !__atomic_load_n(&r->stop, __ATOMIC_RELAXED); at += chunk) {
    uint64_t length = size - at < chunk ? size - at : chunk;
    // The host's MemAvailable before each step (one read of /proc/meminfo per 16 MiB): what fits was reckoned at the
    // start, and if the host is short now the prefault stops where it is; the valve then decides about what is locked.
    uint64_t total = 0, available = 0;
    if (r->meminfo && r->unlock_below && xh_meminfo(r->meminfo, &total, &available) == 0 && available < r->unlock_below) {
      r->short_stop = true;
      r->short_available = available;
      break;
    }
    if (!step(r, base, at, length)) break;
    __atomic_store_n(&r->prefaulted, at + length, __ATOMIC_RELAXED);
  }
  r->ms = ms_now() - t0;
  __atomic_store_n(&r->done, true, __ATOMIC_RELEASE);
}

bool xh_ramfix_valve(xh_ramfix *r, uint8_t *base, uint64_t available)
{
  uint64_t locked = __atomic_load_n(&r->locked, __ATOMIC_RELAXED);
  if (!locked || r->unlocked || !r->unlock_below || available >= r->unlock_below) return false;
  (void)xh_ramfix_munlock(base, (size_t)locked);
  r->unlocked = true;
  r->unlock_available = available;
  __atomic_store_n(&r->locked, 0, __ATOMIC_RELAXED);
  return true;
}

// From the main loop (nice 0) rather than the prefault's nice-19 thread: under load (eight busy vCPUs at nice 0) a nice-19
// thread gets a few percent of a core and could wait seconds to run, which is when the valve is wanted. One read of
// /proc/meminfo a second while something is locked, nothing otherwise; never during the prefault (it checks itself).
bool xh_ramfix_valve_tick(xh_ramfix *r, uint8_t *base, uint64_t *available)
{
  if (!__atomic_load_n(&r->done, __ATOMIC_ACQUIRE) || r->unlocked || !r->unlock_below || !r->meminfo ||
      !__atomic_load_n(&r->locked, __ATOMIC_RELAXED))
    return false;
  uint64_t total = 0, now = 0;
  if (xh_meminfo(r->meminfo, &total, &now)) return false;
  if (available) *available = now;
  return xh_ramfix_valve(r, base, now);
}

uint64_t xh_ramfix_unlocked_from(const xh_ramfix *r, uint64_t vm_locked)
{
  uint64_t locked = __atomic_load_n(&r->locked, __ATOMIC_RELAXED);
  // VmLck is the size of this process's locked mappings: when it does not cover what the prefault locked (it should
  // always), count everything.
  return locked && !r->unlocked && vm_locked >= locked ? locked : 0;
}

// ---- what Linux says about locking here (read only), once at the start ----------------------------------------------
static bool read_line(const char *path, char *out, size_t size)
{
  FILE *f = fopen(path, "r");
  if (!f) return false;
  bool ok = fgets(out, (int)size, f) != NULL;
  fclose(f);
  if (ok) out[strcspn(out, "\n")] = 0;
  return ok;
}

// CapEff's bit 14 (CAP_IPC_LOCK: mlock past RLIMIT_MEMLOCK).
static int cap_ipc_lock(void)
{
  FILE *f = fopen("/proc/self/status", "r");
  char line[256];
  int have = -1;
  while (f && fgets(line, sizeof line, f))
    if (!strncmp(line, "CapEff:", 7)) { have = (int)(strtoull(line + 7, NULL, 16) >> 14 & 1); break; }
  if (f) fclose(f);
  return have;
}

// The size ram=auto may take depends on it (config.c: the headroom with the lock is smaller, there is no zram working
// set for the guest), so it is asked before the size is known: a lock of any size works with either.
static bool can_lock(void)
{
  struct rlimit l = { 0, 0 };
  return cap_ipc_lock() == 1 || (getrlimit(RLIMIT_MEMLOCK, &l) == 0 && l.rlim_cur == RLIM_INFINITY);
}
bool (*xh_ram_can_lock)(void) = can_lock;

void xh_ramfix_facts(char *out, size_t size)
{
  struct rlimit l = { 0, 0 };
  char limit[32], cg[256] = "?", path[320], max[32] = "?", swap[32] = "?", high[32] = "?";
  getrlimit(RLIMIT_MEMLOCK, &l);
  if (l.rlim_cur == RLIM_INFINITY) snprintf(limit, sizeof limit, "unlimited");
  else snprintf(limit, sizeof limit, "%llu KiB", (unsigned long long)(l.rlim_cur >> 10));
  // cgroup v2: "0::/path"; the memory controller's limits of that cgroup, if it has them
  char line[256];
  FILE *f = fopen("/proc/self/cgroup", "r");
  while (f && fgets(line, sizeof line, f))
    if (!strncmp(line, "0::", 3)) { line[strcspn(line, "\n")] = 0; snprintf(cg, sizeof cg, "%s", line + 3); break; }
  if (f) fclose(f);
  if (strcmp(cg, "?")) {
    snprintf(path, sizeof path, "/sys/fs/cgroup%s/memory.max", cg);       if (!read_line(path, max, sizeof max)) strcpy(max, "-");
    snprintf(path, sizeof path, "/sys/fs/cgroup%s/memory.swap.max", cg);  if (!read_line(path, swap, sizeof swap)) strcpy(swap, "-");
    snprintf(path, sizeof path, "/sys/fs/cgroup%s/memory.high", cg);      if (!read_line(path, high, sizeof high)) strcpy(high, "-");
  }
  int cap = cap_ipc_lock();
  snprintf(out, size, "RLIMIT_MEMLOCK %s, CAP_IPC_LOCK %s; cgroup %s (memory.max %s, memory.high %s, memory.swap.max %s)", limit,
           cap < 0 ? "?" : cap ? "yes" : "no", cg, max, high, swap);
}

// ---- 2 MiB host pages (fix-ram-hugepages, 2026-11-15) -----------------------------------------------------------------
// The RAM is a memfd (udmabuf takes no other file) that every iOS task maps in 16 KiB pieces. So of Linux's 2 MiB pages:
// - shmem THP: a memfd is on the kernel's internal shmem mount, governed by shmem_enabled; with "advise" a mapping with
//   MADV_HUGEPAGE gets 2 MiB folios, "always"/"within_size"/"force" give them anyway, "never" (the kernel's default) and
//   "deny" never. A task's 16 KiB mapping of a 2 MiB folio is mapped with 4 KiB entries, which Linux allows.
// - hugetlb (MFD_HUGETLB): every mmap of such a file must be 2 MiB-aligned in offset and length, which a task's 16 KiB
//   page is not, and GKI 6.1's defconfig has no CONFIG_HUGETLBFS. Read for the log only.
// - Nothing global is written here (the phone's rule: what is changed is restored): where shmem_enabled is "never", the
//   OneOS fork's switch would write "advise" and its return "never" (oneos/interface.md); xnuhost only reads.
void xh_huge_choose(const char *root, bool want, xh_huge_choice *c)
{
  char path[320], text[160];
  memset(c, 0, sizeof *c);
  if (!root) root = "";
  snprintf(path, sizeof path, "%s/sys/kernel/mm/transparent_hugepage/shmem_enabled", root);
  if (read_line(path, text, sizeof text)) {
    // "always within_size [advise] never deny force": the bracketed one is in force
    char *open = strchr(text, '['), *close = open ? strchr(open, ']') : NULL;
    if (open && close && close - open - 1 < (long)sizeof c->shmem_enabled) {
      memcpy(c->shmem_enabled, open + 1, (size_t)(close - open - 1));
      c->shmem_enabled[close - open - 1] = 0;
    }
  }
  snprintf(path, sizeof path, "%s/sys/kernel/mm/transparent_hugepage/hpage_pmd_size", root);
  if (read_line(path, text, sizeof text)) c->pmd_size = strtoull(text, NULL, 10);
  snprintf(path, sizeof path, "%s/proc/meminfo", root);
  static const char *const keys[2] = { "HugePages_Free", "Hugepagesize" };
  uint64_t v[2] = { 0, 0 };
  (void)xh_proc_values(path, keys, 2, v);
  c->hugetlb_free = v[0];
  c->hugetlb_size = v[1];
  const char *s = c->shmem_enabled;
  if (!want) snprintf(c->why, sizeof c->why, "not asked (ram-huge=0, or ram-giveback=1: a give-back's pieces would split them)");
  else if (c->pmd_size != (2ull << 20))
    snprintf(c->why, sizeof c->why, "no (hpage_pmd_size %llu: no transparent huge pages of 2 MiB on this kernel)", (unsigned long long)c->pmd_size);
  else if (!strcmp(s, "advise") || !strcmp(s, "always") || !strcmp(s, "within_size") || !strcmp(s, "force"))
    c->mode = XH_HUGE_SHMEM;
  else if (!*s) snprintf(c->why, sizeof c->why, "no (shmem_enabled unreadable)");
  else snprintf(c->why, sizeof c->why, "no (shmem_enabled %s: a memfd gets 4 KiB pages; the fork's switch would set advise and its "
                "return restore %s)", s, s);
}

// ---- the counters of the `memory:` lines ------------------------------------------------------------------------------
// This process's memory mapped with 2 MiB entries (/proc/self/smaps_rollup: ShmemPmdMapped, FilePmdMapped,
// AnonHugePages). The kernel walks every mapping of the process for it (the kernel's heap and the tasks' maps are many),
// so it is read at the prefault's end and at the stop only, not every host-mem-log.
bool xh_pmd_mapped_self(uint64_t *shmem, uint64_t *file, uint64_t *anon)
{
  static const char *const keys[3] = { "ShmemPmdMapped", "FilePmdMapped", "AnonHugePages" };
  uint64_t v[3] = { 0, 0, 0 };
  unsigned found = xh_proc_values("/proc/self/smaps_rollup", keys, 3, v);
  *shmem = v[0];
  *file = v[1];
  *anon = v[2];
  return found > 0;
}

bool xh_vmstat_swaps(uint64_t *ins, uint64_t *outs)
{
  static const char *const keys[2] = { "pswpin", "pswpout" };
  uint64_t v[2] = { 0, 0 };
  bool ok = xh_proc_values("/proc/vmstat", keys, 2, v) == 2;
  *ins = v[0];
  *outs = v[1];
  return ok;
}

void xh_ramfix_sample(xh_ramfix_counters *c)
{
  memset(c, 0, sizeof *c);
  static const char *const keys[2] = { "VmLck", "VmPTE" };
  uint64_t v[2] = { 0, 0 };
  (void)xh_proc_values("/proc/self/status", keys, 2, v);
  c->vm_locked = v[0];
  c->vm_pte = v[1];
  (void)xh_vmstat_swaps(&c->swap_ins, &c->swap_outs);
  // /proc/self/stat: minflt is the 10th field and majflt the 12th, after "(comm)" which may hold spaces
  char text[1024];
  FILE *f = fopen("/proc/self/stat", "r");
  size_t got = f ? fread(text, 1, sizeof text - 1, f) : 0;
  if (f) fclose(f);
  text[got] = 0;
  char *p = strrchr(text, ')');
  for (int field = 2; p && field < 12; field++) {
    p = strchr(p + 1, ' ');
    if (p && field == 9) c->minor_faults = strtoull(p + 1, NULL, 10);
  }
  if (p) c->major_faults = strtoull(p + 1, NULL, 10);
}

size_t xh_kgsl_memory(int pid, char *out, size_t size)
{
  char dir[96], path[400], value[64];
  snprintf(dir, sizeof dir, "/sys/class/kgsl/kgsl/proc/%d", pid);
  DIR *d = opendir(dir);
  size_t used = 0;
  if (!d || !size) { if (d) closedir(d); if (size) *out = 0; return 0; }
  *out = 0;
  struct dirent *e;
  unsigned shown = 0;
  while ((e = readdir(d)) && shown < 12) {
    if (e->d_name[0] == '.') continue;
    snprintf(path, sizeof path, "%s/%s", dir, e->d_name);
    char *end;
    if (!read_line(path, value, sizeof value)) continue;
    unsigned long long v = strtoull(value, &end, 10);
    if (end == value || (*end && *end != ' ')) continue;                 // one number each, as KGSL prints them
    int n = snprintf(out + used, size - used, "%s%s %llu", shown ? ", " : "", e->d_name, v);
    if (n < 0 || (size_t)n >= size - used) break;
    used += (size_t)n;
    shown++;
  }
  closedir(d);
  return used;
}

// ---- the thread ---------------------------------------------------------------------------------------------------------
static pthread_t thread;
static bool thread_on;
static uint8_t *ram_base;

static void say_done(xh_ramfix *r)
{
  char why[400], short_by[160] = "";
  uint64_t locked = __atomic_load_n(&r->locked, __ATOMIC_RELAXED);
  if (r->short_stop)
    snprintf(short_by, sizeof short_by, "; stopped at %llu MiB: host MemAvailable %llu MiB is under ram-unlock-below (%llu MiB)",
             (unsigned long long)(r->prefaulted >> 20), (unsigned long long)(r->short_available >> 20), (unsigned long long)(r->unlock_below >> 20));
  if (locked && locked == r->size) snprintf(why, sizeof why, "locked (mlock: the host cannot swap it)");
  else if (locked && r->lock_error)
    snprintf(why, sizeof why, "%llu MiB of it locked (mlock: %s past that; what is locked stays locked), not prefaulted further: the rest "
             "is faulted in as iOS uses it", (unsigned long long)(locked >> 20), strerror(r->lock_error));
  else if (locked) snprintf(why, sizeof why, "%llu MiB of it locked", (unsigned long long)(locked >> 20));
  else if (!r->lock) snprintf(why, sizeof why, "not locked (%s)", r->lock_why ? r->lock_why : "ram-lock=0");
  else if (r->lock_error)
    snprintf(why, sizeof why, "not locked (mlock: %s), so not prefaulted: the guest's RAM is faulted in as iOS uses it (an unlocked, "
             "unused page is the first the host swaps)", strerror(r->lock_error));
  else snprintf(why, sizeof why, "not locked");
  LOGI("memory: the guest's RAM is held: %llu MiB prefaulted in %llu ms, %s%s%s", (unsigned long long)(r->prefaulted >> 20),
       (unsigned long long)r->ms, why, r->lock ? "" : ", read once a page", short_by);
}

static void *ramfix_main(void *arg)
{
  xh_ramfix *r = arg;
  // Below every vCPU and task thread (nice 0): a prefault takes no CPU time the guest wants. SCHED_OTHER, never realtime.
  // The thread ends with the prefault; the valve runs in the main loop (xh_ramfix_valve_tick), not at nice 19.
  (void)setpriority(PRIO_PROCESS, (id_t)syscall(SYS_gettid), 19);
  xh_ramfix_prefault(r, ram_base, r->size);
  if (!__atomic_load_n(&r->stop, __ATOMIC_RELAXED)) say_done(r);
  return NULL;
}

int xh_ramfix_start(xh_ramfix *r, uint8_t *base, uint64_t size, const char *meminfo)
{
  ram_base = base;
  r->size = size;
  if (meminfo && *meminfo) r->meminfo = meminfo;
  else if (!r->meminfo) r->meminfo = "/proc/meminfo";
  int rc = pthread_create(&thread, NULL, ramfix_main, r);
  if (rc) { LOGE("memory: no thread for the prefault: %s", strerror(rc)); return -1; }
  pthread_setname_np(thread, "xnuhost-ram");
  thread_on = true;
  return 0;
}

void xh_ramfix_stop(xh_ramfix *r)
{
  __atomic_store_n(&r->stop, true, __ATOMIC_RELAXED);
  if (thread_on) pthread_join(thread, NULL);
  thread_on = false;
}
