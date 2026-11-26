# Documentation

Project documentation lives here as markdown.

- [usage.md](usage.md): every option, the board, the settings file, the SD card, how a run ends.
- [settings-format.md](settings-format.md): the board's settings file, format v2: the header,
  the diff entries, the migration from the old raw file, and the versioning rules.
- [status.md](status.md): the detailed status: what works, the CPU core, speed and known gaps.
- [speed.md](speed.md): how fast the interpreter runs today, where the host time goes, the ten
  things I looked at and what each was worth, which figures I measured and which I estimated, and
  what real speed needs (issues 15, 16 and 74).
- [goals.md](goals.md): what the emulator is for and what follows from that.
- [hardware-map.md](hardware-map.md): the CMM2 Gen 2 hardware as measured and documented.
- [background.md](background.md): what existed before, the three levels of emulator, and the
  sources.
- `decisions/`: short records of design decisions and the evidence for them:
  [0001-cpu-engine.md](decisions/0001-cpu-engine.md) (the CPU core) and
  [0002-jit-first.md](decisions/0002-jit-first.md) (speed comes from an x86-64 JIT, built first; the
  result of the JIT study; the constraint that the emulator must also run on Windows).
- [boot-trace.md](boot-trace.md): what the real firmware does at boot (written in iteration 1, and
  extended in iterations 3 and 4 as each model went in).
- [sd-card.md](sd-card.md): how the firmware drives the SD card (SPI mode) and the card model.
- [ds3231.md](ds3231.md): how the firmware talks to the DS3231 clock chip on PD4 and PD5, and the
  model of it.
- [rng.md](rng.md): the RNG the firmware enables at boot (the cause of a false `Error:1`), the model
  of it, and `--rng-seed`.
- [timers.md](timers.md): the model of TIM5 and TIM16 with their update interrupts, which `TIMER`
  and `PAUSE` depend on (issues 28 and 70).
- [video.md](video.md): the video modes, PLL3 and the frame rates, layers, blending and page flips.
- [usb-host.md](usb-host.md): how the firmware's USB host stack sees a keyboard, the plan to emulate
  it, and the model that was built (the keyboard works; mouse and gamepad are not done).
- [checkpoint-iteration-3.md](checkpoint-iteration-3.md): where the project stood at the end of
  iteration 3, with the speed measurements of that day. It is a historical snapshot.

Local copies of manuals, schematics and firmware are kept in `reference/`, which is not tracked.

## Code layout

The repository is a Cargo workspace with three members, two of them mine:

- `cmm2-core`: the emulation (CPU, memory, peripherals). It has no SDL or UI dependency, so it
  can be tested on its own and reused.
- `cmm2-emulator`: the front end binary. It owns the window and input and drives `cmm2-core`.
- `zmu_cortex_m`: the Cortex-M core, a vendored copy of the one in
  [zmu](https://github.com/jjkt/zmu) with my changes to it (listed in
  `crates/zmu_cortex_m/NOTICE.md`). It is Apache-2.0, as upstream; `cmm2-core` depends on it.

`cmm2-core` and `cmm2-emulator` are separate so the SDL front end stays out of the emulation
code.

In `cmm2-emulator`, `main.rs` is a short entry point over modules: `args` (the command line),
`batch` (a run with no console or window), `live` (a run with the console or a window), `machine`
(building the machine), `board` (the board's directory, its lock and what it remembers) and
`launch` (which run is on the board), `storage`, `saveable`, `atomic_write` and `run_and_save`
(the `--flash` and `--sd` files, read at the start and saved safely at the end), `panic_hook`,
`signals`, `output` and `error`, `trace`, `slice`, `report` and `inspect`, `script` (the `--on`
keys), `pacing`, `terminal`, `keys`, `usb_keys`, `window` and `local_clock`. Two more, `run_id`
and `test_support`, are built for the tests only. The unit tests of a module are in
`<module>/tests.rs` beside it, or in the file itself for some of the smaller ones, and the
command-line and real-firmware tests are in `crates/*/tests/`, split by topic.
