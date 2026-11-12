# Background: emulating the Colour Maximite 2

Notes from before the project started, kept for the reasoning and the sources. Each fact is tagged:
**[verified]** checked in a primary source or by running it, **[reported]** taken from a forum post or
a search summary, **[idea]** a suggestion that has not been checked. What has been measured since is in
[hardware-map.md](hardware-map.md), and what the project is aiming for is in [goals.md](goals.md).

## 1. What exists today

- **No official CMM2 emulator** [reported]. A MiSTer FPGA forum thread says there does not seem to be
  one (https://misterfpga.org/viewtopic.php?p=36334). A ClockworkPi forum reply says Peter Mather wrote
  an MMBasic-for-Windows simulator that approximates the CMM2, graphics included, but it never had a
  proper release (https://forum.clockworkpi.com/t/picocalc-ish-emulation/17004). I have not seen it.
- **MMB4L** [verified from its README and project page]: "MMBasic for Linux" by Thomas Williams
  (`thwill1000`), https://github.com/thwill1000/mmb4l. Release v0.8-alpha.1 (2026-09-24) for Linux x86_64,
  aarch64 and armv6l, with windowed graphics. It takes code from the PicoMite, PicoMite VGA, Colour
  Maximite 2 and MMB4W ports. `OPTION SIMULATE` (or `mmbasic -s CMM2`) imitates several devices: CMM2,
  Game*Mite, MMB4W, PicoCalc, PicoMiteHDMI, PicoMiteVGA, PicoMiteVGAUSB. It is alpha and a high-level
  simulation: it runs the MMBasic interpreter natively and maps the device's commands, so the CMM2 font,
  for example, is a best-effort mapping. Limits from its project page: timing is not real time (`SETTICK
  FAST` is unsupported), program space is 0.5 MB and other RAM 1 MB, there is no GPIO, CSUBs are not
  mentioned, and only some CMM2 commands are simulated (for example `PAGE COPY`, `PAGE SCROLL`, `PAGE
  WRITE`). The tarball carries LICENSE, LICENSE.MIT and LICENSE.MMBasic. I have a copy but have not
  extracted or run it.

## 2. Three levels of emulator

| Level | What it is | Cost | What it buys |
|---|---|---|---|
| A. High-level | Run MMBasic natively on the host and fake the CMM2 commands (MMB4L's approach) | Moderate, and MMB4L already does part of it | Fast BASIC prototyping. Cannot run ARM machine code, so no CSUBs |
| B. ARM-level harness for a CSUB only | Run the cross-compiled Thumb-2 object of a CSUB in a CPU emulator with a fake memory map (state buffer, framebuffer) and a fake CSUB call table | Small (days) | Catches compiler, ABI, alignment, relocation and `.rodata` problems in the exact machine code that will ship, before hardware. No firmware, no hardware timing |
| C. Full system | Emulate the STM32H743 CPU and enough peripherals to run the real firmware `.bin` | Very large | The real MMBasic with its screen on the host: loaders, BLIT, pages, sprites, the file system. Timing never exact |

This project is level C, which is what the Fab Agon Emulator does for the Agon Light 2 (https://github.com/tomm/fab-agon-emulator).
Judging from its folder layout, it has a separate eZ80 CPU core and the VDP firmware built to run on the host.
[idea: worth reading how it is split]

## 3. Facts that matter for level C

All from the CMM2 manuals unless marked.

- CPU: ARM Cortex-M7 at up to 480 MHz, 2 MB flash, 1 MB on-chip RAM [verified]. Gen 2 has 32 MB of
  off-chip RAM [verified, geoffg.net]. The chip's own video controller generates the VGA signal [verified].
- Video: modes 1 to 17, colour depths 8, 12, 16 and (Gen 2) 32 bit, many off-screen pages in off-chip RAM,
  display pages switched at frame blanking, and a frame-blanking interrupt registered by `MODE` [verified in
  the 5.07.01 and 7.00.00b3 manuals]. Whether 32-bit mode has alpha is disputed between two sources.
- Peripherals the firmware uses: SD card, USB host (keyboard, mouse, and in v7 gamepads and USB drives),
  a USB serial console through a CH340 chip on Gen 2, two DAC outputs for audio, I2C, SPI, serial ports,
  GPIO, and a real-time clock with battery-backed RAM [verified].
- Firmware licence: the compiled `.bin` is free software, "you can use or redistribute it as you please"
  (5.07.01 manual, copyright page) [verified]. The source is free to individuals for personal use and
  under a negotiated licence for commercial use (http://mmbasic.com) [verified]. The hardware design
  (schematic, PCB, Gerbers) is in the Construction Pack on https://geoffg.net/maximite.html.
- The firmware used for testing: v7.00.00b5 (2 Nov 2026), `CMM2_MMBasic_7.00.00b5.bin`, 1,179,432 bytes,
  sha256 `6dbbdd03d2dd8a53b6f968d8b1a25a675bc0bce04a68698ca051611b5f806464` [verified].
- CSUBs call back into the firmware through a jump table declared in `ARMCFunctions.h` [reported in forum
  threads], and v7 b5 adds `MM.INFO(CALLTABLE)` [reported in the release notes]. Neither has been read in
  detail. A level B harness would need a fake of this table, and level C gets the real one.

## 4. Ideas

- **Level B** as a stand-alone tool: the game's core is plain integer C that builds for ARM with
  `clang --target=thumbv7em-none-eabi -mcpu=cortex-m7 -ffreestanding`. A CPU-only emulator such as
  Unicorn could run it with a fake memory map. Level C should make this unnecessary. [idea]
- **Existing frameworks before writing CPU or peripheral models:** Renode (Antmicro) lists a number of
  STM32 platforms, and mainline QEMU has several STM32 boards but, as far as I know, no H7 machine. Whether
  either models enough of the STM32H743 still needs checking. [idea]
- **Test oracles from the real board:** `SAVE IMAGE` screenshots (BMP, retrievable with `XMODEM SEND` or the SD
  card), benchmark output over the serial link, and replay logs with state hashes. The game's scripted run ends
  in the same state hash on the board as on a host build (3204661053 and 2556454035), which makes it a good
  end-to-end check: if the emulator reproduces the hash, the ARM code and the firmware are running correctly.
- **Likely hard parts:** page and frame-blanking timing the programs rely on, SDRAM and cache behaviour, any DMA
  used for fast block copies and fills (not confirmed), USB host, and audio mixing. A first target could be
  "boots to the prompt and shows the logo in a window".

## 5. Sources

- MMB4L: https://github.com/thwill1000/mmb4l
- CMM2 manuals: 5.07.01 (https://geoffg.net/Downloads/Maximite/Colour_Maximite_2_User_Manual.pdf) and 7.00.00b5 (shipped with the firmware release)
- CMM2 description and downloads: https://geoffg.net/CMM2_Description.html and https://geoffg.net/maximite.html
- v7 beta thread: https://www.thebackshed.com/forum/ViewTopic.php?FID=16&TID=19182 ; Linux firmware loading: TID=12171
- Fab Agon Emulator: https://github.com/tomm/fab-agon-emulator
