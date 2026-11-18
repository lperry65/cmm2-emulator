# cmm2-emulator

An emulator of the **Colour Maximite 2 Generation 2** (CMM2 G2), written in Rust.

The aim is the same as the Fab Agon Emulator for the Agon Light: run the **official, unmodified
firmware** and MMBasic on an emulated CPU and peripherals, so programs behave as they do on the
real machine and can be developed and tested without the hardware.

## Status

As of commit 8260358 (2026-11-14) the emulator boots the unmodified 7.00.00b5 firmware through its
first-boot setup and a system reset to the `>` prompt and runs BASIC there. What works:

- The Cortex-M7 (a vendored and changed copy of the zmu core, see "The CPU core" below) and the
  peripherals the firmware waits on, checked against the real firmware: clocks and power, the RTC,
  the flash controller (erase and program, so the options and a stored program persist), GPIO,
  USART1 as the console, the timers TIM5 and TIM16, the RNG, and the LTDC.
- `TIMER` and `PAUSE` count properly: TIM5 and TIM16 raise their update interrupts (issues 28 and
  70, see [docs/timers.md](docs/timers.md)). The timer clock is a fixed 240 MHz and does not follow
  the RCC (issue 76).
- The RNG is modelled ([docs/rng.md](docs/rng.md)), so `RND` gives different numbers from run to
  run, as on the board, and a fresh boot no longer prints a false `Error:1` (issues 22 and 46).
- Video in every mode and colour depth, with layers, blending and page flips, in an SDL window
  (`--window`) or as a PPM file (`--screenshot`).
- A USB keyboard, driven from the window through the firmware's real USB host stack, so
  `KEYDOWN()`, `INKEY$` and Ctrl-C work as on the board. The console over a terminal also works.
- An SD card on a disk image (`--sd`), in the firmware's default SPI mode.
- A board that remembers itself between runs: its firmware, its settings, the DS3231 clock chip
  and the SD card in its socket (see "The board"; `--flash` and `--sd` still name files of their
  own).
- Real-time pacing at the machine's 480 MHz clock (built; it has no effect while the host is
  slower than the machine).

Galactic Raiders, the game I am writing for the CMM2, loads and draws all its sprites and colours
(my report). In a reviewer's run it ran for about 95 s of host time and then stopped at an ARMv7E-M
DSP instruction that the core does not implement, at 0x08146b6c (issue 77). My own first run
returned straight to the shell, and I do not know that it ended at that fault. In the reviewer's run
the game printed `startup_load_ms=763.0`; I do not know what that covers, so I make no comparison
with the board. It is far too slow to play yet (speed, issue 74).

### The CPU core

The core is my changed copy of zmu, and the changes are listed in `crates/zmu_cortex_m/NOTICE.md`,
which is the detailed record; the decision to use it is
[docs/decisions/0001-cpu-engine.md](docs/decisions/0001-cpu-engine.md). Since the first boot it has been checked against the
ARMv7-M manual: exceptions that arise while another is entered are derived exceptions as the manual
has them, an exception entry counts 12 cycles and a tail-chained entry 6 (the 6 is ARM's figure for
the Cortex-M3 and M4, which I have not verified for the M7), a lockup is a state of its own with the
priorities the manual gives, a reset loads the vector table from a boot address, and SMULL, which
multiplied unsigned, is fixed (issue 64). Alignment UsageFaults are modelled (8260358, issues 68 and
67, closed): the instructions that always fault on an unaligned address do so, LDR, STR, LDRH,
LDRSH, STRH and TBH only with `CCR.UNALIGN_TRP` set, the fault is precise, and `CFSR.UNALIGNED` is
recorded, with escalation to a HardFault. Not modelled is the Device and Strongly-ordered memory
types, as the core has none. The same commit stops the DWT cycle counter in sleep unless
`DBGSLEEP_D1` is set (the emulator does not sleep on WFI yet, issue 72) and fixes two reset-related
bugs. What it still lacks is the rest of the ARMv7E-M DSP instructions (issue 77: the game stops at
0x08146b6c, which the reviewer reads as UQADD8).

### Speed

Speed is the main thing that is not done. These are the figures, and which of them I measured:

- Measured: the core runs about 54.5 million instructions a second at the prompt (2026-11-13,
  release build; `docs/usb-host.md` section 12), and about 24 to 25 million in the BASIC interpreter
  (`FOR i=1 TO 100000:a=a+1:NEXT`, a quiet machine; the working is in my local speed notes, which
  are not in the repository).
- Estimated, not measured: the board runs about 220 million instructions a second (zmu's cycle
  estimate, as in `docs/checkpoint-iteration-3.md`). By that estimate the host is about four times
  too slow. That "about four times" is an estimate from instruction counts.
