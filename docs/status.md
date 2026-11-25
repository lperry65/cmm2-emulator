# Status

The detailed status of the emulator: what works, what the CPU core has had done to it, the speed
figures and which of them are measured, and the open gaps. The short version is in the
[README](../README.md).

The status list below is current to commit 208c7fb (2026-11-20). The CPU, game-run and speed
figures under it date from e3e8936 (2026-11-18) and have not been re-measured at 208c7fb.

As of commit 208c7fb (2026-11-20) the emulator boots the unmodified 7.00.00b5 firmware through its
first-boot setup and a system reset to the `>` prompt and runs BASIC there. 7.00.00RC6 now boots
with its own settings layout (issue 82, level 2). Progress is tracked in the GitHub issues and
milestones.

## What works

- The Cortex-M7 (a vendored and changed copy of the zmu core, see "The CPU core" below) and the
  peripherals the firmware waits on, checked against the real firmware: clocks and power, the RTC,
  the flash controller (erase and program, so the options and a stored program persist), GPIO,
  USART1 as the console, the timers TIM5 and TIM16, the RNG, and the LTDC.
- `TIMER` and `PAUSE` count properly: TIM5 and TIM16 raise their update interrupts (issues 28 and
  70, see [timers.md](timers.md)). The timer clock is a fixed 240 MHz and does not follow the RCC
  (issue 76).
- The RNG is modelled ([rng.md](rng.md)), so `RND` gives different numbers from run to run, as on
  the board, and a fresh boot no longer prints a false `Error:1` (issues 22 and 46).
- Video in every mode and colour depth, with layers, blending and page flips, in an SDL window
  (`--window`) or as a PPM file (`--screenshot`).
- A USB keyboard, driven from the window through the firmware's real USB host stack, so
  `KEYDOWN()`, `INKEY$` and Ctrl-C work as on the board. The console over a terminal also works.
- An SD card on a disk image (`--sd`), in the firmware's default SPI mode.
- A board that remembers itself between runs: its firmware, its settings, the DS3231 clock chip
  and the SD card in its socket (see [the board](usage.md#the-board); `--flash` and `--sd` still
  name files of their own).
- Real-time pacing at the machine's 480 MHz clock (built; it has no effect while the host is
  slower than the machine).

## The game run

Galactic Raiders, the game I am writing for the CMM2, loads and draws all its sprites and colours
(my report). It is far too slow to play yet (speed, issue 74).

It used to stop at an ARMv7E-M DSP instruction that the core did not have, at 0x08146b6c. An
earlier reading took that instruction as UQADD8. It was UQSUB8 (A7.7.210), and the whole DSP
extension was missing, not one instruction (issue 77). All five batches of the work on it are
committed (see "The CPU core").

I tested commit aafbce9 (2026-11-18): it ran through a pty, with a card built
from the game's real files, and no keys were sent to the game. The game ran for the whole 470 s of
host time with no fault trap and no error line other than the quit-key line, and the run was ended
with the quit key. On the emulated clock the game printed `sheet loaded ms=335.3`,
`raiders loaded ms=363.5` and `startup_load_ms=1571.3`. On the board I measured 751 ms for a 1 MB
`LOAD DATA` and 1,740 ms for the whole start-up (issue 70).

I do not know that the emulated figures measure the same things as the board's. If they do, the
emulator is faster than the board in emulated time, which would fit the missing memory wait states
(issue 65); that is a hypothesis on issue 77 and I have not checked it. My reading of why it
differs from the empty `FOR` loop in "Speed" below, where the emulated machine is slower than the
board: that figure compares CPU cycle counts on a loop that runs in fast memory, and the load
figures involve SDRAM and file reads, so they are different measurements. That is a reading, not a
measurement.

