// SPDX-License-Identifier: MIT
// The treble backend's HAL client and the HALs' parcels (hal.h).
#define _GNU_SOURCE
#include "hal.h"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct br_hal {
  ab      *b;
  uint32_t handle;
  int32_t  version;
};

void ab_put_float(ab_parcel *p, float value) { int32_t bits; memcpy(&bits, &value, 4); ab_put_i32(p, bits); }
float ab_get_float(ab_reader *r) { int32_t bits = ab_get_i32(r); float v; memcpy(&v, &bits, 4); return v; }
void ab_put_i64(ab_parcel *p, int64_t value) { ab_put_i32(p, (int32_t)(uint32_t)value); ab_put_i32(p, (int32_t)(uint32_t)((uint64_t)value >> 32)); }
int64_t ab_get_i64(ab_reader *r)
{
  uint32_t low = (uint32_t)ab_get_i32(r), high = (uint32_t)ab_get_i32(r);
  return (int64_t)((uint64_t)high << 32 | low);
}

void br_hal_parcel(ab_parcel *p, const char *descriptor) { ab_init(p); ab_put_token(p, descriptor); }

const char *br_hal_error(int rc)
{
  static _Thread_local char text[64];
  if (rc == -ENOENT) return "not registered with servicemanager, or SELinux denies the lookup";
  if (rc == -ETIMEDOUT) return "no answer in 5 s";
  if (rc == -EPIPE) return "the HAL's process died";
  if (rc == -EREMOTEIO) return "the HAL answered with an exception";
  if (rc == -EPROTO) return "a reply this client cannot read";
  snprintf(text, sizeof text, "%s", strerror(rc < 0 ? -rc : rc));
  return text;
}

struct taken { ab *b; uint32_t handle; bool have; };
static int take_handle(void *ctx, ab_reader *r)
{
  struct taken *t = ctx;
  if (!ab_get_binder(r, &t->handle, NULL)) return r->bad ? -EPROTO : -ENOENT;   // a null binder: not registered
  int rc = ab_acquire(t->b, t->handle);          // ours, before the reply's buffer goes back
  if (!rc) t->have = true;
  return rc;
}
struct named { const char *want; bool same; };
static int take_descriptor(void *ctx, ab_reader *r)
{
  struct named *n = ctx;
  char text[96];
  if (!ab_get_string(r, text, sizeof text)) return -EPROTO;
  n->same = !strcmp(text, n->want);
  return 0;
}
static int take_i32(void *ctx, ab_reader *r) { *(int32_t *)ctx = ab_get_i32(r); return r->bad ? -EPROTO : 0; }

br_hal *br_hal_open(const char *device, const char *instance, const char *descriptor, char *why, size_t size)
{
  br_hal *h = calloc(1, sizeof *h);
  if (!h) { snprintf(why, size, "no memory"); return NULL; }
  h->b = ab_open(device, NULL, NULL, NULL);
  if (!h->b) {
    snprintf(why, size, "%s: no binder device (%s: %s)", instance, device ? device : getenv("ONEIOS_BINDER") ? getenv("ONEIOS_BINDER") : "/dev/binder", strerror(errno));
    free(h);
    return NULL;
  }
  ab_parcel p;
  br_hal_parcel(&p, HAL_SERVICE_MANAGER);
  ab_put_string(&p, instance);
  struct taken t = { .b = h->b };
  int32_t specific = 0;
  int rc = ab_call(h->b, 0, HAL_SM_GET_SERVICE, &p, &specific, take_handle, &t);
  if (rc || !t.have) {
    snprintf(why, size, "%s (no service: %s)", instance, br_hal_error(rc ? rc : -ENOENT));
    ab_close(h->b);
    free(h);
    return NULL;
  }
  h->handle = t.handle;
  struct named n = { .want = descriptor };
  ab_init(&p);
  rc = ab_call(h->b, h->handle, AB_INTERFACE, &p, &specific, take_descriptor, &n);
  if (rc || !n.same) {
    snprintf(why, size, "%s (%s)", instance, rc ? br_hal_error(rc) : "it is not a service of that interface");
    br_hal_close(h);
    return NULL;
  }
  ab_init(&p);
  ab_put_token(&p, descriptor);
  if (ab_call(h->b, h->handle, AB_GET_VERSION, &p, &specific, take_i32, &h->version)) h->version = 0;
  return h;
}

int32_t br_hal_version(const br_hal *h) { return h->version; }

int br_hal_call(br_hal *h, uint32_t code, const ab_parcel *p, ab_reply_fn on_reply, void *ctx)
{
  int32_t specific = 0;
  return ab_call(h->b, h->handle, code, p, &specific, on_reply, ctx);
}

void br_hal_close(br_hal *h)
{
  if (!h) return;
  if (h->handle) ab_release(h->b, h->handle);
  ab_close(h->b);
  free(h);
}

