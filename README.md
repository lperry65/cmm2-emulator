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
ends. That is the quit key, the window closed, the `--run` limit, `--stop-at`, a fault, a SIGINT,
SIGTERM or SIGHUP (Ctrl-C in a terminal that is not in raw mode, `kill`, the terminal being
closed), and a panic in the emulator, which is saved from and then goes on as a panic (the
terminal is put back before its message prints, and while the run goes the message is held back
until the files are written, and then said, as standard error may be a pipe that nobody is
reading and a message with a backtrace may not fit in it). A panic that comes while another is
being unwound (a `Drop` that panics) is an abort, and nothing is saved then; what was held and the new message are written at once, so that they are not lost with it. The write goes through a temporary file in the
same directory (named `.cmm2.<pid>.<time>.tmp`, so that one left by a crash is never in the
way), which is synced and renamed over the file, and then the directory is synced, so a crash
never leaves half a file. A directory that cannot be synced (not readable, or a file system with
no such thing) is a warning on stderr, as the file is in its place by then. A crash does leave
that temporary file, as big as the card. A save holds an advisory lock (`flock`) on its temporary
file while it is open, which goes when the process does, however it ends. When a run starts, the
`.cmm2.<pid>.<time>.tmp` files in the directory that have not been changed for a minute and that
can be locked are removed: nobody is writing them. So a live emulator's file on this host is never touched,
whatever the pid in its name means to this host (another host or container sharing the
directory) and whatever the clock does. On a file system whose locks are not shared between
hosts (NFS mounted with `local_lock=flock` or `nolock`, say) a lock does not show a save on
another host, and then a left-over file of a save that is going on there could be removed by
an emulator starting here, and that save would fail when it renames it (the old file is kept
and nothing is lost, but the run's changes are). Where locking does not work at all the lock
fails and nothing is removed, so a left-over file there has to be removed by hand. Another one
that is left is a file with no read and no write right (mode 000), which cannot be locked:
`rm .cmm2.*.tmp` in the directory clears it. The files that are looked at are only regular
files (a FIFO, or a link, with such a name is left alone). The name has to be digits only, with a process id above 0, to count. A symlink is followed, even a dangling one (the first save creates what
it points to), so the file it points to is the one replaced and the link stays. The file keeps
its permissions. It does not keep its owner, which only a privileged process can set, and a hard
link to the old file stops seeing the contents, as the file is replaced and not written in
place. What the save needs is a directory it can make a file in, so the file's own permissions do
not matter (a read-only file is replaced and keeps its mode). That is looked at when the run
starts, with the effective user's rights, and a `--flash` file that could not be saved is an error
then, naming why: the directory does not exist (or a part of the path is not a directory, or the
links loop), a directory on the path cannot be searched, the directory is not writable (or is
append-only), the file system is read-only, the file is immutable or append-only (`chattr +i`,
`+a`), or it is another user's file in a sticky directory such as /tmp (unless the user has
CAP_FOWNER, which is looked at and not just being root). A file that cannot be read is refused in
words as well. A name that is empty (an unset shell variable), or ends in a slash, or in `.` or
`..`, or is longer than the file system allows (255 bytes on most) is refused too, and so is a
link that leads to such a name. The
temporary file has a short name of its own, so that a name of any allowed length can be saved. Both files have to be regular files: a device, a FIFO or a directory is refused before
anything is opened (a FIFO would hang the load, and `sudo` with `--sd /dev/mmcblk0` would read a
whole card and then rename a file over its node). What cannot be seen at the start, and so can
still fail when the run is over and lose it, is the rest: a full disk or quota, a file made
immutable during the run, a network file system that goes away, a security module (SELinux,
AppArmor) that says no.

With `--console` the terminal is in raw mode, so Ctrl-C goes to the guest and the quit key is
Ctrl-]; a signal sent from outside (SIGTERM, SIGHUP) still ends the run, and so does SIGINT with
`--window` and no `--console`. A signal that was ignored when the emulator started (a job run
with `nohup`, or in the background from a shell that ignores SIGINT) stays ignored. When a signal
ends a run the files are written first, and then the emulator ends by that same signal, so the
exit status is 128 plus its number and a shell stops a loop or an `&&` chain. A second signal
ends it at once, with no save, and with nothing said of a panic's message that was being held
(the handler cannot take a lock or allocate): that is what a second Ctrl-C means, and the run is lost back to
the last time the files were written, which is the start of it. The same is true of a run that is
blocked writing to a pipe, a FIFO or a terminal that nobody reads: the first signal ends it and
it saves. Everything the run writes (the guest's output, the summary, the lines on stderr,
`--trace-mmio`, and the `--trace` file, which may be a FIFO or `/dev/stdout`) goes through one
writer, which writes by what the descriptor is. Regular files, and the memory devices that take
any write at once (`/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random` and `/dev/urandom`), are
written directly, 64 KB at a time, with no waiting. A pipe, a FIFO or the slave side of a
terminal is reopened through `/proc/self/fd` with `O_NONBLOCK`, which is an open file description
of its own (stdin and stderr stay blocking), written 64 KB at a time, and when it takes no more
the writer waits for it to, or for the signal, whichever comes first; there is no instant in
which a signal can be missed. Anything else (a socket, the master side of a pseudo-terminal, any
other device, or no /proc) is written the old way, in small chunks after a wait, and there a
signal in the instants between the wait and the write can be missed, and a second one ends the
run without the save. A signal that comes while the files are being written waits for the
end of the run as any other, and a second one still ends it at once. Before the run begins, while the files are
being read (which can be slow, or stuck on a hung mount), a signal ends the process at once with
its own status, as there is nothing to save yet. From the start of the run the main thread takes
the signals, as the threads the emulator makes (stdin, SDL) have them blocked; threads that SDL
makes later by itself are not covered. What nothing can save is the process being killed outright (SIGKILL), an abort, or the power going.
The first run, with no file yet, goes through the first-boot setup and saves the options it makes.
A second run with the same file reaches the prompt with no setup questions and no reset:

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