- Measured on a program: an empty `FOR` loop of 400,000 iterations takes 255.7 ms on the emulated
  machine's clock (read with `TIMER`, on 2026-11-14), and the board takes 44.944 ms for 100,000 (my
  measurement on the board, issue 70). So the emulated machine takes about 1.4 times as long as the
  board on that loop. This compares the machine's clock with the board's, not the host's speed.

I decided on 2026-11-14 to build an x86-64 JIT directly, with no interim steps, because the interim
steps (an estimate of about 2.7 to 2.8 times together; two of the five were measured on prototypes;
the working is in my local speed notes) cannot reach the board's speed. See
[docs/decisions/0002-jit-first.md](docs/decisions/0002-jit-first.md) (issue 74).

### Windows

The project is cross-platform Rust. I have built and run it on Linux only so far (CI runs on
Ubuntu). Windows is planned (issue 75): several parts of the front end are Unix-only now (the signal
handling, the terminal's raw mode, file locks and `/proc`), and the SDL2 build and a Windows CI job
are not done. The JIT design has to weigh portability for the same reason.

### Known gaps

The open issues at 8260358, from `gh issue list --state open`:

- Speed and timing: 74 (the JIT), 15 (the speed of the machine's bus path), 16 (zmu fidelity gaps
  found while integrating), 65 (memory wait states), 72 (WFI never sleeps), 76 (timer clocks do
  not follow the RCC).
- The CPU core: 77 (DSP instructions), 59 (zmu follow-ups after dec44de, to check against the
  ARMv7-M manual), 17 (readable disassembly in the trace).
- Console and input: 66 (console input arrives faster than the baud rate), 48 (pasted lines are
  joined), 47 (a hard fault on `ESC O D`, the SS3 left arrow), 71 (the emulator exits silently on
  `RUN` with the console on a terminal; every end now says why, and what is left is that the line
  cannot know where an escape sequence moves the cursor, and that the window-closed end has no
  end-to-end test), 62 (keyboard follow-ups), 53 (USB mouse), 54 (gamepad).
- Storage and the board's state: 63 (SPI3 for the SD card, `OPTION SD TIMING FAST`), 44 (a host
  folder as the SD card), 41 (the guest writes 32 zero bytes into the firmware region of flash), 69
  (remember the board between runs: built, see "The board"; the issue is not closed yet).
- Window and platform: 39 (scaling, full screen and aspect controls), 75 (Windows).
- Firmware and end-to-end: 57 (firmware 5.07, the version the Gen 2 shipped with), 49 (run
  Galactic Raiders as an end-to-end test), 25 (boot to the MMBasic idle loop: the acceptance is only
  partly met. No test asserts the idle loop; the tests assert the prompt and commands, in
  `crates/cmm2-core/tests/console.rs`. `GPIOH_IDR`, `GPIOD_IDR` and `TIM5_CNT` appear in
  docs/boot-trace.md only in its iteration-1 sections, as an expectation from an earlier spike that
  was never confirmed. A speed note exists, in my local speed notes, which are not in the
  repository).
- Not in an issue: sound (the DAC) and a GDB option (the vendored core has a GDB server, but the
  front end does not start it).

Progress is tracked in the GitHub issues and milestones.

## Target

- Colour Maximite 2 Generation 2: STM32H743 (Cortex-M7, 480 MHz), 32 MB SDRAM, LTDC video.
- Official MMBasic firmware, currently v7.00.00b5 (`MM.VER` 7.000005).
- CMM2 only. PicoMite emulation is out of scope for now.

## Layout

The repository is a Cargo workspace with three members, two of them mine:

- `cmm2-core`: the emulation (CPU, memory, peripherals). It has no SDL or UI dependency, so it can
  be tested on its own and reused.
- `cmm2-emulator`: the front end binary. It owns the window and input and drives `cmm2-core`.
- `zmu_cortex_m`: the Cortex-M core, a vendored copy of the one in
  [zmu](https://github.com/jjkt/zmu) with my changes to it (listed in
  `crates/zmu_cortex_m/NOTICE.md`). It is Apache-2.0, as upstream; `cmm2-core` depends on it.

`cmm2-core` and `cmm2-emulator` are separate so the SDL front end stays out of the emulation code.

