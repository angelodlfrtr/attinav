# Attinav host protocol — version 1

Normative description of the binary protocol spoken by the Attinav Rev B firmware on both of its
host links: the USB CDC-ACM port and the carrier UART (LPUART1). The reference implementation is
`attinav-core/src/protocol.rs`; when this document and the code disagree, the code is the bug
unless this document is clearly out of date. Bump `PROTOCOL_VERSION` (`attinav-core/src/lib.rs`)
on any change that is not backwards compatible.

All multi-byte integers and floats are **little-endian**. `f32` and `f64` are IEEE-754.
Angles are degrees, fields are microtesla (µT), temperatures are °C, times are unix seconds.

## 1. Links

| link      | parameters                                                                      |
|-----------|---------------------------------------------------------------------------------|
| USB       | CDC-ACM, VID `0x1209` / PID `0x0001` (pid.codes test pair, replace before distribution), 64-byte bulk packets. Baud / line settings are ignored. A frame that is a multiple of 64 bytes is followed by a zero-length packet. |
| UART      | LPUART1, 8N1, no flow control, `DeviceConfig::uart_baud` (default 115 200; applied at the next boot). |

The two links are independent: a reply goes back on the link the request arrived on; unsolicited
frames (events, stream samples, temperatures, logs) are sent on every link that is connected. For
USB "connected" means a host has the port open (DTR asserted). The UART is always considered
connected.

## 2. Framing

Every frame is **COBS**-encoded and terminated by one `0x00` delimiter byte. Back-to-back
delimiters are legal idle filler and are ignored. After any corruption a receiver resynchronises by
discarding bytes up to the next delimiter.

Before COBS encoding a frame is:

| offset | size | field     |                                                                |
|-------:|-----:|-----------|----------------------------------------------------------------|
|      0 |    1 | `kind`    | message kind (section 4)                                       |
|      1 |    1 | `seq`     | sequence number chosen by the host, echoed in every reply      |
|      2 |  `n` | `payload` | kind-specific, section 5 and 6                                 |
|  `2+n` |    2 | `crc16`   | CRC-16/CCITT-FALSE over `kind`, `seq` and `payload`            |

CRC-16/CCITT-FALSE: polynomial `0x1021`, init `0xFFFF`, no reflection, no final XOR; check value
for `"123456789"` is `0x29B1`.

Limits (`attinav-core/src/protocol.rs`):

| constant       | value | meaning                                              |
|----------------|------:|------------------------------------------------------|
| `MAX_PAYLOAD`  |  2112 | largest payload (a full `Calibration`)               |
| `MAX_FRAME`    |  2116 | largest frame before COBS                            |
| `MAX_ENCODED`  |  2126 | largest encoded frame including the delimiter        |

A receiver drops a frame (silently on the device, logged through defmt) when it overflows these
limits, has an invalid COBS structure, is shorter than 4 bytes or fails its CRC. Trailing bytes
after a known payload layout are ignored (forward compatibility).

## 3. Request / reply discipline

* Every host request is answered by **at least one frame carrying the same `seq`**: either the
  dedicated reply listed in section 4 or an `Ack`.
* Unsolicited device frames carry `seq = 0`. Hosts should therefore not use `seq = 0` for requests
  if they want to tell replies from unsolicited traffic by `seq` alone (matching on `kind` works in
  every case).
* The device is single-threaded: one request is processed at a time. While a **long operation**
  runs (a shot, a calibration shot, a solve) the device still drains its inbound queue between
  steps: `Ping` and `Abort` are honoured (`Ack Ok`), every other request gets `Ack Busy` and is
  **not** executed. A host must wait for the result of a shot before issuing the next command.
* An unknown `kind` is answered with `Ack Unsupported`; a known kind with a malformed payload with
  `Ack BadPayload`.
* The device keeps two inbound frames buffered per link task; a host should wait for the reply
  before sending the next request (strict request/reply), except for `Abort`.

### `Ack` (kind `0x80`)

| offset | size | field     |                                                     |
|-------:|-----:|-----------|-----------------------------------------------------|
|      0 |    1 | `request` | kind of the request being answered                  |
|      1 |    1 | `status`  | table below                                         |
|      2 |    2 | `detail`  | status- or request-specific (see each request)      |

