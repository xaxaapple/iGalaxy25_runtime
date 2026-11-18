// SPDX-License-Identifier: MIT
// The treble backend's HAL client: a stable AIDL HAL asked for by name from servicemanager (handle 0 of /dev/binder,
// IServiceManager.getService), its descriptor and interface version checked, then plain calls. The binder code is the
// Wi-Fi supplicant's (wifi/aidl.h): no Android library. And the parcels of the HALs the bridges use, both ways (the
// encoders serve the fake HAL of the tests and the stand-in phone, tests/fake_hal.c).
//
// The method codes are the order of each interface's frozen AIDL file in AOSP (hardware/interfaces/.../aidl): a
// stable AIDL interface only appends methods, so they hold for every version. [I] until a phone answers: the open
// checks the descriptor (`_NTF`) and logs the version, so a wrong object is refused before any call.
#ifndef ONEIOS_BRIDGES_HAL_H
#define ONEIOS_BRIDGES_HAL_H

#include "../wifi/aidl.h"
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define HAL_SERVICE_MANAGER "android.os.IServiceManager"
#define HAL_SM_GET_SERVICE  1u                    // IServiceManager.getService(String): IBinder

// android.hardware.health.IHealth (health/aidl/android/hardware/health/IHealth.aidl)
#define HAL_HEALTH          "android.hardware.health.IHealth"
#define HAL_HEALTH_INSTANCE "android.hardware.health.IHealth/default"
#define HAL_HEALTH_GET_INFO 12u                   // getHealthInfo(): HealthInfo (after registerCallback .. getDiskStats)
// android.hardware.thermal.IThermal (thermal/aidl/android/hardware/thermal/IThermal.aidl)
#define HAL_THERMAL         "android.hardware.thermal.IThermal"
#define HAL_THERMAL_INSTANCE "android.hardware.thermal.IThermal/default"
#define HAL_THERMAL_GET_TEMPERATURES 3u           // getTemperatures(): Temperature[]
// android.hardware.vibrator.IVibrator (vibrator/aidl/android/hardware/vibrator/IVibrator.aidl)
#define HAL_VIBRATOR        "android.hardware.vibrator.IVibrator"
#define HAL_VIBRATOR_INSTANCE "android.hardware.vibrator.IVibrator/default"
#define HAL_VIB_GET_CAPABILITIES 1u               // getCapabilities(): int
#define HAL_VIB_OFF          2u                   // off()
#define HAL_VIB_ON           3u                   // on(int timeoutMs, IVibratorCallback callback): callback null here
#define HAL_VIB_SET_AMPLITUDE 6u                  // setAmplitude(float 0 < a <= 1)
#define HAL_VIB_CAP_AMPLITUDE 4                   // IVibrator.CAP_AMPLITUDE_CONTROL

// ---- the connection -----------------------------------------------------------------------------
typedef struct br_hal br_hal;
// Opens the binder device (NULL: ONEIOS_BINDER, else /dev/binder), asks servicemanager for `instance` (no wait: a
// HAL that is not registered is missing), checks that it answers `descriptor`. NULL with the reason in why.
br_hal *br_hal_open(const char *device, const char *instance, const char *descriptor, char *why, size_t size);
int32_t br_hal_version(const br_hal *h);         // the interface version it reported (0: none)
// A call: 0, or -errno (-EPIPE: the HAL's process died; -EREMOTEIO: it answered with an exception).
int     br_hal_call(br_hal *h, uint32_t code, const ab_parcel *p, ab_reply_fn on_reply, void *ctx);
void    br_hal_close(br_hal *h);
const char *br_hal_error(int rc);                 // a few words for a log line
void    br_hal_parcel(ab_parcel *p, const char *descriptor);   // a call's parcel: the interface token
void    ab_put_float(ab_parcel *p, float value);
float   ab_get_float(ab_reader *r);
int64_t ab_get_i64(ab_reader *r);
void    ab_put_i64(ab_parcel *p, int64_t value);

// ---- health: HealthInfo (HealthInfo.aidl, the fields this bridge reads, in their order) ---------
enum { HAL_BATTERY_UNKNOWN = 1, HAL_BATTERY_CHARGING = 2, HAL_BATTERY_DISCHARGING = 3, HAL_BATTERY_NOT_CHARGING = 4,
       HAL_BATTERY_FULL = 5 };                                     // BatteryStatus
typedef struct hal_health_info {
  bool    ac, usb, wireless, dock;                                 // charger*Online
  int32_t max_charging_ua, max_charging_uv;
  int32_t status, health;                                          // BatteryStatus, BatteryHealth
  bool    present;
  int32_t level;                                                   // percent
  int32_t voltage_mv, temp_deci_c, current_ua, cycles, full_uah, counter_uah;
  char    technology[24];
  int32_t current_average_ua;
  int32_t capacity_level;                                          // BatteryCapacityLevel (v1)
  int64_t time_to_full_s;
  int32_t design_uah;                                              // batteryFullChargeDesignCapacityUah (0: not sent)
} hal_health_info;
// The reply of getHealthInfo after its status header (a non-null parcelable). 0 or -EPROTO. Fields past the end of
// an older HAL's parcelable stay 0; fields a newer one adds are skipped.
int  hal_health_decode(ab_reader *r, hal_health_info *out);
void hal_health_encode(ab_parcel *p, const hal_health_info *in);  // the fake HAL's

// ---- thermal: Temperature[] (Temperature.aidl: type, name, value in degrees C, throttlingStatus) -
enum { HAL_TEMP_CPU = 0, HAL_TEMP_GPU = 1, HAL_TEMP_BATTERY = 2, HAL_TEMP_SKIN = 3, HAL_TEMP_SOC = 13 };   // TemperatureType
#define HAL_TEMPS_MAX 64u
typedef struct hal_temperature { int32_t type; char name[24]; float celsius; int32_t throttling; } hal_temperature;
// The reply of getTemperatures after its status header. The number read (at most max), or -EPROTO.
int  hal_temperatures_decode(ab_reader *r, hal_temperature *out, unsigned max);
void hal_temperatures_encode(ab_parcel *p, const hal_temperature *in, unsigned n);

#endif
