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
  requirement, not polish, and the CPU engine choice (issue 5) is judged against it.
- **Host folder as the SD card**, as Fab Agon does, so files move in and out with no disk image tools.
  A disk image is also supported.
- **Proper keyboard (and later mouse) input**, including held-key state for games. Typing through
  the serial console alone is not enough. USB host emulation is therefore on the critical path and
  needs an early spike.
- **Sound** through the DAC.
- **Window, scaling and full screen** like Fab Agon.
- **Debugging.** A GDB remote stub (the CPU is ARM, so `gdb-multiarch` works) and an instruction
  trace, for firmware bring-up and for debugging CSUBs.
- **Fidelity checked against a real board.** Behaviour is compared with the real CMM2 over its serial
  console, using test programs.

## Not goals for now

- PicoMite emulation.
- Cycle-exact timing.
- Emulating the Generation 1 board, unless it turns out to come almost for free.