| status | name              | meaning                                                                 |
|-------:|-------------------|-------------------------------------------------------------------------|
|      0 | `Ok`              | done                                                                    |
|      1 | `Busy`            | a shot or a solve is running                                            |
|      2 | `InvalidArgument` | a field was out of range                                                |
|      3 | `Unsupported`     | unknown kind                                                            |
|      4 | `SensorError`     | a sensor did not answer or refused a setting (`detail` = `sensors_ok` mask where noted) |
|      5 | `Timeout`         | reserved                                                                |
|      6 | `NoCalibration`   | no stored calibration / no solved set pending                           |
|      7 | `FlashError`      | flash erase, write or verify failed                                     |
|      8 | `SolveFailed`     | reserved (solve outcomes travel in `CalSolved`)                         |
|      9 | `NotCalibrating`  | calibration command outside a session                                   |
|     10 | `Aborted`         | the operation was aborted by the host                                   |
|     11 | `BadPayload`      | malformed payload                                                       |
|     12 | `Internal`        | firmware-internal error                                                 |
|     13 | `BadImage`        | firmware image rejected (CRC, size, alignment, sequencing)              |

## 4. Message kinds

### Host → device

| kind   | name          | payload (section 5)  | reply                                                 |
|--------|---------------|----------------------|-------------------------------------------------------|
| `0x01` | `Ping`        | none                 | `Ack Ok`                                              |
| `0x02` | `GetInfo`     | none                 | `Info`                                                |
| `0x03` | `SetTime`     | `u32` unix seconds   | `Ack Ok`                                              |
| `0x04` | `Reset`       | none                 | `Ack Ok`, then the device resets 100 ms later         |
| `0x10` | `Shot`        | `ShotRequest`        | `ShotResult`, then `ShotRaw` ×5 if `WANT_RAW`; or `Ack` (`SensorError` with `detail` = `sensors_ok`, `Busy`, `InvalidArgument`, `Aborted` with `detail` = shot id) |
| `0x11` | `Abort`       | none                 | `Ack Ok`; stops a running shot, solve or stream       |
| `0x12` | `Stream`      | `StreamRequest`      | `Ack Ok` (`detail` = effective rate in Hz), then `Sample` / `Temps` frames |
| `0x20` | `GetConfig`   | none                 | `Config`                                              |
| `0x21` | `SetConfig`   | `DeviceConfig`       | `Ack` (`Ok`, `InvalidArgument`, `SensorError`, `FlashError`) |
| `0x30` | `CalBegin`    | none                 | `Ack Ok`; opens a session, clears the shot buffer and any pending set |
| `0x31` | `CalShot`     | `group:u8 roll:u8`   | `CalShotDone`; or `Ack` (`NotCalibrating`, `SensorError`, `Aborted`, `InvalidArgument` when the buffer is full) |
| `0x32` | `CalRemove`   | `group:u8 roll:u8`   | `Ack` (`Ok` / `InvalidArgument` if no such shot; `detail` = shots left) |
| `0x33` | `CalSolve`    | none                 | `CalSolved` (always, success or error)                |
| `0x34` | `CalStore`    | `id:u32`             | `Ack` (`Ok` with `detail` = set index, `NoCalibration` when nothing is pending, `FlashError`) |
| `0x35` | `CalLoad`     | none                 | `Ack` (`Ok` with `detail` = number of sets, `NoCalibration`) |
| `0x36` | `CalGet`      | none                 | `CalData`                                             |
| `0x37` | `CalSet`      | `Calibration`        | `Ack` (`Ok`, `FlashError`)                            |
| `0x38` | `CalClear`    | none                 | `Ack` (`Ok`, `FlashError`)                            |
| `0x39` | `CalInfo`     | none                 | `CalInfo`                                             |
| `0x3A` | `CalGetShots` | none                 | `CalShotData` × n, then `Ack Ok` (`detail` = n)       |
| `0x3B` | `CalEnd`      | none                 | `Ack Ok`; closes the session, keeps the buffer        |
| `0x40` | `FwBegin`     | `FwBegin`            | `Ack` (`Ok`, `InvalidArgument` for a bad size)        |
| `0x41` | `FwChunk`     | `FwChunk`            | `Ack` (`Ok` with `detail` = bytes received so far / 1024, `BadImage`, `FlashError`) |
| `0x42` | `FwFinish`    | none                 | `Ack` (`Ok` then reset after 200 ms; `BadImage`, `FlashError`) |
| `0x43` | `FwAbort`     | none                 | `Ack Ok`                                              |

