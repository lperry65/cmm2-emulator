# cmm2-emulator

An emulator of the **Colour Maximite 2 Generation 2** (CMM2 G2), written in Rust.

The aim is the same as the Fab Agon Emulator for the Agon Light: run the **official, unmodified
firmware** and MMBasic on an emulated CPU and peripherals, so programs behave as they do on the
real machine and can be developed and tested without the hardware.

## Status

Early planning. Nothing runs yet. Progress is tracked in the GitHub issues and milestones.

## Target

- Colour Maximite 2 Generation 2: STM32H743 (Cortex-M7, 480 MHz), 32 MB SDRAM, LTDC video.
- Official MMBasic firmware, currently v7.00.00b5 (`MM.VER` 7.000005).
- CMM2 only. PicoMite emulation is out of scope for now.

## Layout

The repository is a Cargo workspace with two crates:

- `cmm2-core`: the emulation (CPU, memory, peripherals). It has no SDL or UI dependency, so it can
  be tested on its own and reused.
- `cmm2-emulator`: the front end binary. It owns the window and input and drives `cmm2-core`.

They are separate so the SDL front end stays out of the emulation code.

## Building

```
cargo build
cargo test
```

One test checks the vector table of the real firmware, so it is ignored by default. To run it, point
`CMM2_FIRMWARE` at your copy of the `.bin`:

```
CMM2_FIRMWARE=/path/to/CMM2_MMBasic_7.00.00b5.bin cargo test -- --ignored
```

### Trying it

To run the firmware for 100,000 instructions and see which peripheral registers it touches:

```
cargo run --release -- --firmware /path/to/CMM2_MMBasic_7.00.00b5.bin --run 100000 --trace-mmio
```

`--stop-at 0x<address>` stops before the instruction at that address instead, and
`--trace <file>` writes every executed instruction (and, with `--trace-mmio`, the peripheral
accesses after it) to a file. `--trace-from <count>` starts the traces only once that many
instructions have run, so a long run is fast until the part you want to see:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --run 232000000 --trace t.txt --trace-from 230000000
```

`--console` can write the trace to a file in the same way (`--trace-mmio` there needs `--trace`).

To use it interactively, connect the console to your terminal:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console
```

What you type goes to the firmware and what it prints appears as it sends it. Ctrl-] quits. The
first text takes about ten seconds to appear today (about 196 million instructions).

For scripts and CI, `--on <text> <keys>` types the keys once the firmware has printed the text
(`\r`, `\n`, `\t`, `\e`, `\\` and `\xHH` are understood), and `--run` ends the run:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --run 260000000 \
    --on "Keyboard type" "1"
```

To see the screen, add `--window`. It opens an SDL window that shows what the firmware has drawn
and scales it to fit, and closing the window ends the run. It works with or without `--console`
(without it the run goes on until the window is closed, or `--run` or `--stop-at` ends it).

`--console` and `--window` run the machine at its own speed: the core's clock (480 MHz) is held
to the wall clock by sleeping, in steps of at most 10 ms, whenever the emulation is ahead. When
the host is too slow it runs flat out and does not try to catch up later, and a stall (dragging
the window, a suspended process) is forgotten rather than made up with a burst. Add `--unpaced`
to run as fast as the host allows. A plain `--run` is never paced. Today the host is about 4 to
7 times slower than the real machine, so pacing does nothing until the core gets faster.

Building needs the SDL2 development files (`libsdl2-dev` on Debian and Ubuntu).

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