In `cmm2-emulator`, `main.rs` is a short entry point over modules: `args` (the command line),
`batch` (a run with no console or window), `live` (a run with the console or a window), `machine`
(building the machine), `board` (the board's directory, its lock and what it remembers) and
`launch` (which run is on the board), `storage`, `saveable`, `atomic_write` and `run_and_save` (the
`--flash` and `--sd` files, read at the start and saved safely at the end), `panic_hook`,
`signals`, `output` and `error`, `trace`, `slice`, `report` and `inspect`, `script` (the `--on`
keys), `pacing`, `terminal`, `keys`, `usb_keys`, `window` and `local_clock`. The unit tests of a
module are in `<module>/tests.rs` beside it, or in the file itself for some of the smaller ones,
and the command-line and real-firmware tests are in `crates/*/tests/`, split by topic.

## Building

```
cargo build
cargo test
```

Building needs the SDL2 development files (`libsdl2-dev` on Debian and Ubuntu). I have built and run
it on Linux only so far; Windows is planned (issue 75).

Nearly all the ignored tests run the real firmware: they need your copy of the `.bin` and take a
while. `cargo test --workspace -- --ignored --list` counts 69 of them (counted on 2026-11-18 in the
working tree after the board was added, on top of 4bd964d; listing needs no firmware): 44 in
`cmm2-core` (43 in the nine files of `crates/cmm2-core/tests/`, and a display speed test in
`display/tests.rs`), 24 in `cmm2-emulator` (3 in `tests/real_firmware.rs`, 1 in
`tests/board_burn.rs` and 20 in the tests of `keys.rs` and `usb_keys.rs`), and one in
`zmu_cortex_m`. Two of the 69 do not use the firmware: the display speed test, which times the
compositing, and the `zmu_cortex_m` one, a decode dump run by hand with `ZMU_DUMP_IMAGE` and
`ZMU_DUMP_OUT`. To run the others, point `CMM2_FIRMWARE` at the `.bin`; a release build is a good
deal faster:

```
CMM2_FIRMWARE=/path/to/CMM2_MMBasic_7.00.00b5.bin cargo test --workspace --release -- --ignored
```

### The board

A real board is burned once and then keeps its firmware, its settings and the card in its socket
until you change them. I wanted the emulator to be the same, so a plain `cmm2-emulator` switches
the board on as it was last left, with the window (the screen and the USB keyboard), as a board
with a monitor and a keyboard does. `--console` replaces the window with the terminal console
(`--window` as well gives both), and `--run`, `--stop-at`, `--on`, `--unpaced`, `--trace`,
`--screenshot`, `--rng-seed` and the rest apply to the board run.

The board is a directory, and the directory is the board. It is `cmm2-emulator` in `$XDG_DATA_HOME`
(a relative `XDG_DATA_HOME` is ignored, as the XDG specification has it), or
`~/.local/share/cmm2-emulator` when that is not set. The environment variable `CMM2_EMULATOR_HOME`
names the directory itself instead, which is what the tests use so that they never touch mine, and
a different one is a different board. It holds `firmware.bin` (my copy of the image),
`settings.flash` (the `--flash` file below: the stored program, the saved options and the clock),
`sd.path` (the card in the socket) and `board.lock`.

```
cmm2-emulator --burn CMM2_MMBasic_7.00.00b5.bin    # burn it, once, and start the board
cmm2-emulator                                      # from then on: as it was left
```

`--burn <file>` burns the firmware. The image is loaded before anything is made, and the loader
checks it as it always did: an image that reaches 0x08120000 is refused, and so is one whose vector
table is not sensible (the stack pointer in RAM, a Thumb reset handler inside the image). What a
burn cannot know is whether the image is whole: a firmware image has no checksum to ask, so a copy
that was cut short, or damaged, and still has a sensible vector table is burned. I considered
checking that the handlers in the vector table point inside the image, and took it out: the real
image has only four distinct handlers, so it would decide for hardly any cut. A burn says what it
burned on one line of standard error (the size, the initial stack pointer and the reset handler),
so the size can be compared with the file I meant. The image is then written into the board with a
temporary file in the same directory, synced and renamed into place, so a burn that fails (a full
disk, say) leaves the old firmware where it was, and moving or changing the `.bin` afterwards does
nothing until I burn it again. If `firmware.bin` is a symbolic link, the rename replaces the link
itself and what it pointed to is not written; the cleanup of old temporary files looks only in the
board directory. Burning keeps the settings: the file holds only what the user changes and never
the firmware (as the `--flash` section below has it), so I expect it to survive a new firmware, but
I have only tested that a burn leaves it alone, not that another firmware version accepts it. The
board then starts: the first run, with nothing in the directory yet, makes the directory (mode
0700, as the XDG specification has it), burns and starts, and the first-boot questions come up in
the window (or the console) as before. `--burn` goes with `--console`, `--run`, `--sd` and the
other board options. The burn is done before the machine is built, so a start that then fails (a
card that has gone, see below) has burned all the same. A plain start with nothing burned prints
how to burn and which directory it looked in, exits with status 1 and makes nothing, not even the
lock file.

`--firmware <file>` is not the board. It runs that image as a scratch machine, as before the board
existed: the file is read, nothing is read from or written to the board directory, and `--flash`
and `--sd` are its only files. Every command line in the sections below that names `--firmware` is
such a run, so CI and scripts have no hidden state. `--firmware <file>` alone prints the vector
table. `--burn` together with `--firmware` is a usage error (status 2), and so is a second `--burn`
or a second `--firmware`. `--no-sd` is for the board and is refused with `--firmware`. Options may
come in any order, and `--firmware` may now come anywhere on the line (it had to come first, so
`--run 10 --firmware f` was an error and is a scratch run now); `--version` and `--lookup` still
have to come first.

A board can only be switched on once. A run on the board takes a lock (flock) on `board.lock` for
as long as it goes, and a second one, burning included, says that the board is already switched on
and exits with status 1. Only the lock is looked at, not the other process. The lock file is opened
to write (NFS wants that for an exclusive lock, flock being emulated with byte-range locks there),
and only if that is refused with a permission error (EACCES, or EPERM for an immutable file,
`chattr +i`), or because the file system is read-only, is a lock file that is there opened to read.
A plain start that races a first burn may say that nothing is burned, as that is looked at before
the lock; the next one says the board is switched on. A file system that cannot lock refuses the
board with a message that says which way the file was opened. Only `--burn` makes the directory; a
plain start never does. A board directory that cannot be written switches on if `board.lock` is
already in it (a directory with no lock file in it cannot be switched on: the error is `board.lock:
Permission denied`), but the run cannot be saved in it: the board's own settings file is in the
directory, so a plain `--run 10` exits with status 1 and says that its directory is not writable,
and only a run with `--flash` to a file elsewhere goes. What is in the directory is written as the
`--flash` and `--sd` files are. An empty `CMM2_EMULATOR_HOME` is an error and not the same as
unset, so that a script with an unset variable does not land on the usual board.

`--flash <file>` without `--firmware` means this settings file instead of the board's own, for a
separate set of settings; the board's firmware is still used, so that run still takes the board's
lock. The card in the socket is remembered by `--sd <file>` without `--firmware`: its absolute path
goes in `sd.path`, once the file has been read as a card, and the next start puts it in again. The
file stays where it is (it is not copied), and a new `--sd` replaces it. A name that ends in a
blank is kept as it is; one that ends in a line end is refused, as the record could not keep it. If
the file has gone, or cannot be used, the start fails with an error that says so and tells me to
give another with `--sd <file>` or take the card out with `--no-sd`, which forgets it and leaves
the socket empty. If I write `sd.path` by hand, any line ends after the path are fine; anything
that is not an absolute path in UTF-8, or a directory in its place, gets the same advice, and
`--sd` and `--no-sd` replace or remove such a record (a directory only if it is empty: I do not
delete what is in one).

The window is the default when there is none of `--console`, `--run` and `--stop-at`, so there is
no `--no-window`: `--console` on its own (or `--run`, `--stop-at`) runs the board without one. A
window that cannot be opened is an error, and it says how to go without one: to use `--console`
when the window is the default of a plain start, and to drop `--window` when I typed it
(`--console` would keep that window). When it is the default window of a plain start, with
`SDL_VIDEODRIVER` not set, and SDL has fallen back to its KMSDRM or offscreen driver (which it does
with no display), the start is refused: KMSDRM would take over the physical screen and offscreen
draws nowhere, so the run would seem to do nothing. A `--window` that I typed, an `SDL_VIDEODRIVER`
that I set, and every `--firmware` run are not refused and are as they were before the board, so a
headless script with `--window` and `--screenshot` still works. I tested the refusal on offscreen
with no `DISPLAY`, `WAYLAND_DISPLAY` or `XDG_RUNTIME_DIR`; the KMSDRM row is covered by a unit test
of the rule only, as SDL fails by itself on a machine with no DRM device, and I have not tried a
real console on KMSDRM. The board's directory name has to be UTF-8, and on Windows (issue 75, not
built) `%LOCALAPPDATA%` is where I mean it to live.

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