### Device → host

| kind   | name          | payload (section 6) | when                                                   |
|--------|---------------|---------------------|--------------------------------------------------------|
| `0x80` | `Ack`         | `Ack`               | reply                                                  |
| `0x81` | `Info`        | `Info`              | reply to `GetInfo`                                     |
| `0x82` | `Config`      | `DeviceConfig`      | reply to `GetConfig`                                   |
| `0x90` | `ShotResult`  | `ShotResult`        | reply to `Shot`                                        |
| `0x91` | `ShotRaw`     | `ShotRaw`           | after `ShotResult` when `WANT_RAW` (same `seq`)        |
| `0x92` | `Sample`      | `Sample`            | unsolicited, while a stream runs                       |
| `0x93` | `Temps`       | `Temps`             | unsolicited, once per second while a stream with `TEMPS` runs |
| `0xA0` | `CalShotDone` | `CalShotReport`     | reply to `CalShot`                                     |
| `0xA1` | `CalShotData` | `CalShotReport`     | reply to `CalGetShots`                                 |
| `0xA2` | `CalSolved`   | `CalSolved`         | reply to `CalSolve`                                    |
| `0xA3` | `CalData`     | `Calibration`       | reply to `CalGet`                                      |
| `0xA4` | `CalInfo`     | `CalInfo`           | reply to `CalInfo`                                     |
| `0xF0` | `Log`         | `LogLine`           | unsolicited (reserved; defmt is the primary log path)  |
| `0xF1` | `Event`       | `Event`             | unsolicited                                            |

## 5. Host payloads

### `ShotRequest` (5 bytes)

| offset | size | field          |                                                          |
|-------:|-----:|----------------|----------------------------------------------------------|
|      0 |    1 | `flags`        | bit 0 `WANT_RAW`: also send one `ShotRaw` per sensor; bit 1 `NO_QUIET`: do not assert `QUIET` |
|      1 |    2 | `mag_samples`  | magnetometer samples to average, 1..=4000; 0 = configured default |
|      3 |    2 | `cycle_count`  | RM3100 cycle count, 30..=4000; 0 = configured default    |

A shot runs for roughly `mag_samples / rate(cycle_count)` seconds, where the RM3100 single
measurement rate is about `88 000 / (3 · cycle_count)` Hz (400 → 73 Hz → 1.4 s for 100 samples).
The accelerometers and the IMU are sampled at `DeviceConfig::acc_rate_hz` for the same window.

### `StreamRequest` (4 bytes)

| offset | size | field     |                                                                |
|-------:|-----:|-----------|----------------------------------------------------------------|
|      0 |    1 | `enable`  | 0 stops, non-zero starts or changes the stream                 |
|      1 |    2 | `rate_hz` | ticks per second, clamped to 1..=500 (the effective value is returned in the `Ack` detail) |
|      3 |    1 | `sensors` | bit mask, table below                                          |

| bit | sensor     | sample tag |
|----:|------------|-----------:|
|   0 | `MAG1`     | 0          |
|   1 | `MAG2`     | 1          |
|   2 | `MAG3`     | 2          |
|   3 | `ACC1`     | 3          |
|   4 | `ACC2`     | 4          |
|   5 | `IMU_GYRO` | 5          |
|   6 | `IMU_ACC`  | 6          |
|   7 | `TEMPS`    | sends a `Temps` frame once per second |

At every tick the device polls the selected sensors once and sends one `Sample` per selected
sensor. The magnetometers are polled measurements, so the real rate is bounded by the cycle
count; the ticker catches up but never exceeds the sensor. Samples are **dropped** on a link whose
outbound queue is full rather than stalling the device. When triads are selected the rate is
also clamped to 90 % of the RM3100 measurement rate at the current cycle count (about 66 Hz at
400). A stream survives other requests except `Abort` and `Stream` with `enable = 0` (both emit
`StreamStopped`); a `Shot` runs with the stream paused and the ticker restarts afterwards.

### `DeviceConfig` (56 bytes)

