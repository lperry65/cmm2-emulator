# 0002: Speed comes from an x86-64 JIT, built first

Status: accepted, 2026-11-14 (issue 74). It is not built yet.

## Decision

I'm going to build the x86-64 translation of the guest's code directly: Thumb-2 blocks turned into
host machine code. I'm not doing the cheap interim speed steps first, and I'm not building a
block-interpreter stage (option A below) on the way. The aim is
to run as fast as the host allows, and then pace down to the board's 480 MHz clock, which the pacing
already does.

The emulator must also build and run on Windows (issue 75). That is a constraint on this work, and
it is described below.

## What I needed

Real-time games. The figures, and where each comes from (the profiling working is in my local speed
notes, which are not in the repository):

- **Measured:** the core runs about 54.5 million instructions a second at the prompt. I measured
  it on 2026-11-13 over the 400 million instructions after the prompt, in a release build on a
  Ryzen 5 5600X; it is also in `docs/usb-host.md` section 12.
- **Measured:** the BASIC interpreter runs about 24 to 25 million instructions a second, on
  `FOR i=1 TO 100000:a=a+1:NEXT` after a first boot, in a release build on a quiet machine. Treat
  absolute rates as plus or minus 20 percent.
- **Estimated, not measured:** the board runs 215 to 300 million instructions a second depending on
  the code mix (about 220 at the prompt). This is zmu's cycle estimate (about 2.2 cycles an
  instruction, as in `docs/checkpoint-iteration-3.md`), and I have not measured it on the board.
  BASIC needs 215 to 270 million by that estimate.
- **Measured, but about the cycle model and not the host:** an empty `FOR` loop of 400,000
  iterations took 255.7 ms on the emulated clock (`TIMER`, 2026-11-14) against the board's
  44.944 ms for 100,000 (issue 70). That is about 1.4 times as long as the board, and it does not
  say how fast the host runs the emulation.

Galactic Raiders loads and draws everything now (my report), and is far too slow to play. In a
reviewer's run it ran for about 95 s of host time before it stopped at an unimplemented DSP
instruction (issue 77).

## What I looked at

| | Gain | Effort | Where it ends up (BASIC) |
|---|---|---|---|
| Interim steps: a decoded-instruction cache for ITCM and RAM, SysTick in closed form, less per-step work, a direct RAM path, execution by reference | an estimate of about 2.7 to 2.8 times all together (see below) | half a day to two days each, my estimate | about 70 million a second, estimated |
| A. Pre-decoded blocks of micro-ops | about 150 million a second, estimated | 2 to 4 weeks, estimated | short of the board |
| B. Block translation to x86-64 | 400 million to 1 billion a second, estimated | 6 to 10 weeks, estimated | past the board |

The 2.7 to 2.8 is the product of five gains: the ITCM decode cache 2.0 (measured on a prototype in a
scratch copy: 4.23 s against 2.11 s of host CPU time for 53 million BASIC instructions, runs
interleaved on a loaded machine), SysTick in closed form 1.09 (measured on a prototype), and
1.12, 1.07 and 1.04 for the other three, which are estimates from the profile. The estimates for A
and B are mine, from the cost per instruction I measured and from what such designs usually
reach. I did not prototype either.

## Why this one

- The interim steps can't reach the board's speed. About 70 million a second is a quarter to a
  third of the 215 to 270 million that BASIC needs, and the work would still leave B to do.
- A would get to about 150 million, which is still short, and it is a stage I would then replace.
- B is the only option whose estimated range is above the board's estimated rate.

## What I keep doing regardless

These are correctness, not speed, and the translation needs them as they are:

- Memory wait states, so that SDRAM and the other regions cost cycles as on the board (issue 65).
- `TIMER` counting properly (issue 70, done).
- The DWT cycle counter and SysTick.

Building the frame off the CPU thread is independent of this, and waits for a profile.

## What it costs

- **The hard parts are the FPU and exactness.** The board's FPv5 double precision with Arm's NaN and
  rounding rules, precise exceptions, self-modifying code (MMBasic copies code into RAM, as issue
  74 has it), and faults inside a block. These are the risks I see for B.
- **The existing interpreter is the oracle.** The test is to run both one instruction at a time and
  compare registers, flags and memory. That has to exist before the translation can be trusted.
- **The interpreter stays.** It is the fallback for hosts the translation does not cover, and it is
  what GDB and the trace use, as they need single steps.
- **6 to 10 weeks**, as an estimate.

## Windows (issue 75)

The emulator is cross-platform Rust and must build and run on Windows as well as Linux. I have
built and run it on Linux only so far; the emulator crate is Unix-only in several places (the
signal handling, the terminal's raw mode, the `flock` locks and `/proc` use in the saving and the
output), and none of it is ported yet. For this decision it means:

- The choice of code generator has to weigh portability. Cranelift covers more hosts than a
  hand-written x86-64 emitter: it would also serve Windows on ARM and Apple Silicon, where an x86-64
  emitter serves x86-64 only. I have not decided between them. This record does not pick one.
- A path with no translation always has to remain, which is the interpreter, for any host the JIT
  does not support.
- The host differences have to be designed for from the start: how executable memory is obtained
  and protected on each system, and the calling convention at the edge between generated code and
  the rest of the emulator.

## What would make me revisit this

- A prototype of B that comes out well under 400 million a second on the firmware's own code.
- The Windows work finding that the code generator I would pick can't be used on one of the hosts
  I want.
- A measurement of the board's real instruction rate that is far from the estimate of 215 to 300
  million a second, in either direction.

## Where the evidence is

My local speed notes (not in the repository) hold the profiles, the prototype results and the
working for the estimates. Issues 74 (this decision), 75 (Windows) and 65 (wait states) are on
GitHub.