`--screenshot <file>` writes the screen as a PPM file when the run ends, in any mode. If the
firmware has not turned the LTDC on by then it says `no screenshot: the LTDC is not enabled` and
writes none. `--firmware <file>` alone prints the initial stack pointer and the reset handler,
`--lookup 0x<address>` names the peripheral register at an address, and `--version` prints the
version.

To use a firmware file interactively, as a scratch machine, connect the console to your terminal
(the board has its own way, above):

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console
```

What you type goes to the firmware and what it prints appears as it sends it. Ctrl-] quits. The
first text appears after about 196 million instructions: a few seconds in a release build.

For scripts and CI, `--on <text> <keys>` types the keys once the firmware has printed the text
(`\r`, `\n`, `\t`, `\e`, `\\` and `\xHH` are understood), and `--run` ends the run:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --run 260000000 \
    --on "Keyboard type" "1"
```

Typed input has two limits to know about. First, MMBasic's console makes an LF after a CR only
once nothing has arrived for 3 ms, so keys pushed in one go reach it as bare CRs. A program sent
that way is stored with bare CRs by `AUTOSAVE` and then fails to load, and the real board does the
same with the same bytes (issue 73, closed: not an emulator bug). So end each line of scripted
input for `--console` and `--on` with CRLF (`\r\n`), or wait for the echo before the next line. I
tried a three-line program through `AUTOSAVE` in one `--on` burst: with CRLF it ran, with bare CRs
`RUN` said `Error in line 1: Expression syntax`. Second, the bytes reach the firmware as fast as it
reads them, not at the serial baud rate (issue 66, open), so a paste of more than 255 bytes
overflows MMBasic's 256-byte input buffer and part of it is lost. I have not tested that overflow
myself; the figure is from the issue.