| offset | size | field              |                                                      |
|-------:|-----:|--------------------|------------------------------------------------------|
|      0 |    2 | `mag_cycle_count`  | RM3100 cycle count for shots, 30..=4000              |
|      2 |    2 | `mag_samples`      | samples averaged per shot, 1..=4000                  |
|      4 |    2 | `acc_rate_hz`      | accelerometer / IMU rate during a shot, 10..=2000    |
|      6 |    2 | `quiet_lead_ms`    | delay between asserting `QUIET` and the first sample, ≤ 5000 |
|      8 |    1 | `quiet_enabled`    | bool                                                 |
|      9 |    1 | `led_enabled`      | bool                                                 |
|     10 |    2 | reserved (0)       |                                                      |
|     12 |    4 | `mag_disagree_deg` | `Thresholds`, each an `f32`, finite and ≥ 0          |
|     16 |    4 | `acc_disagree_deg` |                                                      |
|     20 |    4 | `mag_norm_tol`     | relative                                             |
|     24 |    4 | `acc_norm_tol`     | relative                                             |
|     28 |    4 | `dip_tol_deg`      |                                                      |
|     32 |    4 | `motion_dps`       |                                                      |
|     36 |    4 | `mag_noise_rel`    | relative to `|B|`                                    |
|     40 |    4 | `acc_noise_rel`    | relative to g                                        |
|     44 |    4 | `ref_b_total_ut`   | `f32`, external field reference, 0 = none            |
|     48 |    4 | `ref_dip_deg`      | `f32`                                                |
|     52 |    4 | `uart_baud`        | `u32`, 9600..=2 000 000, applied at next boot        |

Defaults: 400, 100, 200, 20, true, true, thresholds 0.5° / 0.3° / 0.03 / 0.02 / 1.0° / 0.5 °/s /
0.004 / 0.01, no reference, 115 200.

`SetConfig` validates, programs the new cycle count into the triads, then persists config and
calibration together in flash.

### `FwBegin` (11 bytes)

| offset | size | field     |                                                           |
|-------:|-----:|-----------|-----------------------------------------------------------|
|      0 |    4 | `size`    | image size in bytes, 1..=size of the active partition (456 KiB) |
|      4 |    4 | `crc32`   | CRC-32/ISO-HDLC of the whole image                        |
|      8 |    3 | `version` | major, minor, patch (informational)                       |

### `FwChunk` (4 + 1..=1024 bytes)

| offset | size | field    |                                                            |
|-------:|-----:|----------|------------------------------------------------------------|
|      0 |    4 | `offset` | byte offset in the image; must equal the bytes received so far and be a multiple of 16 |
|      4 |  `n` | `data`   | 1..=1024 bytes; every chunk but the last should be a multiple of 16 bytes long |

See `OTA.md` for the complete update procedure.

### `Calibration` (16 + 524 × `n_sets` bytes, ≤ 2112)

| offset | size | field             |                                                   |
|-------:|-----:|-------------------|---------------------------------------------------|
|      0 |    1 | `format_version`  | 1                                                 |
|      1 |    1 | `n_sets`          | 0..=4                                             |
|      2 |    2 | `mag_cycle_count` | cycle count the magnetometer models were solved for, ≠ 0 |
|      4 |    4 | `id`              | host-chosen identifier, 0 = none                  |
|      8 |    4 | `date_unix`       | 0 = unknown                                       |
|     12 |    4 | reserved (0)      |                                                   |
|     16 |  524 | `SensorSet` × `n_sets` |                                              |

`SensorSet` (524 bytes):

| offset | size | field          |                                                          |
|-------:|-----:|----------------|----------------------------------------------------------|
|      0 |   12 | `mag_temp_c`   | 3 × `f32`, coil temperature of each triad when solved    |
|     12 |    8 | `acc_temp_c`   | 2 × `f32`, die temperature of each accelerometer         |
|     20 |  288 | `mag`          | 3 × `Affine`                                             |
|    308 |  192 | `acc`          | 2 × `Affine` (`acc[0]` has an upper-triangular matrix)   |
|    500 |    8 | `dip_deg`      | `f64`, fitted dip at the calibration site                |
|    508 |    8 | `field_ut`     | `f64`, estimated field magnitude at the site             |
|    516 |    4 | `residual_deg` | `f32`, RMS angular residual of the fit                   |
|    520 |    2 | `shots`        | `u16`, shots the set was solved from                     |
|    522 |    2 | reserved (0)   |                                                          |

`Affine` (96 bytes): `offset` as 3 × `f64`, then the 3×3 `matrix` row-major as 9 × `f64`;
`calibrated = matrix · (raw − offset)`. Every coefficient and temperature must be finite; the
device rejects a `Calibration` that is not (`BadPayload`).

