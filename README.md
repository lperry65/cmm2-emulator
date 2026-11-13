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

The window is also the keyboard, and it reaches the firmware in one of two ways. With `--window`
it is a USB keyboard, as on the real board: the window plugs a HID boot keyboard into the USB
port, and sets its report from the keys that are down (SDL's scancodes are USB usages, so a key
goes in as itself, modifiers in the first byte, up to six keys in the order they were pressed, and
the rollover error in all six slots if more are held). SDL's key repeat is ignored, as the firmware
does its own repeat. A key that goes down and up between two polls is held in the report until the
keyboard has sent it, and when the window loses the focus every key is let go. So `KEYDOWN()` and
`INKEY$` see held keys, and Ctrl-C stops a program as it does on the board.

The firmware's keyboard layout (`OPTION USBKEYBOARD`, which the first boot sets to UK) now decides
the characters, as it does on the board, so the layout of the host no longer matters for USB. A
US host keyboard with a UK firmware layout gives UK characters for some keys (Shift with 2 types
`"` and Shift with ' types `@`), which is what the real board would do with that keyboard.

The firmware starts polling the keyboard about 300 million instructions after the prompt: 1.4 s of
the machine's time, and about 6 s of host time in a release build (measured, the tests in
`usb_keys.rs`). Until then the keys are the console's: the window types bytes into the UART, which
is what answers the first-boot keyboard question and anything typed before USB is up. On that
path printable keys come as SDL text input, so the host layout and Shift apply, and the other keys
are sent as the bytes the firmware's console reads from a terminal. Enter is CR, Backspace is
0x08 (the firmware ignores 0x7F), and Tab and Escape are themselves. The arrows are `ESC [ A` to `D`, Home and End are `ESC [ 1 ~` and `ESC [ 4 ~` (measured: the
firmware does not read xterm's `ESC [ H` and `ESC [ F`), Insert, Delete, Page Up and Page Down are
`ESC [ 2 ~`, `3~`, `5~` and `6~`, F1 to F4 are `ESC O P` to `S`, and F5 to F12 are `ESC [ 15 ~`,
`17~`, `18~`, `19~`, `20~`, `21~`, `23~` and `24~`. Ctrl with a letter sends the control code,
so Ctrl-C stops a running program. Held keys repeat as SDL repeats them. Text other than ASCII
is dropped, as the console has no way to take it.

Once the firmware polls the keyboard the window sends USB reports only, and no console bytes, or
every key would be typed twice. A key that is held across that moment was typed on the console, so
it is typed once, and it is not in the report: it counts again from its next press. Modifiers are
always in the report, so Shift held across the switch still shifts the next key. A system reset
takes the window back to the console until the firmware polls again.

`--console-keys` keeps the old behaviour with `--window`: the keys are the console's all the while
and no USB keyboard is plugged in, which is useful if USB misbehaves. The host terminal
(`--console`) always feeds the UART, in both modes. With `--window` and no `--console` the
window alone gives input; with both, the terminal and the window feed the same console.

`--console` and `--window` run the machine at its own speed: the core's clock (480 MHz) is held
to the wall clock by sleeping, in steps of at most 10 ms, whenever the emulation is ahead. When
the host is too slow it runs flat out and does not try to catch up later, and a stall (dragging
the window, a suspended process) is forgotten rather than made up with a burst. Add `--unpaced`
to run as fast as the host allows. A plain `--run` is never paced. Today the host is about 4 to
7 times slower than the real machine, so pacing does nothing until the core gets faster.

`--flash <file>` keeps the board's settings between runs, as a real board does. If the file
exists it is loaded at the start, and the flash and the clock are written back to it when the run
ends (quit key, window closed, `--run` limit, `--stop-at` or a fault), through a temporary file
and a rename, so a crash never leaves half a file. The first run, with no file yet, goes through
the first-boot setup and saves the options it makes. A second run with the same file reaches the
prompt with no setup questions and no reset:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --flash board.flash
```

The file holds only what the user changes, never the firmware, so it stays valid when you move
to a newer firmware `.bin`: the stored program and saved options (flash from 0x08120000 to the
end, 896 KB) and then the clock's time and date (8 bytes), which a real board's battery keeps. The
firmware asks its setup questions again if the clock has not been set, so the clock has to be
saved too. The time stands still while the emulator is off. The file must be exactly 917,512
bytes, or the run stops with an error and the file is left alone.

`--sd <file>` puts an SD card in the socket, with the file as its contents. The firmware drives the
card in SPI mode over GPIO pins, and the card is a model of that: it answers the commands the
firmware sends (initialisation, single-block read and write) from the image. The file has to exist
and has to be a whole number of 512-byte blocks, or the run stops with an error and the file is
left alone; the emulator never makes a card out of nothing. When the run ends, by any of the ways
above, the card is written back to the file, through a temporary file and a rename as `--flash`
is. Without `--sd` the socket is empty and the firmware says "SD card not present".

The image needs a FAT file system on it, which I make with the usual tools and not with the
emulator. For a 64 MB FAT32 card:

```
truncate -s 64M card.img
mkfs.vfat -F 32 -n CMM2 card.img
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --flash board.flash --sd card.img
```

`7z l card.img` lists what is on it after a run. To put a file on it before a run, mtools'
`mcopy -i card.img file.bas ::` should do it, though I have not tried that here. The firmware sets the card up the first time it is used, which costs
about 5 million instructions; after that it is quick. The CMM2 has no `SAVE` or `LOAD` for
programs: `FILES`, `RUN "T.BAS"`, `LIST "T.BAS"`, `EDIT "T.BAS"` and `AUTOSAVE "T.BAS"` use the
card. `OPTION SD TIMING FAST` (the SPI3 peripheral) is not modelled yet, and nor are the multi-block
commands, which the firmware does not use for anything I have tried.

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
