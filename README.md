# cmm2-emulator

An emulator of the **Colour Maximite 2 Generation 2** (CMM2 G2), written in Rust.

It runs the **official, unmodified firmware** and MMBasic on an emulated STM32H743 (Cortex-M7)
and its peripherals, so programs behave as they do on the real machine and can be developed and
tested without the hardware. The aim is the same as the Fab Agon Emulator for the Agon Light.

## Goal

This is a development emulator for CMM2 software. The first target is my game, Galactic
Raiders. The scope is whatever the hardware supports. See [docs/goals.md](docs/goals.md).

## Status

It boots the unmodified 7.00.00b5 firmware through its first-boot setup and a system reset to the
`>` prompt and runs BASIC there. 7.00.00RC6 boots with its own settings layout. What works today:

- The Cortex-M7, with the ARMv7E-M DSP instructions, and the peripherals the firmware waits on:
  clocks, the RTC, flash, GPIO, USART1 as the console, the timers, the RNG and the LTDC.
- `TIMER` and `PAUSE`, and a modelled RNG, so `RND` differs from run to run as on the board.
- Video in every mode and colour depth, with layers, blending and page flips, in an SDL window or
  as a PPM screenshot.
- A USB keyboard in the window, through the firmware's own USB host stack, and the console over a
  terminal.
- An SD card on a disk image, in the firmware's SPI mode.
- A DS3231 clock chip, and a board that remembers its firmware, settings, clock and card between
  runs.
- Galactic Raiders loads and draws all its sprites and colours (my report).

What does not work yet, or is incomplete:

- Speed: the host is too slow for the machine's clock, so Galactic Raiders is far too slow to
  play. The plan is an x86-64 JIT ([issue 74](https://github.com/lperry65/cmm2-emulator/issues/74)).
- The floating-point instructions of the DSP work still to do
  ([issue 77](https://github.com/lperry65/cmm2-emulator/issues/77)).
- Memory wait states ([issue 65](https://github.com/lperry65/cmm2-emulator/issues/65)).
- Other peripherals: the USB mouse and gamepad, SPI3 for the SD card, and sound (the DAC) are
  not done.
- Firmware versions other than 7.00.00b5 and 7.00.00RC6 have no settings layout yet
  ([issue 82](https://github.com/lperry65/cmm2-emulator/issues/82)).
- Windows is planned but not built
  ([issue 75](https://github.com/lperry65/cmm2-emulator/issues/75)).

The detail, with the CPU history, the speed figures and the full list of open issues, is in
[docs/status.md](docs/status.md). Progress is tracked in the
[GitHub issues](https://github.com/lperry65/cmm2-emulator/issues).

## Building

```
cargo build
cargo test
```

Building needs the SDL2 development files (`libsdl2-dev` on Debian and Ubuntu). I have built and
run it on Linux only so far. A release build (`--release`) is a good deal faster. The tests that
run the real firmware are ignored by default and need your copy of the `.bin`; see
[docs/usage.md](docs/usage.md#running-the-tests).

## Quick start

You need your own copy of the official firmware `.bin` (see the notice below). A real board is
burned once and then keeps its firmware, settings and card, and the emulator does the same:

From a checkout, `cargo build --release` makes `target/release/cmm2-emulator`, or
`cargo install --path crates/cmm2-emulator` installs it. The commands below say `cmm2-emulator`.

```
cmm2-emulator --burn CMM2_MMBasic_7.00.00b5.bin    # burn it, once, and start the board
cmm2-emulator                                      # from then on: as it was left
```

- The window opens by default and is the screen and a USB keyboard. Closing it ends the run.
  The first-boot questions come up there the first time. A plain start needs a display: with none
  it is refused, and the message says to use `--console`.
- `--console` uses the terminal instead (Ctrl-] quits); `--window` as well gives both.
- `--sd card.img` puts an SD card in the socket and the board remembers it; `--no-sd` takes it
  out. The image has to exist first, with a FAT file system on it (the emulator never makes a
  card): see the `truncate` and `mkfs.vfat` example in
  [docs/usage.md](docs/usage.md#the-sd-card---sd).
- The board is a directory: `cmm2-emulator` in `$XDG_DATA_HOME` (a relative one is ignored), or
  `~/.local/share/cmm2-emulator` when that is not set. It holds `firmware.bin`, `settings.flash`
  (the stored program, options and clock), `sd.path` and `board.lock`. `CMM2_EMULATOR_HOME` names
  another directory, which is another board.
- `--firmware <file>` runs an image as a scratch machine, with no board directory. On its own it
  only prints the vector table. To run it add `--console`, `--window`, `--run <count>` or
  `--stop-at <address>`; `--on <text> <keys>` needs `--console`. For scripts and CI:

  ```
  cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --run 260000000 \
      --on "Keyboard type" "1"
  ```

Every option, with the rules for the board and for the files the emulator saves, is in
[docs/usage.md](docs/usage.md).

## Documentation

- [docs/README.md](docs/README.md): the index of the documentation.
- [docs/usage.md](docs/usage.md): all the options, the board, the settings file, the SD card.
- [docs/status.md](docs/status.md): the detailed status, the CPU core, speed and known gaps.
- [docs/goals.md](docs/goals.md), [docs/hardware-map.md](docs/hardware-map.md) and the notes on
  the boot, the SD card, the clock chip, the RNG, the timers, video and USB, all listed in the
  index.

## Firmware and documentation are not included

You supply your own copy of the official firmware. This repository does not contain the firmware
`.bin`, the manuals, the Construction Pack or any MMBasic source code. The firmware is available
from <https://geoffg.net/maximite.html>.

## Credits and disclaimer

The Colour Maximite 2 was designed by Geoff Graham, with firmware by Peter Mather. MMBasic is
Geoff Graham's. This project is independent and is not affiliated with or endorsed by them.

The emulator is built from ST's register descriptions (SVD), the published schematic and
observation of the firmware on real hardware. It does not copy MMBasic source code.

The register description in `crates/cmm2-core/data/STM32H743.svd` is © STMicroelectronics, used
unmodified under the Apache License 2.0.

## Licence

GPL-3.0. See [LICENSE](LICENSE).