## 6. Device payloads

### `Info` (37 bytes)

| offset | size | field              |                                                      |
|-------:|-----:|--------------------|------------------------------------------------------|
|      0 |    1 | `protocol`         | 1                                                    |
|      1 |    3 | `firmware`         | major, minor, patch                                  |
|      4 |    1 | `hardware_rev`     | 2 = Rev B                                            |
|      5 |   12 | `uid`              | MCU unique id (96 bits)                              |
|     17 |    4 | `uptime_ms`        |                                                      |
|     21 |    4 | `calibration_id`   | 0 = none                                             |
|     25 |    1 | `calibration_sets` |                                                      |
|     26 |    4 | `calibration_date` | unix, 0 = unknown                                    |
|     30 |    1 | `sensors_ok`       | bit `i` set when sensor `i` (tags 0..=5) passed its identity check at boot |
|     31 |    1 | `time_set`         | bool                                                 |
|     32 |    4 | `unix_time`        | 0 until `SetTime`                                    |
|     36 |    1 | `boot_state`       | 0 confirmed, 1 trial boot of a new image, 2 reverted to the previous image, 3 DFU detach requested, 255 unknown |

### `ShotResult` (82 bytes)

| offset | size | field                   |                                                 |
|-------:|-----:|-------------------------|-------------------------------------------------|
|      0 |    2 | `shot_id`               | counter since boot                              |
|      2 |    2 | `flags`                 | `QualityFlags`, section 7                       |
|      4 |    4 | `unix_time`             | 0 if the clock was never set                    |
|      8 |    4 | `azimuth_deg`           | `f32`, clockwise from magnetic north, [0, 360)  |
|     12 |    4 | `inclination_deg`       | `f32`, positive above the horizon, [−90, 90]    |
|     16 |    4 | `roll_deg`              | `f32`, about the laser axis, right side down positive, (−180, 180] |
|     20 |    4 | `dip_deg`               | `f32`, field angle below horizontal             |
|     24 |    4 | `b_total_ut`            | `f32`                                           |
|     28 |    4 | `g_norm`                | `f32`, calibrated gravity norm (1 = nominal)    |
|     32 |    4 | `mag_spread_deg`        | `f32`, largest angle between triads             |
|     36 |    4 | `acc_spread_deg`        | `f32`, angle between the accelerometers         |
|     40 |    4 | `azimuth_sigma_deg`     | `f32`, 1-σ from sample noise alone              |
|     44 |    4 | `inclination_sigma_deg` | `f32`                                           |
|     48 |    4 | `gyro_peak_dps`         | `f32`, NaN when the IMU is absent               |
|     52 |   12 | `mag_temp_c`            | 3 × `f32`                                       |
|     64 |    8 | `acc_temp_c`            | 2 × `f32`                                       |
|     72 |    4 | `calibration_id`        |                                                 |
|     76 |    2 | `mag_samples`           | samples actually averaged (max over triads)     |
|     78 |    2 | `acc_samples`           |                                                 |
|     80 |    2 | `duration_ms`           | acquisition window                              |

Frame conventions: body frame `X` forward along the laser, `Y` right, `Z` down; world frame NED
with `N` = magnetic north. See `attinav-core/src/survey.rs`.

### `ShotRaw` (41 bytes)

| offset | size | field        |                                                          |
|-------:|-----:|--------------|----------------------------------------------------------|
|      0 |    2 | `shot_id`    |                                                          |
|      2 |    1 | `sensor`     | tag 0..=4                                                |
|      3 |    2 | `count`      | samples averaged (0 = sensor failed)                     |
|      5 |   12 | `mean`       | 3 × `f32`, raw counts                                    |
|     17 |   12 | `std_dev`    | 3 × `f32`, sample standard deviation, raw counts         |
|     29 |   12 | `calibrated` | 3 × `f32`, body frame, unit-ish (gravity points down)    |

### `Sample` (17 bytes)

| offset | size | field    |                                                      |
|-------:|-----:|----------|------------------------------------------------------|
|      0 |    4 | `t_ms`   | milliseconds since boot                              |
|      4 |    1 | `sensor` | tag 0..=6 (section 5, `StreamRequest`)               |
|      5 |   12 | `raw`    | 3 × `i32` raw counts                                 |

### `Temps` (30 bytes)

