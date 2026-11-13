# 0001: CPU core

Status: accepted, 2026-11-12.

## Decision

I'm building the emulator on a vendored fork of [zmu](https://github.com/jjkt/zmu) (Apache-2.0), kept
behind a small interface between the CPU and the rest of the machine so a faster core can replace it later.
I'm not using Unicorn.

## What I needed

A core for the STM32H743: Cortex-M7, ARMv7E-M Thumb-2 with the DSP instructions, the FPv5 double precision
FPU, a proper NVIC, and a way to debug it. It has to run the unmodified firmware, be fast enough for
real-time games, and be licence-compatible with this project (GPL-3.0, and it bundles ST's Apache-2.0
register file). Everything here was measured on a Ryzen 5 5600X in release builds unless it says estimate.

## What I tried

I ran a time-boxed spike on each candidate against the real firmware (v7.00.00b5).

| | Unicorn 2.1.5 | zmu, with local fixes |
|---|---|---|
| Licence | GPL-2.0 only | Apache-2.0 |
| Thumb-2 and double-precision FPU | correct, no unsupported instruction in about 1 G instructions | correct on everything executed; the instructions it lacked are added (below) |
| Interrupts, NVIC, SysTick | none: the M-profile code is stubbed out (`return; // FIXME`), so all of it would be mine to write | present: priorities, nesting, SysTick, SVC/PendSV, FPU stack frames with lazy stacking |
| Debugging | no GDB support; registers, memory, single step and breakpoints are available | GDB server exists, attaches and steps; `continue` with a breakpoint did not work |
| Speed, tight loop | 434 million instructions/s with no hooks | 92 million/s |
| Speed, firmware | 11 to 73 million/s with crude slicing | 86 million/s over the first 100 M instructions, about 58 to 60 million/s once in the idle loops |

Both get through the same boot sequence to the same MMBasic idle loops, given the same fake peripheral values
(clock-ready bits, voltage-ready bits, RTC, RNG, USART flags, flash erase) and working SysTick and flash
interrupts.

## Why not Unicorn

Its crates declare GPL-2.0 only, and the C sources underneath say the same for the package as a whole. That
can't be combined with GPL-3.0 code, or with the Apache-2.0 register file. Relicensing the project to fit one
dependency is the wrong way round. Even setting that aside, I would have to write the whole interrupt model, and
counting instructions costs a hook that takes much of the speed advantage back.

## Why zmu

- The licence is compatible.
- The hard parts (NVIC, exception entry and return, FPU frames, a GDB server) already exist, and the
  firmware boots on it.
- It is easy to read: about 33 k lines, a table-driven decoder, and a test suite that passes (475 tests).

## What it costs

- **It needs a fork.** The bus is hardwired (one flash at address 0, one RAM, one device), it supports 32
  interrupts where the STM32H7 needs about 150, a device cannot raise an interrupt itself, and ITCM, MPU and
  cache registers are missing. My spike patch is about 215 added lines across 16 files, and it fixed four real
  bugs the firmware hit within the first 800,000 instructions: LDR literal decoding, LDR to PC not branching,
  zero-divide always trapping, and the NVIC register decode. Reviewing the import turned up a fifth, the same
  wrong shift in the NVIC clear-pending scan. That is five bugs in code the firmware actually runs, in a core
  with a passing test suite, so I expect more.
- **It is not on crates.io.** The repo is the only source, so it is vendored.
- **Instructions were missing.** From a scan of the whole image: `vrint*`, `vfnma`, `orn` with an
  immediate, `uxtah`, `sxtah` and `pkhbt`, and the fixed-point forms of `vcvt`, which were decoded as
  the plain conversion and so gave wrong answers. I added them (and `vmaxnm`, `vminnm`, `vcvta` and
  its kin) when a plain BASIC `FOR` loop stopped at a `vrintm.f64`; `crates/zmu_cortex_m/NOTICE.md`
  lists each change. Nothing else the scan finds looks like code the firmware runs, except `fstmiax`
  at 0x080018c0 (the VFP save helper beside `fldmiax`, called from what looks like libgcc's
  unwinder), which none of my runs reached and which is still undecoded.
- **It is slow.** At about 90 million instructions a second it is roughly 3 to 5 times slower than Unicorn.
  Its own cycle estimate suggests the firmware averages about 2.2 cycles per instruction, so the real board
  may do about 220 million per second, not 480. If so, zmu is about 2.5 to 3 times slower than real. I have not
  measured the board's real rate. Profiling points at a 32-byte cache entry copied on every step and RAM code
  being decoded again on every step. I estimate 2 to 3 times is reachable by optimising, which would be about
  200 to 300 million instructions a second. That is an estimate, not a measurement.
- **Timing fidelity is weak:** no tail-chaining, no exception-entry cycle accounting, no MPU.

## What would make me revisit this

- After iteration 3 (the prompt working in a terminal) there is a go/no-go checkpoint. If the optimised core
  can't run the game's scripted run fast enough to be useful, a faster core goes behind the same interface: an
  optimised interpreter first, then block translation.
- If maintaining the fork turns into a burden, because upstream moves a lot or because I keep finding decoder
  bugs, I'll reconsider writing my own core on the same design.

## How I'm doing it

- The core goes in as a vendored copy of `zmu_cortex_m` at upstream commit 72feb75, committed unmodified
  first, with a second commit for my changes, so the difference from upstream is always reviewable.
- zmu's licence text is kept, and my changes are listed in a NOTICE file.
- The CPU sits behind a small interface (step, registers, interrupts raised by the machine), so nothing outside
  that interface depends on zmu.

## Where the evidence is

The spike code is not in the repo. The Unicorn spike depends on a GPL-2.0 library, so it stays out of a GPL-3.0
repository. The zmu spike and its patch live in my local `spikes/` folder.
