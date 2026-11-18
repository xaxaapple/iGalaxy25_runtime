// SPDX-License-Identifier: MIT
// The hardware backend layer's log lines (bridges/hw_backend.h): which backend each bridge uses and why, and the one
// line of a treble interface that is missing.
#include "bridges/bridges.h"
#include "bridges/hw_backend.h"

int br_hw_select(unsigned bridge, int forced)
{
  char why[64];
  int backend = br_hw_choose(br_hw_current(), bridge, forced, why, sizeof why);
  br_log(1, "hw: %s: backend %s (%s)", br_hw_bridge_name(bridge), br_hw_name(backend), why);
  return backend;
}

bool br_hw_missing(unsigned bridge, const char *what)
{
  bool fallback = br_hw_fallback_allowed(br_hw_current());
  br_log(fallback ? 1 : 0, "hw: %s: treble: missing: %s; %s", br_hw_bridge_name(bridge), what,
         fallback ? "falling back to samsung (hw-backend-fallback=1)" : "the bridge stays off (hw-backend-fallback=0)");
  return fallback;
}