MMBasic's `RND` reads the hardware RNG, so `RANDOMIZE` does not make a run repeat.
`--rng-seed <number>` seeds the RNG model, so that `RND` gives the same numbers in every run. The
number is decimal or `0x` hexadecimal, up to 64 bits. Without it the seed comes from the host and
the numbers differ from run to run, as on the board. Anything else (text, a negative number, one
too large) prints `cmm2-emulator: bad seed '<value>'` and the usage text, and exits with status 2.
A fully repeatable scripted run needs more than the seed:

- input only through `--on` (nothing typed at the terminal, no window);
- the same starting `--flash` and `--sd` images each time. Both are written back at the end of a
  run, so copy them first, or the second run starts from what the first left;
- no reading of the clock: with `OPTION DS3231 ON` the firmware takes `TIME$` and `DATE$` from the
  DS3231 model, which follows the host's clock, and a chip that has been set keeps following it
  (see below).

I checked it with a flash file that had been through the first boot: the same seed twice, and the
same value as `42` and as `0x2A`, printed the same three numbers for `PRINT RND;RND;RND`, and
another seed, or none, printed others.

To see the screen of a scratch run, add `--window` (the board run has it unless there is
`--console`, `--run` or `--stop-at`). It opens an SDL window that shows what the firmware has drawn
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
is what answers the first-boot keyboard question and anything typed before USB is up. On that path
printable keys come as SDL text input, so the host layout and Shift apply, and the other keys are
sent as the bytes the firmware's console reads from a terminal. Enter is CR, Backspace is 0x08 (the
firmware ignores 0x7F), and Tab and Escape are themselves. The arrows are `ESC [ A` to `D`, Home and
End are `ESC [ 1 ~` and `ESC [ 4 ~` (measured: the firmware does not read xterm's `ESC [ H` and
`ESC [ F`), Insert, Delete, Page Up and Page Down are `ESC [ 2 ~`, `3~`, `5~` and `6~`, F1 to F4 are
`ESC O P` to `S`, and F5 to F12 are `ESC [ 15 ~`, `17~`, `18~`, `19~`, `20~`, `21~`, `23~` and
`24~`. Ctrl with a letter sends the control code, so Ctrl-C stops a running program. Held keys
repeat as SDL repeats them. Text other than ASCII is dropped, as the console has no way to take it.

Once the firmware polls the keyboard the window sends USB reports only, and no console bytes, or
every key would be typed twice. A key that is held across that moment was typed on the console, so
it is typed once, and it is not in the report: it counts again from its next press. Modifiers are
always in the report, so Shift held across the switch still shifts the next key. A system reset
takes the window back to the console until the firmware polls again.

`--console-keys` keeps the old behaviour with `--window`: the keys are the console's all the while
and no USB keyboard is plugged in, which is useful if USB misbehaves. The host terminal
(`--console`) always feeds the UART, in both modes. With `--window` and no `--console` the
window alone gives input; with both, the terminal and the window feed the same console.

