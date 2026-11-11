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

## Firmware and documentation are not included

You supply your own copy of the official firmware. This repository does not contain the firmware
`.bin`, the manuals, the Construction Pack or any MMBasic source code. The firmware is available
from <https://geoffg.net/maximite.html>.

## Credits and disclaimer

The Colour Maximite 2 was designed by Geoff Graham, with firmware by Peter Mather. MMBasic is
Geoff Graham's. This project is independent and is not affiliated with or endorsed by them.

The emulator is built from ST's register descriptions (SVD), the published schematic and
observation of the firmware on real hardware. It does not copy MMBasic source code.

## Licence

GPL-3.0. See [LICENSE](LICENSE).
