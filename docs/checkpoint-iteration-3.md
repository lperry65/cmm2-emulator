# Checkpoint: end of iteration 3 (go / no-go)

Written 2026-11-13, at commit 170c81c. The question: does the approach (the real firmware on an
emulated STM32H743, on a vendored zmu core) still look like it will reach the goals in
[goals.md](goals.md)? My answer is **go**, with one decision for Lee about speed.

## What works

The unmodified firmware (7.00.00b5) runs from reset in a terminal:

- boots through the power, clock, RTC, flash, timer, GPIO and USART1 waits;
- first boot: the setup questions (keyboard type, time, date) are answered from the console, the
  options are saved to the emulated flash, and the firmware resets itself;
- second boot: the banner and the `>` prompt;
- typed commands run: `PRINT 1+2` prints ` 3`, `PRINT TIME$` and `PRINT DATE$` give the time and date
  set at setup, counted by the RTC.

Tests at this commit: cmm2-core 402, emulator 31 unit and 45 command-line, vendored zmu 507, 12 ignored
real-firmware tests (they need the firmware file), Python tools 19. fmt and clippy are clean and CI runs
all of it.

## What it cost, and what it found

- The zmu core had real bugs that only the firmware hit: LDR literal decoding, LDR to PC, divide by zero,
  the NVIC shifts (twice), missing system-register writes, IT state lost across exceptions, reset not
  clearing the NVIC and SysTick, and ORN with an immediate missing. All fixed with tests, listed in the
  fork's NOTICE. Each fix was a one-to-few-hours problem, none was architectural. Expect more as new
  firmware paths run.
- Every wait the firmware has needed a small model, not a canned value, and the models stayed small
  (tens to a few hundred lines each). The pattern (thin decorator over SVD reset values, status computed
  on read, shared handles for clock, console and interrupts) has held for nine peripherals.
- Subagent reports about firmware state were repeatedly wrong, so every claim is re-checked by running
  the firmware and by mutation-testing the tests. That has been slower but has not let a wrong model in.

## Speed

Release build, whole boot to the prompt plus three commands: about 644 million instructions in 19 s, so
roughly 34 million instructions a second on a Ryzen 5 5600X. Earlier measurements ranged 30 to 68 million
a second depending on the code being run (idle loops that poll peripherals are the slow end).

The real board does an estimated 220 million instructions a second (zmu's cycle estimate is about 2.2
cycles an instruction; I have **not** measured the board). So the emulator is currently about 4 to 7 times
slower than real. Boot to the prompt takes about 12 to 18 s of wall clock.

That is fine for a prompt and for BASIC that is not time critical, and not fine for a real-time game.
The decision record already named the likely fixes (the 32-byte cache entry copied per step, RAM code
decoded again each step, the cost of a peripheral read on the polling loops: issues 15 and 16). I
estimated 2 to 3 times from those, which would still leave it at or just under real speed. **A faster
core (an optimised interpreter, then block translation) behind the same interface stays the fallback.**

## What is left, in the order I would do it

1. **Video**: the LTDC already keeps the frame timing; it needs the layer registers, the colour table and
   a window that shows the framebuffer at 0x24000000 (L8 through the CLUT). Without it nothing is seen
   but the console. This is the next iteration.
2. **Keyboard**: the console works for typing, but games need held-key state. This needs the USB host
   spike (issue 11), which I would still do early because it is the riskiest unknown: the firmware drives
   the OTG controller in slave (FIFO) mode with no DMA, which is favourable.
3. **Speed** (issues 15, 16, 14, 17): optimise, measure the board's real rate, add the missing
   instructions (`vrint*`, `vmaxnm`, `uxtah`, `pkhbt` and others; MMBasic programs and CSUBs will hit them).
4. **SD card** (SDMMC), **audio** (DAC and the timers that drive it), the RNG (22) and a timer event
   scheduler (28), which the firmware does not need to reach the prompt but programs will.
5. Beyond that: fidelity checks against the board, the game's scripted run as an end-to-end test (the
   state hashes recorded in background.md), and GDB.

## Risks

- **Speed** is the one that could end the approach for games. Mitigation above; the interface to swap the
  core exists.
- **USB host emulation** is unproven. If it turns out too heavy, the fallback is to feed key events
  straight to the firmware's key buffer, which is a workaround and not what I want, so I would raise it
  with Lee first.
- **More zmu bugs** at about the rate seen so far. Cost is predictable, not a threat.
- GitHub's interaction limit on the repo needs renewing by 2027-05-11.

## Decisions for Lee

1. Go on to iteration 4 (video)? My recommendation is yes.
2. Whether to measure the real board's instruction rate (needs the board free, with a small PEEK-only
   timing program); it would turn the speed estimate into a fact.
3. Whether to add a local gate that runs the ignored real-firmware tests before a push (about 5 minutes in
   debug, 1 minute in release), since CI does not have the firmware file and so never runs them.
