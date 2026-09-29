# Bridging decisions

## Overview

Not everything a DNSP can request through CSIP-AUS has a direct counterpart in
AS5438 or the SunSpec models. This file records the choices this crate makes
about how those requests are applied to the device. Interpretations of AS5438
itself are in `AS5438_comments.md`.

## Default ramp rate (setGradW)

`setGradW` is written to the device's own ramp rate, Model 704 `WRmp`, with
`WRmpRef` set to `WMax`. The device ramps by itself, so the bridge doesn't need
to know `WMax`.

`WRmp` is a `uint16` in whole percent per second with no scale factor, whereas
`setGradW` is in hundredths of a percent per second. A small rate such as the
AS4777.2 default of about 0.28 %/s (`setGradW = 27`) would truncate to 0, which
means "no limit". The conversion therefore rounds up, so any non-zero
`setGradW` becomes at least 1 %/s. `setGradW = 0` stays 0.

Note that `WRmp` only applies to increases in active power.

## Soft-start ramp rate (setSoftGradW)

The device already ramps when it enters service, using Model 703 `EsRmpTms`
(the enter service ramp time in seconds). When `setESRampTms` is given it takes
priority. Otherwise `setSoftGradW` is converted to a ramp time over the full
range (100 %):

```
EsRmpTms [s] = 10000 / setSoftGradW [hundredths of %/s]
```

`setSoftGradW = 0` (no limit) becomes an `EsRmpTms` of 0.

## Reporting ramp rates in DERSettings

The values reported to the server are read back from the device:

- `setGradW` is `WRmp × 100`. When the device doesn't report `WRmp`, the bridge
  reports 0 because `setGradW` is mandatory in DERSettings.
- `setSoftGradW` is derived from `EsRmpTms` with the same conversion as above,
  which is its own inverse.

Both are lossy because the SunSpec side has coarser integer units. For example,
a `setGradW` of 27 is reported back as 100.

## Control ramp time (rampTms)

SunSpec has no per-control ramp time, so the bridge ramps in software. A `ramp`
task sits between the scheduler and the modbus task:

- Only the active power values are ramped: `WMaxLimPct` (opModMaxLimW),
  `WSetPct` (opModFixedW) and `WSet` (opModTargetW). Every other parameter,
  including the enables and modes, switches immediately.
- The ramp is linear and stepped once per second.
- A new target that arrives mid-ramp starts a new ramp from the present values.
- If there was no maximum limit before, the ramp starts at 100 %. For any other
  missing start or target value, the parameter switches immediately.
- `rampTms` is taken from the new set of controls. When a control ends, the
  controls reverted to usually have no `rampTms`, so the change is immediate,
  although increases are still limited by the device's `WRmp`.
- A ramp in progress is not persisted. After a restart the target applies
  immediately.

While a ramp is in progress, the modbus task writes only Model 704 on each step,
rather than every model (including curves).