// ---- health ---------------------------------------------------------------------------------------
// A parcelable array's elements: each a non-null marker and a sized body; skipped whole.
static bool skip_parcelables(ab_reader *r)
{
  int32_t n = ab_get_i32(r);
  if (r->bad || n < -1 || n > 4096) { r->bad = true; return false; }
  for (int32_t i = 0; i < n; i++) {
    size_t end = ab_get_parcelable(r);
    if (!end) return false;
    r->position = end;
  }
  return true;
}

int hal_health_decode(ab_reader *r, hal_health_info *o)
{
  memset(o, 0, sizeof *o);
  size_t end = ab_get_parcelable(r);
  if (!end) return -EPROTO;
#define MORE() (r->position + 4 <= end && !r->bad)
  o->ac = ab_get_i32(r); o->usb = ab_get_i32(r); o->wireless = ab_get_i32(r);
  o->dock = ab_get_i32(r);
  o->max_charging_ua = ab_get_i32(r); o->max_charging_uv = ab_get_i32(r);
  o->status = ab_get_i32(r); o->health = ab_get_i32(r);
  o->present = ab_get_i32(r);
  o->level = ab_get_i32(r);
  o->voltage_mv = ab_get_i32(r); o->temp_deci_c = ab_get_i32(r); o->current_ua = ab_get_i32(r);
  o->cycles = ab_get_i32(r); o->full_uah = ab_get_i32(r); o->counter_uah = ab_get_i32(r);
  if (!ab_get_string(r, o->technology, sizeof o->technology)) return -EPROTO;
  if (MORE()) o->current_average_ua = ab_get_i32(r);
  if (MORE() && !skip_parcelables(r)) return -EPROTO;           // diskStats
  if (MORE() && !skip_parcelables(r)) return -EPROTO;           // storageInfos
  if (MORE()) o->capacity_level = ab_get_i32(r);
  if (r->position + 8 <= end) o->time_to_full_s = ab_get_i64(r);
  if (MORE()) o->design_uah = ab_get_i32(r);
#undef MORE
  if (r->bad || r->position > end) return -EPROTO;
  r->position = end;                                              // what a newer HAL added (chargingState, ...)
  return 0;
}

void hal_health_encode(ab_parcel *p, const hal_health_info *in)
{
  ab_put_i32(p, 1);
  size_t at = p->size;
  ab_put_i32(p, 0);
  ab_put_i32(p, in->ac); ab_put_i32(p, in->usb); ab_put_i32(p, in->wireless); ab_put_i32(p, in->dock);
  ab_put_i32(p, in->max_charging_ua); ab_put_i32(p, in->max_charging_uv);
  ab_put_i32(p, in->status); ab_put_i32(p, in->health); ab_put_i32(p, in->present); ab_put_i32(p, in->level);
  ab_put_i32(p, in->voltage_mv); ab_put_i32(p, in->temp_deci_c); ab_put_i32(p, in->current_ua);
  ab_put_i32(p, in->cycles); ab_put_i32(p, in->full_uah); ab_put_i32(p, in->counter_uah);
  ab_put_string(p, in->technology);
  ab_put_i32(p, in->current_average_ua);
  ab_put_i32(p, 0); ab_put_i32(p, 0);                             // no diskStats, no storageInfos
  ab_put_i32(p, in->capacity_level);
  ab_put_i64(p, in->time_to_full_s);
  ab_put_i32(p, in->design_uah);
  ab_put_i32(p, 1); ab_put_i32(p, 1);                             // chargingState NORMAL, chargingPolicy DEFAULT (v2)
  int32_t size = (int32_t)(p->size - at);
  if (!p->overflow) memcpy(p->data + at, &size, 4);
}

// ---- thermal --------------------------------------------------------------------------------------
int hal_temperatures_decode(ab_reader *r, hal_temperature *out, unsigned max)
{
  int32_t n = ab_get_i32(r);
  if (r->bad || n < -1 || n > 4096) return -EPROTO;
  unsigned kept = 0;
  for (int32_t i = 0; i < n; i++) {
    size_t end = ab_get_parcelable(r);
    if (!end) return -EPROTO;
    hal_temperature t = { 0 };
    t.type = ab_get_i32(r);
    if (!ab_get_string(r, t.name, sizeof t.name)) return -EPROTO;
    t.celsius = ab_get_float(r);
    if (r->position + 4 <= end) t.throttling = ab_get_i32(r);
    if (r->bad || r->position > end) return -EPROTO;
    r->position = end;
    if (kept < max) out[kept++] = t;
  }
  return (int)kept;
}

void hal_temperatures_encode(ab_parcel *p, const hal_temperature *in, unsigned n)
{
  ab_put_i32(p, (int32_t)n);
  for (unsigned i = 0; i < n; i++) {
    ab_put_i32(p, 1);
    size_t at = p->size;
    ab_put_i32(p, 0);
    ab_put_i32(p, in[i].type);
    ab_put_string(p, in[i].name);
    ab_put_float(p, in[i].celsius);
    ab_put_i32(p, in[i].throttling);
    int32_t size = (int32_t)(p->size - at);
    if (!p->overflow) memcpy(p->data + at, &size, 4);
  }
}