| offset | size | field        |                                                    |
|-------:|-----:|--------------|----------------------------------------------------|
|      0 |    4 | `t_ms`       |                                                    |
|      4 |   12 | `mag_temp_c` | 3 × `f32`, NaN when the Pt1000 divider reads open or shorted |
|     16 |    8 | `acc_temp_c` | 2 × `f32`, NaN when the accelerometer failed       |
|     24 |    4 | `imu_temp_c` | `f32`, NaN when the IMU failed                     |
|     28 |    2 | `vdda_mv`    | analogue supply measured through VREFINT           |

### `CalShotReport` (86 bytes)

| offset | size | field        |                                                              |
|-------:|-----:|--------------|--------------------------------------------------------------|
|      0 |    1 | `index`      | slot in the device buffer                                    |
|      1 |    1 | `count`      | shots in the buffer                                          |
|      2 |    2 | `flags`      | `QualityFlags` evaluated with the *current* calibration (informational; zero in `CalShotData`) |
|      4 |    1 | `group`      |                                                              |
|      5 |    1 | `roll`       |                                                              |
|      6 |   36 | `mag`        | 3 × (3 × `f32`), mean raw counts per triad                   |
|     42 |   24 | `acc`        | 2 × (3 × `f32`), mean raw counts per accelerometer           |
|     66 |   12 | `mag_temp_c` | 3 × `f32` (NaN when unreadable)                              |
|     78 |    8 | `acc_temp_c` | 2 × `f32`                                                    |

A `CalShot` with the same `(group, roll)` as a buffered one replaces it. The buffer holds 64 shots;
the nominal procedure is 14 groups × 4 rolls.

### `CalSolved` (43 bytes)

| offset | size | field              |                                                       |
|-------:|-----:|--------------------|-------------------------------------------------------|
|      0 |    1 | `error`            | 0 success, else section 8                             |
|      1 |    4 | `iterations`       | `u32`, LM iterations of the primary-pair solve        |
|      5 |    4 | `cost`             | `f32`, final sum of squared residuals                 |
|      9 |    4 | `residual_deg`     | `f32`, RMS laser-direction scatter within groups      |
|     13 |   12 | `mag_residual_deg` | 3 × `f32`, per triad                                  |
|     25 |    8 | `acc_residual_deg` | 2 × `f32`, per accelerometer                          |
|     33 |    2 | `shots`            | `u16`                                                 |
|     35 |    4 | `dip_deg`          | `f32`                                                 |
|     39 |    4 | `field_ut`         | `f32`                                                 |

On success the solved set is held as *pending* until `CalStore` writes it (or `CalBegin` /
another `CalSolve` replaces it). `Abort` during a solve yields `error = 5` (`NotConverged`).

### `CalInfo` (102 bytes)

| offset | size | field             |                                                        |
|-------:|-----:|-------------------|--------------------------------------------------------|
|      0 |    4 | `id`              |                                                        |
|      4 |    4 | `date_unix`       |                                                        |
|      8 |    2 | `mag_cycle_count` |                                                        |
|     10 |    1 | `n_sets`          |                                                        |
|     11 |    1 | `buffered_shots`  |                                                        |
|     12 |    1 | `session_open`    | bool                                                   |
|     13 |    1 | `pending_set`     | bool                                                   |
|     14 |   88 | `sets`            | 4 × `CalSetSummary`, valid up to `n_sets`              |

`CalSetSummary` (22 bytes): `mag_temp_c:f32` (mean over triads), `acc_temp_c:f32`,
`residual_deg:f32`, `shots:u16`, `field_ut:f32`, `dip_deg:f32`.

### `Event` (5 bytes)

| offset | size | field    |                        |
|-------:|-----:|----------|------------------------|
|      0 |    1 | `kind`   | table below            |
|      1 |    4 | `detail` | `u32`, kind-specific   |

| kind | name                 | detail                                               |
|-----:|----------------------|------------------------------------------------------|
|    1 | `Boot`               | `boot_state << 8 | sensors_ok`                        |
|    2 | `CalibrationLoaded`  | calibration id                                       |
|    3 | `CalibrationMissing` | 0                                                    |
|    4 | `ShotStarted`        | shot id                                              |
|    5 | `ShotFinished`       | shot id                                              |
|    6 | `StreamStopped`      | reason (0 = `Abort`)                                 |
|    7 | `SensorFault`        | sensor tag of a triad that failed (poll, data-ready timeout or read) during the last shot or stream window; it is retried at the next window |

