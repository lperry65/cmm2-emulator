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

## Since then (2026-11-14, updated 2026-11-20 at commit e3e8936)

The decision stands, and the text above is left as I wrote it on 2026-11-12. Several things in it
have been overtaken:

- - The spike patch was 225 added lines across 16 files with the blank ones counted (241 `+` lines
  less the 16 `+++` headers), and 215 without them, so the "about 215" above is the count without
  blank lines. I counted the added lines of `spikes/zmu/patches/zmu-local.patch` (kept locally, not
  in the repository).
- - The speed comparison is superseded. The core runs about 54.5 million instructions a second at
  the prompt (measured on 2026-11-13, in `usb-host.md`), against an estimated 220 million for the
  board, so it is about four times short by that estimate, not 2.5 to 3. The profiling is in my
  local speed notes (not in the repository), and the way to speed is now a JIT built first: see
  [0002-jit-first.md](0002-jit-first.md). Issues 15 and 16 track the earlier speed work.
- - "No tail-chaining, no exception-entry cycle accounting" is no longer true. The core counts a
  12-cycle entry and a 6-cycle tail-chained entry (the 6 is ARM's Cortex-M3 and M4 figure, which I
  have not verified for the M7), derives exceptions as the ARMv7-M manual does, has an explicit
  lockup state with the priorities the manual gives, and resets to the boot address. The MPU is
  still not modelled.
- - The SMULL bug (it multiplied unsigned) was fixed (issue 64). That was the one that stopped the
  game's loader with `Program damaged: symbol table`.
- - Alignment UsageFaults are modelled (8260358, issues 68 and 67, closed): the always-aligned
  instructions, the `CCR.UNALIGN_TRP` rule for LDR, STR, LDRH, LDRSH, STRH and TBH, precise faults,
  and `CFSR.UNALIGNED` with escalation to a HardFault. The same commit stops the DWT cycle counter
  in sleep unless `DBGSLEEP_D1` is set (WFI does not sleep yet, issue 72) and fixes two
  reset-related bugs.
- - The ARMv7E-M DSP instructions are in (issue 77, batches 1 to 4 in commits 001a7bf, aafbce9,
  8fc5d97 and e3e8936, and the fifth batch, the floating-point additions, with this work; the
  core fixes below are from 4bd964d). Galactic Raiders used to stop at an
  instruction that was missing. It was UQSUB8 (A7.7.210; the reviewer first read it as UQADD8), and
  the whole extension was missing, not one instruction. Now the core has the 36 parallel add and
  subtract instructions, QADD, QSUB, QDADD and QDSUB, SMMUL, SMMLA and SMMLS with their R forms,
  USAD8 and USADA8, SXTB16, SXTAB16, UXTB16 and UXTAB16, SSAT16 and USAT16 (they used to decode as
  the wrong instruction), SMULW, SMLAW, SMUAD, SMUSD, SMLAD, SMLSD, SMLALD, SMLSLD, SMLALBB,
  SMLALBT, SMLALTB, SMLALTT, UMAAL, and LDMDB (LDMEA); and, with the fifth batch, VMLA, VMLS,
  VNMLA and VNMLS in F32 and F64, VCVTB and VCVTT between half precision and single or double
  precision, and FLDMX and FSTMX.
  Each batch was compared with an independent
  emulator (Unicorn, which is QEMU underneath) and with the reviewer's own model (issue 77). For
  SMLAD I follow the manual's rule for the Q flag, which differs from the older QEMU inside Unicorn.
- - Wrong decodes were fixed in the same work: CSDB and the unallocated hints no longer decode as a
  branch, every DBG option executes as a NOP, the unprivileged loads and stores exist (the core has
  no MPU model, so they act as ordinary accesses), a coprocessor instruction for a coprocessor other
  than 10 and 11 raises NOCP, CDP is no longer decoded as MCR, and MSR and MRS refuse their
  UNPREDICTABLE shapes. The local exclusive monitor is modelled (STREX used to always succeed). The
  IT state after an exception return (issue 79), the write back of LDM before a load of PC (issue
  80) and SP bits 1:0 (read as zero, writes ignored, issue 78) are fixed, and LDM, LDMDB and POP
  follow one cycle rule (1 + N, plus 3 for a load of PC).
- - The floating-point additions are in now too (the last batch of issue 77), so the `fstmiax`
  that the text above calls undecoded runs instead of raising a fault. What is still missing in
  the core: an MPU model, and the Device and Strongly-ordered memory types, as the core has none.
  Issue 59 holds the other core follow-ups.
- - Every change I made to the core is listed in `crates/zmu_cortex_m/NOTICE.md`, which has grown
  well past the spike patch: the interrupt count is 240 now, the fidelity fixes came from checking
  the core against the ARMv7-M manual, and so on.

## Where the evidence is

The spike code is not in the repo. The Unicorn spike depends on a GPL-2.0 library, so it stays out
of a GPL-3.0
repository. The zmu spike and its patch are kept locally in a `spikes/` folder, not in the
repository.
