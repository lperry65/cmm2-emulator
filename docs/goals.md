# Goals

The reference point is the Fab Agon Emulator for the Agon Light 2. The aim is a CMM2 Gen 2
emulator that is as useful for development as that one is.

## The workflow it must support

1. Write and edit MMBasic programs in the emulator (the built-in editor, or a host editor).
2. Run them, including real-time games that poll the keyboard and draw every frame.
3. Save them to an emulated SD card, and move the same files onto a real CMM2 SD card unchanged.
4. Develop games before touching hardware, as was done for the Agon Light 2 game: write it, run it
   in the emulator, and only then put it on the real machine.
5. Native code (CSUBs compiled from C) runs unchanged, because the real firmware runs on an
   emulated Cortex-M7.

## What follows from that

- **Runs the official firmware unmodified.** The user supplies the firmware file.
- **Real-time speed.** Games must run at their intended frame rate on an ordinary PC. Speed is a
  requirement, not polish, and the CPU engine choice (issue 5) is judged against it. It is not met
  yet: the plan to get there is an x86-64 JIT built first
  ([decisions/0002-jit-first.md](decisions/0002-jit-first.md), issue 74).
- **Cross-platform.** The emulator is Rust and is meant to build and run on Windows as well as
  Linux. I have built and run it on Linux only so far; Windows is planned (issue 75).
- **Host folder as the SD card**, as Fab Agon does, so files move in and out with no disk image
  tools. A disk image is also supported, and works (`--sd`); the host folder is not built (issue
  44).
- **Proper keyboard (and later mouse) input**, including held-key state for games. Typing through
  the serial console alone is not enough. USB host emulation is therefore on the critical path and
  needs an early spike. As of e3e8936 (2026-11-18) the spike is done and the keyboard is built
  (issues 11 and 50 to 52, closed; the model is described in [usb-host.md](usb-host.md)), and
  `KEYDOWN()` and `INKEY$` work from the window. The mouse (issue 53) and the gamepad (issue 54) are
  open.
- **Sound** through the DAC (not built yet).
- **Window, scaling and full screen** like Fab Agon. The window exists and scales to fit; the
  controls for scaling, full screen and aspect are not built (issue 39).
- **Debugging.** A GDB remote stub (the CPU is ARM, so `gdb-multiarch` works) and an instruction
  trace, for firmware bring-up and for debugging CSUBs. The instruction trace exists (`--trace`);
  there is no GDB option yet: the vendored core has a GDB server, but the front end does not start
  it.
- **Fidelity checked against a real board.** Behaviour is compared with the real CMM2 over its
  serial console, using test programs.

## Not goals for now

- PicoMite emulation.
- Cycle-exact timing.
- Emulating the Generation 1 board, unless it turns out to come almost for free.