The Gen 2's real time comes from the external DS3231, and the emulator has one fitted, always. It
is on PD4 and PD5, bit-banged by the firmware, and `OPTION DS3231 ON` at the prompt switches the
firmware over to it (it saves the option and reboots), as on the board. Until you set the time the
chip shows the host PC's local time (the system's time zone), so `TIME$` and `DATE$` follow the
PC's clock, daylight saving changes included. Setting `TIME$ =` or `DATE$ =` makes it a clock of
its own, like the real chip: it counts seconds evenly from then on and knows nothing of daylight
saving or time zones, so a time set at 00:59:59 as the PC's clocks go forward does not jump an hour.
What it keeps in the `--flash` file is the time as an offset from the PC's UTC clock, so it goes on
counting while the emulator is closed, as the chip does on its battery. Delete the file, or the
chip's state in it, to get back to the PC's time. Without `--flash` the time you set is lost at the
end of the run. With the option off the firmware uses the STM32's RTC as before and never talks to
the chip. The time zone is read from the system once, when the chip is first asked the time, so a zone
changed while the emulator runs (a new `TZ`, a move to another zone) shows from the next run; the
zone's daylight saving rules are applied at each time. If the PC's clock is stepped (by NTP, or by
hand) while the emulator runs, a chip you have never set follows the step, and one you have set
follows it in UTC; the real chip would not see it, but it only matters while the emulator runs.
See [docs/ds3231.md](docs/ds3231.md).

The chip's state is added to the file only once you have set its time or cleared its status
flags. A file whose chip was never set is 917,512 bytes, which builds from before the chip was
modelled read too. After a run that set the time the file is 917,522 bytes: the ten more bytes are
the offset (milliseconds, an i64), whether the time was set, and the chip's status register. A build
from before the chip was modelled refuses a file of that size ("the saved flash and clock are 917512
bytes ..."), so if you go back to such a build, delete the file or truncate it to its first
917,512 bytes (you then lose the time you set). Files of 917,520 bytes, which an earlier
version of this build wrote (the offset in seconds from the host's local time), still load, with the
same time, and are written back in the new form.

The address 0x08120000, where the user flash starts and the firmware ends, is only known to me
for MMBasic 7.00.00b5 (its image ends 216 bytes below it); I have not checked it against another
release. A firmware image that reaches it is refused at the start with an error, so that a
release that does not fit is noticed and its tail is not merged into the `--flash` file.

`--sd <file>` puts an SD card in the socket, with the file as its contents. The firmware drives the
card in SPI mode over GPIO pins, and the card is a model of that: it answers the commands the
firmware sends (initialisation, single-block and multiple-block read and write, and the CSD and
CID registers) from the image. The file has to exist and has to be a whole number of 512-byte
blocks, and at least 512 KB (the smallest size a version 2.0 CSD can describe), or the run stops
with an error and the file is left alone; the emulator never makes a card out of nothing. When the run ends, by any of the ways above, the card is written back to the
file, in the way `--flash` is, but only if the guest has changed the card. A card that was only
read leaves the file alone, so a run that never wrote to it does not touch it (its time and inode
stay the same), and changes made to the file on the host meanwhile are not lost. If the guest did
write, the whole image is written back from the emulator's copy and replaces the file, so do not
change the file on the host during such a run. A card is write-protected,
as with the switch on a real one, when its file could not be written back: the file is read-only
(its permissions say so), or its directory is not writable (or is append-only), or its file
system is read-only, or the file is immutable or append-only, or it is another user's file in a
sticky directory such as /tmp (the same reasons as for `--flash`, which refuses to start
instead). The
emulator says which on stderr; the firmware reads the pin, says "The physical drive is write
protected" and does not write, and nothing is written back whatever happens. Without `--sd` the socket is empty and the
firmware says "SD card not present".

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
`mcopy -i card.img file.bas ::` should do it, though I have not tried that here. The firmware sets the card up the first time it is used, which costs
about 5 million instructions; after that it is quick. The CMM2 has no `SAVE` or `LOAD` for
programs: `FILES`, `RUN "T.BAS"`, `LIST "T.BAS"`, `EDIT "T.BAS"` and `AUTOSAVE "T.BAS"` use the
card. `OPTION SD TIMING FAST` (the SPI3 peripheral) is not modelled yet. The multiple-block
commands and the CSD and CID are there and unit tested, but the firmware did not use them for
anything I tried with it (a 640 KB file written, copied and read back, `FILES` and `MM.INFO`), so
they have not been seen working against the real firmware.

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