A screenshot at the end of the run showed the HUD (`SCORE 000300`, `HI 020000`,
`WAVE 3`), a starfield, six empty outline boxes and a white dot, which looks like the game's demo
or attract mode (my reading; I did not check the game's code). I make no claim about whether the
boxes are the game's own.

My own first run returned straight to the shell. The fix for silent ends is in (issue 71,
closed): every end of a live run now says why. I still do not know what ended that first run, and
the issue says it was not reproduced.

## The CPU core

The core is my changed copy of zmu, and the changes are listed in
`crates/zmu_cortex_m/NOTICE.md`, which is the detailed record; the decision to use it is
[decisions/0001-cpu-engine.md](decisions/0001-cpu-engine.md). This is the short list of what it
has now, from the commits and that file.

Checked against the ARMv7-M manual since the first boot:

- Exceptions that arise while another is entered are derived exceptions as the manual has them. An
  exception entry counts 12 cycles and a tail-chained entry 6 (the 6 is ARM's figure for the
  Cortex-M3 and M4, which I have not verified for the M7).
- A lockup is a state of its own with the priorities the manual gives, and a reset loads the
  vector table from a boot address.
- SMULL, which multiplied unsigned, is fixed (issue 64).
- Alignment UsageFaults are modelled (8260358, issues 68 and 67, closed): the instructions that
  always fault on an unaligned address do so, LDR, STR, LDRH, LDRSH, STRH and TBH only with
  `CCR.UNALIGN_TRP` set, the fault is precise, and `CFSR.UNALIGNED` is recorded, with escalation
  to a HardFault. The same commit stops the DWT cycle counter in sleep unless `DBGSLEEP_D1` is
  set (the emulator does not sleep on WFI yet, issue 72) and fixes two reset-related bugs.

The ARMv7E-M DSP extension and the base instructions that were missing (issue 77, in five
batches; batches 1 to 4 are commits 001a7bf, aafbce9, 8fc5d97 and e3e8936, and the last batch
adds the floating-point instructions; the core fixes in the last bullet are from 4bd964d):

- The 36 parallel add and subtract instructions; QADD, QSUB, QDADD and QDSUB; SMMUL, SMMLA and
  SMMLS with their R forms; USAD8 and USADA8; SXTB16, SXTAB16, UXTB16 and UXTAB16; SSAT16 and
  USAT16 (they used to decode as SSAT and USAT, so they ran with a wrong result); SMULW and SMLAW;
  SMUAD, SMUSD, SMLAD and SMLSD with their X forms; SMLALD and SMLSLD; SMLALBB, SMLALBT, SMLALTB
  and SMLALTT; UMAAL; and LDMDB, also written LDMEA.
- The floating-point additions, the last batch: VMLA, VMLS, VNMLA and VNMLS in F32 and F64; VCVTB
  and VCVTT between half precision and single or double precision, both directions; and FLDMX and
  FSTMX, the odd-word-count forms of VLDM and VSTM.
- Each batch was compared with an independent emulator (Unicorn, which is QEMU underneath) and with
  an independent hand-derived model (issue 77). One difference is left on purpose: for SMLAD I
  follow the manual's rule for the Q flag, which differs from the older QEMU inside Unicorn
  (details in NOTICE.md). Real silicon was not available to decide it.
- Decodes that ran wrong are fixed. CSDB and the unallocated hints no longer decode as a branch,
  and every DBG option executes as a NOP. The unprivileged loads and stores (LDRT, LDRBT, LDRHT,
  LDRSBT, LDRSHT, STRT, STRBT and STRHT) exist; the core has no MPU model, so they act as
  ordinary accesses. A coprocessor instruction for a coprocessor other than 10 and 11 raises
  NOCP, and CDP is no longer decoded as MCR. MSR and MRS refuse their UNPREDICTABLE shapes.
- The local exclusive monitor is modelled: LDREX tags the word, STREX passes only to the tagged
  word, and CLREX, exception entry, exception return and reset open the monitor. Before, STREX
  always succeeded.
- Core fixes (commit 4bd964d): the IT state after an exception return (issue 79), the order of
  the write back in LDM before a load of PC (issue 80), SP bits 1:0 read as zero and ignore
  writes (issue 78, ARM B1.4.1), and one cycle rule for LDM, LDMDB and POP (1 + N, plus 3 for a
  load of PC).

What is still missing: an MPU model, and the Device and Strongly-ordered memory types, as the
core has none.

## Speed

Speed is the main thing that is not done. These are the figures, and which of them I measured:

- Measured: the core runs about 54.5 million instructions a second at the prompt (2026-11-13,
  release build; [usb-host.md](usb-host.md) section 12), and about 24 to 25 million in the BASIC
  interpreter (`FOR i=1 TO 100000:a=a+1:NEXT`, a quiet machine; the working is in my local speed
  notes, which are not in the repository).
- Estimated, not measured: the board runs about 220 million instructions a second (zmu's cycle
  estimate, as in [checkpoint-iteration-3.md](checkpoint-iteration-3.md)). By that estimate the
  host is about four times too slow. That "about four times" is an estimate from instruction
  counts.
- Measured on a program: an empty `FOR` loop of 400,000 iterations takes 255.7 ms on the emulated
  machine's clock (read with `TIMER`, on 2026-11-14), and the board takes 44.944 ms for 100,000
  (my measurement on the board, issue 70). So the emulated machine takes about 1.4 times as long
  as the board on that loop. This compares the machine's clock with the board's, not the host's
  speed.
- Pacing: one boot run (200 million instructions, an unpaced `--run` of the first-boot text) took
  about 6 s of CPU time on 2026-11-14, on a loaded machine. That is slower than the 70 to 77
  million a second that I measured for the boot in my local speed notes, so the two disagree, and I
  do not know which is right.

I decided on 2026-11-14 to build an x86-64 JIT directly, with no interim steps, because the
interim steps (an estimate of about 2.7 to 2.8 times together; two of the five were measured on
prototypes; the working is in my local speed notes) cannot reach the board's speed. See
[decisions/0002-jit-first.md](decisions/0002-jit-first.md) (issue 74).

The JIT study is done (2026-11-18): a design note and a spike exist locally and are not in the
repository. Its recommendation is a hand-written x86-64 emitter behind a block IR that does not
depend on the host, with the interpreter as the fallback; Cranelift stays an option for aarch64.
The recommendation and nine open questions are on issue 74. The questions wait for me, and none of
the JIT is built.

## Windows

The project is cross-platform Rust. I have built and run it on Linux only so far (CI runs on
Ubuntu). Windows is planned (issue 75): several parts of the front end are Unix-only now (the
signal handling, the terminal's raw mode, file locks and `/proc`), and the SDL2 build and a
Windows CI job are not done. The JIT design has to weigh portability for the same reason.

## Known gaps

The open issues at 208c7fb, from `gh issue list --state open --limit 100`:

- Speed and timing: 74 (the JIT), 15 (the speed of the machine's bus path), 16 (zmu fidelity gaps
  found while integrating), 65 (memory wait states), 72 (WFI never sleeps), 76 (timer clocks do
  not follow the RCC).
- The CPU core: 77 (the DSP extension and the missing base instructions: done, in five batches,
  the last one the floating-point additions), 59 (zmu follow-ups after dec44de, to
  check against the ARMv7-M manual), 17 (readable disassembly in the trace).
- Console and input: 66 (console input arrives faster than the baud rate), 48 (pasted lines are
  joined), 47 (a hard fault on `ESC O D`, the SS3 left arrow), 62 (keyboard follow-ups), 53 (USB
  mouse), 54 (gamepad), 81 (the live-run end line: the cursor tracker cannot know where an escape
  moves the cursor, and the window-closed end has no end-to-end test).
- Storage and the board's state: 63 (SPI3 for the SD card, `OPTION SD TIMING FAST`), 44 (a host
  folder as the SD card), 41 (the guest's 32-byte write into the firmware region of flash: the
  firmware's own first-start write, which the settings file now keeps, so only whether the
  hardware would accept that write remains).
- Window and platform: 39 (scaling, full screen and aspect controls), 75 (Windows).
- Firmware and end-to-end: 82 (support for firmware versions: 7.00.00b5 and 7.00.00RC6 have
  settings layouts, 8.00.00b2 and any other release do not), 57 (firmware 5.07, the version the
  Gen 2 shipped with), 49 (run Galactic Raiders as an end-to-end test: a run goes on for minutes
  with no fault, but the hash test is not done), 25 (boot to the MMBasic idle loop: the
  acceptance is only partly met. No test asserts the idle loop; the tests assert the prompt and
  commands, in `crates/cmm2-core/tests/console.rs`. `GPIOH_IDR`, `GPIOD_IDR` and `TIM5_CNT`
  appear in [boot-trace.md](boot-trace.md) only in its iteration-1 sections, as an expectation
  from an earlier spike that was never confirmed. A speed note exists, in my local speed notes,
  which are not in the repository).
- Not in an issue: sound (the DAC) and a GDB option (the vendored core has a GDB server, but the
  front end does not start it).

## Target

- Colour Maximite 2 Generation 2: STM32H743 (Cortex-M7, 480 MHz), 32 MB SDRAM, LTDC video.
- Official MMBasic firmware: my target is 7.00.00b5 (`MM.VER` 7.000005) and 7.00.00RC6. I am
  moving to RC6, and b5 stays the target until then. b5 and RC6 have settings layouts.
- CMM2 only. PicoMite emulation is out of scope for now.
