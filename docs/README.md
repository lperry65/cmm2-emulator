# Documentation

Project documentation lives here as markdown.

- [goals.md](goals.md): what the emulator is for and what follows from that.
- [hardware-map.md](hardware-map.md): the CMM2 Gen 2 hardware as measured and documented.
- [background.md](background.md): what existed before, the three levels of emulator, and the
  sources.
- `decisions/`: short records of design decisions and the evidence for them:
  [0001-cpu-engine.md](decisions/0001-cpu-engine.md) (the CPU core) and
  [0002-jit-first.md](decisions/0002-jit-first.md) (speed comes from an x86-64 JIT, built first, and
  the emulator must also run on Windows).
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