`--console` and `--window` run the machine at its own speed: the core's clock (480 MHz) is held to
the wall clock by sleeping, in steps of at most 10 ms, whenever the emulation is ahead. When the
host is too slow it runs flat out and does not try to catch up later, and a stall (dragging the
window, a suspended process) is forgotten rather than made up with a burst. Add `--unpaced` to run
as fast as the host allows. A plain `--run` is never paced. The host is too slow for the machine's
clock (see "Speed" under Status for the figures and which are measured), so pacing does nothing
until the core gets faster. One boot run (200 million instructions, an unpaced `--run` of the first-
boot text) took about 6 s of CPU time on 2026-11-14, on a loaded machine (a load average of about 20
in the reviewer's repeat). That is slower than the 70 to 77 million a second that I measured for the
boot in my local speed notes, so the two disagree, and I do not know which is right.

`--flash <file>` is the file that keeps the board's settings between runs, as a real board does;
the board's own is `settings.flash` in its directory, saved and loaded in the same way, and
`--flash` names another. If the file exists it is loaded at the start, and the flash and the clock
are written back to it when the run ends. That is the quit key, the window closed, the `--run`
limit, `--stop-at`, a fault, a SIGINT, SIGTERM or SIGHUP (Ctrl-C in a terminal that is not in raw
mode, `kill`, the terminal being closed), and a panic in the emulator, which is saved from and then
goes on as a panic (the terminal is put back before its message prints, and while the run goes the
message is held back until the files are written, and then said, as standard error may be a pipe
that nobody is reading and a message with a backtrace may not fit in it). A panic that comes while
another is being unwound (a `Drop` that panics) is an abort, and nothing is saved then; what was
held and the new message are written at once, so that they are not lost with it. The write goes
through a temporary file in the same directory (named `.cmm2.<pid>.<time>.tmp`, so that one left by
a crash is never in the way), which is synced and renamed over the file, and then the directory is
synced, so a crash never leaves half a file. A directory that cannot be synced (not readable, or a
file system with no such thing) is a warning on stderr, as the file is in its place by then. A
crash does leave that temporary file, as big as the card. A save holds an advisory lock (`flock`)
on its temporary file while it is open, which goes when the process does, however it ends. When a
run starts, the `.cmm2.<pid>.<time>.tmp` files in the directory that have not been changed for a
minute and that can be locked are removed: nobody is writing them. So a live emulator's file on
this host is never touched, whatever the pid in its name means to this host (another host or
container sharing the directory) and whatever the clock does. On a file system whose locks are not
shared between hosts (NFS mounted with `local_lock=flock` or `nolock`, say) a lock does not show a
save on another host, and then a left-over file of a save that is going on there could be removed
by an emulator starting here, and that save would fail when it renames it (the old file is kept and
nothing is lost, but the run's changes are). Where locking does not work at all the lock fails and
nothing is removed, so a left-over file there has to be removed by hand. Another one that is left
is a file with no read and no write right (mode 000), which cannot be locked: `rm .cmm2.*.tmp` in
the directory clears it. The files that are looked at are only regular files (a FIFO, or a link,
with such a name is left alone). The name has to be digits only, with a process id above 0, to
count. A symlink is followed, even a dangling one (the first save creates what it points to), so
the file it points to is the one replaced and the link stays. The file keeps its permissions. It
does not keep its owner, which only a privileged process can set, and a hard link to the old file
stops seeing the contents, as the file is replaced and not written in place. What the save needs is
a directory it can make a file in, so the file's own permissions do not matter (a read-only file is
replaced and keeps its mode). That is looked at when the run starts, with the effective user's
rights, and a `--flash` file that could not be saved is an error then, naming why: the directory
does not exist (or a part of the path is not a directory, or the links loop), a directory on the
path cannot be searched, the directory is not writable (or is append-only), the file system is
read-only, the file is immutable or append-only (`chattr +i`, `+a`), or it is another user's file
in a sticky directory such as /tmp (unless the user has CAP_FOWNER, which is looked at and not just
being root). A file that cannot be read is refused in words as well. A name that is empty (an unset
shell variable), or ends in a slash, or in `.` or `..`, or is longer than the file system allows
(255 bytes on most) is refused too, and so is a link that leads to such a name. The temporary file
has a short name of its own, so that a name of any allowed length can be saved. Both files have to
be regular files: a device, a FIFO or a directory is refused before anything is opened (a FIFO
would hang the load, and `sudo` with `--sd /dev/mmcblk0` would read a whole card and then rename a
file over its node). What cannot be seen at the start, and so can still fail when the run is over
and lose it, is the rest: a full disk or quota, a file made immutable during the run, a network
file system that goes away, a security module (SELinux, AppArmor) that says no.

Every end of a `--console` or `--window` run says why, on one line of stderr, after the terminal is
put back: `cmm2-emulator: quit key (Ctrl-]) pressed`, `cmm2-emulator: stopped by SIGTERM` (or
`SIGINT`, `SIGHUP`; `signal <n>` for any other), `cmm2-emulator: reached address 0x08000104`
(`--stop-at`), `cmm2-emulator: instruction limit reached` (`--run`) and `cmm2-emulator: window
closed`. The exit codes are as they were: 0 for the quit key, the limit, the address and the window
closing, 1 for a fault (which has its own line, `fault trap: ...`), and a signal ends the process
by that signal. If the guest left the cursor part way along a row, the line starts on a new one.
That is worked out from the bytes the guest wrote, which tell nothing of where an escape sequence
moves the cursor: a colour change or a window title is known to move nothing, but any other escape
(a screen drawn with cursor moves, say) is taken as leaving the cursor on a row with something on
it, so the line goes on a new row that may already have something on it, or may leave an empty row
before it. If the guest's output ends inside an escape sequence that was never finished (at the
limit, say, or at a signal), the line starts with CAN, which cancels it, so that the line is not
taken into it. A script that runs `--console --run N` and expects an empty stderr now finds
`instruction limit reached` there. A run with no console and no window is not changed: its summary
on stdout says how it ended.