`Boot` events are emitted before any host can have opened the USB port, so USB hosts normally see
them only on the UART. `GetInfo` carries the same information.

### `LogLine` (1 + ≤ 96 bytes)

`level:u8` (0 error, 1 warn, 2 info, 3 debug) followed by UTF-8 text.

## 7. Quality flags

`QualityFlags` is a `u16` bit set. A shot is **clean** when no bit is set.

| bit | name                   | meaning                                                             |
|----:|------------------------|---------------------------------------------------------------------|
|   0 | `NO_CALIBRATION`       | nominal gains used; angles are rough                                |
|   1 | `TEMP_EXTRAPOLATED`    | sensor temperature outside the calibrated range (nearest set used)  |
|   2 | `MAG_DISAGREE`         | triads disagree by more than `mag_disagree_deg` (nearby iron); the closest pair was fused |
|   3 | `ACC_DISAGREE`         | accelerometers disagree by more than `acc_disagree_deg`; accelerometer 1 alone was used |
|   4 | `MAG_NORM`             | fused `|B|` off the reference / calibration value by more than `mag_norm_tol` (distant iron, anomaly) |
|   5 | `ACC_NORM`             | `|g|` off nominal by more than `acc_norm_tol` (motion, fault)       |
|   6 | `DIP`                  | dip differs from the reference by more than `dip_tol_deg`           |
|   7 | `MOTION`               | gyro peak above `motion_dps` during the window                      |
|   8 | `MAG_NOISY`            | a triad's per-axis sample standard deviation exceeded `mag_noise_rel · |B|` |
|   9 | `ACC_NOISY`            | same for an accelerometer                                           |
|  10 | `SENSOR_FAULT`         | a sensor delivered no data                                          |
|  11 | `QUIET_SKIPPED`        | `QUIET` was not asserted for the window                             |
|  12 | `SHORT`                | fewer samples than requested (abort or sensor loss)                 |
|  13 | `CYCLE_COUNT_MISMATCH` | shot cycle count ≠ calibration cycle count; nominal rescale applied |

Severe flags (the result should not be recorded as a survey leg): `SENSOR_FAULT`, `MOTION`,
`MAG_DISAGREE`, `ACC_DISAGREE`, `NO_CALIBRATION`.

## 8. Solve error codes

| code | name               | meaning                                                             |
|-----:|--------------------|---------------------------------------------------------------------|
|    0 | —                  | success                                                             |
|    1 | `NotEnoughShots`   | fewer than 24 shots, or fewer than 6 groups holding ≥ 2 shots       |
|    2 | `GroupTooLarge`    | a group holds more than 8 shots                                     |
|    3 | `BadInput`         | a raw vector was zero or not finite                                 |
|    4 | `Singular`         | degenerate geometry (e.g. all shots in one plane)                   |
|    5 | `NotConverged`     | iteration limit reached, or aborted                                 |
|    6 | `ResidualTooLarge` | converged but RMS residual above 0.5°                               |

## 9. Sensor tags

| tag | sensor                                      |
|----:|---------------------------------------------|
|   0 | `Mag1`, RM3100 triad A (front)              |
|   1 | `Mag2`, triad B (middle)                    |
|   2 | `Mag3`, triad C (rear)                      |
|   3 | `Acc1`, SCA3300 #1 (defines the body frame) |
|   4 | `Acc2`, SCA3300 #2 (rotated 90° about Z)    |
|   5 | `Imu`, ICM-42688-P (motion gate only)       |

In `Sample.sensor`, 5 means the IMU gyroscope and 6 the IMU accelerometer.

## 10. Typical sessions

Survey leg:

```
→ SetTime(now)          ← Ack Ok
→ Shot{WANT_RAW}        ← ShotResult, ShotRaw ×5        (≈ 1.5 s at the defaults)
```

Calibration (14 groups × 4 rolls):

```
→ CalBegin              ← Ack Ok
→ CalShot{g, r} ×56     ← CalShotDone each (watch `flags` for MOTION / SENSOR_FAULT and retake)
→ CalSolve              ← CalSolved                      (seconds; Ping/Abort honoured meanwhile)
→ CalStore{id}          ← Ack Ok (detail = set index)
→ CalEnd                ← Ack Ok
```

Repeat at another temperature to add a second set (sets closer than 5 °C replace each other).
