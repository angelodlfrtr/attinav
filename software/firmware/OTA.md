# Firmware update (OTA over USB or the carrier UART)

The application stages a new image into the second flash bank while running, verifies it, asks
the bootloader to swap it in and resets. The bootloader (`attinav-boot`, built on `embassy-boot`)
performs the swap page by page and **reverts to the previous image** if the new one does not
confirm itself on its first boot. A power loss at any point leaves a bootable device.

## Flash map

STM32U575RG: 1 MiB in two banks of 512 KiB, 8 KiB pages, 16-byte write granularity.

| address       | size    | region             | written by                                   |
|---------------|--------:|--------------------|----------------------------------------------|
| `0x0800_0000` |  32 KiB | bootloader         | SWD only (`cargo boot-flash`)                |
| `0x0800_8000` |   8 KiB | spare              |                                              |
| `0x0800_A000` | 456 KiB | **active** image   | bootloader (swap); SWD for development       |
| `0x0807_C000` |  16 KiB | spare              |                                              |
| `0x0808_0000` | 464 KiB | **DFU** staging    | application (`FwChunk`)                      |
| `0x080F_4000` |   8 KiB | state slot A       | application (config + calibration)           |
| `0x080F_6000` |   8 KiB | state slot B       |                                              |
| `0x080F_8000` |   8 KiB | bootloader state   | bootloader and application (`embassy-boot`)  |
| `0x080F_A000` |  24 KiB | spare              |                                              |

The map lives in `attinav-boot/memory.x` and `attinav-fw/memory.x`; both must agree. The
application reads its own partition sizes from linker symbols, so there is no second copy in Rust
code. The DFU partition is one page larger than the active one, as `embassy-boot` requires for
its swap scratch page. Everything that is ever written at run time, including the bootloader state
page, sits in bank 2, so no erase stalls the bank the code runs from. Configuration and calibration survive an update untouched.

## Producing an image

```sh
cargo fw-bin            # → target/attinav-fw.bin (raw binary, linked for 0x0800_A000)
```

The image is the plain `objcopy -O binary` of the release build. It is **not** position
independent: it only runs from the active partition, which is where the bootloader puts it. Its
CRC-32/ISO-HDLC (the zlib / Ethernet CRC, e.g. `python3 -c 'import zlib,sys;
print(hex(zlib.crc32(open(sys.argv[1],"rb").read())))' target/attinav-fw.bin`) goes into
`FwBegin`.

## Host procedure

1. `GetInfo`. Refuse to proceed when `boot_state == 1` (a trial image is still unconfirmed) or
   when a shot or stream is running (`Abort` first).
2. `FwBegin { size, crc32, version }` → expect `Ack Ok`. `InvalidArgument` means the size is zero
   or exceeds the active partition.
3. Send the image in order as `FwChunk { offset, data }`, `offset` starting at 0 and advancing by
   the chunk length, 1024 bytes per chunk (the maximum; every chunk except the last must be a
   multiple of 16 bytes). Wait for each `Ack` before sending the next chunk; `detail` is the number
   of KiB received so far, suitable for a progress bar. The device erases DFU pages lazily as the
   chunks arrive, so the first chunk of each 8 KiB page takes a few milliseconds longer.
   Any `Ack` other than `Ok` aborts the session on the device; start again from step 2.
4. `FwFinish` → the device reads the staged image back, checks its CRC-32 against `FwBegin`,
   marks it for swap and answers `Ack Ok`, then resets 200 ms later. `BadImage` means the CRC did
   not match or the byte count did not equal `size`; `FlashError` means the bootloader state page
   could not be written.
5. Wait for the device to come back (USB re-enumerates; expect 1–3 s for the swap of a 140 KiB
   image). `GetInfo` must now report the new `firmware` version and `boot_state == 0`: the
   application calls `mark_booted` as soon as its sensors are up and the request loop is reached,
   so a successfully enumerating device has already confirmed itself.
   `boot_state == 2` means the new image failed to confirm (hard fault, watchdog) and the
   bootloader reverted to the previous one.

`FwAbort` at any time before `FwFinish` discards the session; nothing persistent has changed.

Timing: a 140 KiB image is ~140 chunks; at USB full speed the transfer is dominated by flash
programming (~1.5 ms per KiB) and takes well under a second. On the UART at 115 200 baud it takes
about 15 s.

## What protects the device

* The active image is never touched until `FwFinish` succeeded; a failed or interrupted transfer
  leaves the running firmware intact.
* The CRC is checked on the bytes **read back from flash**, not on the stream, so a flash write
  error is caught too. A flash ECC fault during the read-back also fails the check.
* `embassy-boot` swaps with a progress log in the state page; a reset mid-swap resumes it.
* The new image must call `mark_booted` once (the firmware does so after sensor bring-up);
  otherwise the next reset reverts. The independent watchdog, started by the bootloader at 32 s
  and re-armed by the application at 16 s, turns a hung image into that reset even when it
  faults before reaching its own initialisation.
* The bootloader is not updatable over the air. It is 8.5 KiB, has no sensor or protocol code and
  should never need to change.

## Development flashing (SWD)

```sh
cargo boot-flash            # bootloader, once per board (or when memory.x changes)
cargo run --release         # application through probe-rs, with defmt logs
```

`probe-rs` writes the application straight into the active partition. The bootloader state page
is left as it was; if a previous OTA left it in "swap" state, the first boot of the SWD-flashed
image is treated as the trial boot and confirms itself as usual.