With `--console` the terminal is in raw mode, so Ctrl-C goes to the guest and the quit key is
Ctrl-]; a signal sent from outside (SIGTERM, SIGHUP) still ends the run, and so does SIGINT with
`--window` and no `--console`. A signal that was ignored when the emulator started (a job run with
`nohup`, or in the background from a shell that ignores SIGINT) stays ignored. When a signal ends a
run the files are written first, and then the emulator ends by that same signal, so the exit status
is 128 plus its number and a shell stops a loop or an `&&` chain. A second signal ends it at once,
with no save, and with nothing said of a panic's message that was being held (the handler cannot
take a lock or allocate): that is what a second Ctrl-C means, and the run is lost back to the last
time the files were written, which is the start of it. The same is true of a run that is blocked
writing to a pipe, a FIFO or a terminal that nobody reads: the first signal ends it and it saves.
Everything the run writes (the guest's output, the summary, the lines on stderr, `--trace-mmio`, and
the `--trace` file, which may be a FIFO or `/dev/stdout`) goes through one writer, which writes by
what the descriptor is. Regular files, and the memory devices that take any write at once
(`/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random` and `/dev/urandom`), are written directly, 64
KB at a time, with no waiting. A pipe, a FIFO or the slave side of a terminal is reopened through
`/proc/self/fd` with `O_NONBLOCK`, which is an open file description of its own (stdin and stderr
stay blocking), written 64 KB at a time, and when it takes no more the writer waits for it to, or
for the signal, whichever comes first; there is no instant in which a signal can be missed. Anything
else (a socket, the master side of a pseudo-terminal, any other device, or no /proc) is written the
old way, in small chunks after a wait, and there a signal in the instants between the wait and the
write can be missed, and a second one ends the run without the save. A signal that comes while the
files are being written waits for the end of the run as any other, and a second one still ends it at
once. Before the run begins, while the files are being read (which can be slow, or stuck on a hung
mount), a signal ends the process at once with its own status, as there is nothing to save yet. From
the start of the run the main thread takes the signals, as the threads the emulator makes (stdin,
SDL) have them blocked; threads that SDL makes later by itself are not covered. What nothing can
save is the process being killed outright (SIGKILL), an abort, or the power going. The first run,
with no file yet, goes through the first-boot setup and saves the options it makes. A second run
with the same file reaches the prompt with no setup questions and no reset:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --flash board.flash
```

The file holds only what the user changes, never the firmware, so it stays valid when you move
to a newer firmware `.bin`: the stored program and saved options (flash from 0x08120000 to the
end, 896 KB) and then the clock's time and date (8 bytes), which a real board's battery keeps. The
firmware asks its setup questions again if the clock has not been set, so the clock has to be saved
too. The STM32's RTC runs on emulated time, not the host's: it counts with the machine's clock, so
it is slower than the wall clock when the host is slow, and it stands still while the emulator is
off. That makes the file 917,512 bytes, and the run stops with an error, leaving the file alone, if
it is any other size than the three below.

The Gen 2's real time comes from the external DS3231, and the emulator has one fitted, always. It is
on PD4 and PD5, bit-banged by the firmware, and `OPTION DS3231 ON` at the prompt switches the
firmware over to it (it saves the option and reboots), as on the board. Until you set the time the
chip shows the host PC's local time (the system's time zone), so `TIME$` and `DATE$` follow the PC's
clock, daylight saving changes included. Setting `TIME$ =` or `DATE$ =` makes it a clock of its own,
like the real chip: it counts seconds evenly from then on and knows nothing of daylight saving or
time zones, so a time set at 00:59:59 as the PC's clocks go forward does not jump an hour. What it
keeps in the `--flash` file is the time as an offset from the PC's UTC clock, so it goes on counting
while the emulator is closed, as the chip does on its battery. Delete the file, or the chip's state
in it, to get back to the PC's time. Without `--flash` the time you set is lost at the end of the
run. With the option off the firmware uses the STM32's RTC as before and never talks to the chip.
The time zone is read from the system once, when the chip is first asked the time, so a zone changed
while the emulator runs (a new `TZ`, a move to another zone) shows from the next run; the zone's
daylight saving rules are applied at each time. If the PC's clock is stepped (by NTP, or by hand)
while the emulator runs, a chip you have never set follows the step, and one you have set follows it
in UTC; the real chip would not see it, but it only matters while the emulator runs. See
[docs/ds3231.md](docs/ds3231.md).

The chip's state is added to the file only once the chip is no longer as it was made: you have set
its time, cleared its status flags, or written the weekday or the 12-hour form of the hours
register. A file whose chip was never touched is 917,512 bytes, which builds from before the chip
was modelled read too. Once it was, the file is 917,522 bytes: the ten more bytes are the offset
(milliseconds, an i64), a flag byte and the chip's status register. In the flag byte bit 0 says the
time was set, bits 1 to 3 are the weekday count the chip keeps and bit 4 is the 12-hour form. A
build from before the chip was modelled refuses a file of that size ("the saved flash and clock are
917512 bytes ..."), so if you go back to such a build, delete the file or truncate it to its first
917,512 bytes (you then lose the time you set). Files of 917,520 bytes, which an earlier version of
this build wrote (the offset in seconds from the host's local time), still load, with the same time,
and are written back in the new form.

The address 0x08120000, where the user flash starts and the firmware ends, is only known to me
for MMBasic 7.00.00b5 (its image ends 216 bytes below it); I have not checked it against another
release. A firmware image that reaches it is refused at the start with an error, so that a
release that does not fit is noticed and its tail is not merged into the `--flash` file.

`--sd <file>` puts an SD card in the socket, with the file as its contents. The firmware drives the
card in SPI mode over GPIO pins, and the card is a model of that: it answers the commands the
firmware sends (initialisation, single-block and multiple-block read and write, and the CSD and CID
registers) from the image. The file has to exist and has to be a whole number of 512-byte blocks,
and at least 512 KB (the smallest size a version 2.0 CSD can describe), or the run stops with an
error and the file is left alone; the emulator never makes a card out of nothing. When the run ends,
by any of the ways above, the card is written back to the file, in the way `--flash` is, but only if
the guest has changed the card. A card that was only read leaves the file alone, so a run that never
wrote to it does not touch it (its time and inode stay the same), and changes made to the file on
the host meanwhile are not lost. If the guest did write, the whole image is written back from the
emulator's copy and replaces the file, so do not change the file on the host during such a run. A
card is write-protected, as with the switch on a real one, when its file could not be written back:
the file is read-only (its permissions say so), or its directory is not writable (or is
append-only), or its file system is read-only, or the file is immutable or append-only, or it is
another user's file in a sticky directory such as /tmp (the same reasons as for `--flash`, which
refuses to start instead). The emulator says which on stderr; the firmware reads the pin, says "The
physical drive is write protected" and does not write, and nothing is written back whatever happens.
Without `--sd` the socket is empty and the firmware says "SD card not present".

The whole image is held in memory for the run, so the card is as big as the RAM you can spare. I
have only used 64 MB. A few GB is reasonable (a 4 GB card is a 4 GB allocation, read at the start
and written back at the end if the guest wrote to it); I would not go past that. The card says
it is an SDHC card, with the size of the image rounded down to 512 KB in its CSD. Its CID is made
up: manufacturer 0, application "CM", product "CMM2E", serial number 1, made in January 2026.

The image needs a FAT file system on it, which I make with the usual tools and not with the
emulator. For a 64 MB FAT32 card:

```
truncate -s 64M card.img
mkfs.vfat -F 32 -n CMM2 card.img
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --console --flash board.flash --sd card.img
```

`7z l card.img` lists what is on it after a run. To put a file on it before a run, mtools'
`mcopy -i card.img file.bas ::` does it: I put a one-line `PRINT` program on a card that way and
`RUN` ran it (2026-11-14). The firmware sets the card up the first time it is used, which costs
about 5 million instructions; after that it is quick. The CMM2 has no `SAVE` or `LOAD` for programs:
`FILES`, `RUN "T.BAS"`, `LIST "T.BAS"`, `EDIT "T.BAS"` and `AUTOSAVE "T.BAS"` use the card.
`OPTION SD TIMING FAST` (the SPI3 peripheral) is not modelled yet (issue 63). The multiple-block
commands and the CSD and CID are there and unit tested, but the firmware did not use them for
anything I tried with it (a 640 KB file written, copied and read back, `FILES` and `MM.INFO`), so
they have not been seen working against the real firmware.

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
