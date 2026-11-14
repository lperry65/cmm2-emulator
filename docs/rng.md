# The RNG

This is the model of the STM32H743's true random number generator (issue 22), and what I found
when I built it. The sources are ST's RM0433 Rev 9, chapter 34 (RNG) and chapter 8 (RCC), which
I read from `reference/manuals/rm0433.pdf`. Nothing here is copied from it.

## Why it exists

At boot the firmware's init code (around 0x0809dcb8) calls `HAL_RNG_Init` (0x08007a04). That
sets `RNG_CR.RNGEN` and polls `RNG_SR` for `DRDY`, failing on a seed error or after about 2 ms.
With no RNG the poll timed out and the firmware queued `Error:1` in its error buffer (0x20013314).
The first command that came back through the error path printed it (`NEW`, or the end of the
first `RUN`). A real board never prints it, so every user of the emulator saw a false error.

## What the chip does

- The RNG is on AHB2 at 0x48021800 (RM0433 2.3, memory map) with three registers: `RNG_CR`
  (0x00), `RNG_SR` (0x04) and `RNG_DR` (0x08) (34.7).
- `RNG_CR` has `RNGEN` (bit 2), `IE` (bit 3) and `CED` (bit 5), all clear after a reset. `CED`
  cannot be changed while the RNG is enabled (34.7.1).
- `RNG_SR` has `DRDY` (bit 0), `CECS` (1), `SECS` (2), `CEIS` (5) and `SEIS` (6). Only `CEIS`
  and `SEIS` can be written, and only to clear them with a 0; writing 1 does nothing (34.7.2).
- The conditioning stage makes four 32-bit words at a time. `DRDY` is set while there is a word,
  and drops when the fourth has been read, until four more are made (34.3.3, 34.7.2).
- `RNG_DR` gives the next word, and 0 when `DRDY` is clear (34.7.3).
- The first four words come 192 RNG kernel clocks plus 213 AHB clocks after `RNGEN` is set, or
  128 RNG clocks plus 426 AHB clocks when the AHB clock is under 213 x fRNG / 64 (34.5). Each
  later four come 16 x fAHB / fRNG AHB clocks after the buffer empties, but not under 213 (34.5).
- With `RNGEN` clear the words already made can still be read. A start that was interrupted
  carries on from where it was when `RNGEN` is set again (34.3.8).
- The interrupt (`IE`) is pending while `DRDY`, `SEIS` or `CEIS` is set, and only with `RNGEN`
  set. `DRDY` clears itself; the other two clear by writing 0 (34.4, Table 281). It is IRQ 80,
  `HASH_RNG`, shared with the HASH (Table 144).
- A kernel clock under the AHB clock / 32, with `CED` clear, sets `CECS` and `CEIS`. `CECS` goes
  again when the clock is right; `CEIS` stays until cleared (34.3.6, 34.3.7).
- A seed error sets `SEIS` and `SECS` and stops the generator. Recovery is to clear `SEIS`,
  read 12 words and throw them away, then check `SEIS` is still clear (34.3.7).
- The RCC gates it: `RCC_AHB2ENR.RNGEN` (bit 6) turns on the kernel clock and the bus clock, and
  `RCC_D2CCIP2R.RNGSEL` picks the kernel clock: hsi48, pll1_q, lse or lsi (8.7.5, 8.7.42,
  Figure 58). `RCC_AHB2RSTR.RNGRST` resets the block (8.7.30). The register has two offsets,
  0x0DC and 0x13C (Table 68). With the bus clock off, a read returns invalid data and a write is
  ignored, with no bus error (8.5.11, p. 373).

## What the emulator models

The model is `crates/cmm2-core/src/rng.rs`, with its tests in `rng/tests.rs`.

- All three registers as above: reset values, the bits that can be written, and the clearing
  rules. `DRDY` is worked out from the four-word buffer, and a read of `RNG_DR` takes a word.
- The time to the first four words and to each next four, in RNG kernel clocks, as 34.5 gives
  it. I count it in core cycles of the machine's clock, as the timers do, never in host time.
  The work only advances while `RNGEN` is set and the clocks run, and it picks up where it left
  off. I take the AHB clock to be 240 MHz, as I do for the timers. On the board's hsi48 the first
  four words come 2346 core cycles (about 4.9 us) after `RNGEN`, and each next four 426 cycles
  after the buffer empties.
- The RCC side: `RNGEN`, `RNGRST` and `RNGSEL` in `rcc.rs`, handed to the RNG through a small
  shared handle (`rcc/rng_clock.rs`) that logs each change with its cycle, as the pixel clock
  does. `0x13C` is an alias of `0x0DC` for the AHB2ENR. With the clock off the registers read 0
  and writes are dropped. `RNGSEL` sets the kernel clock: hsi48 48 MHz, lse 32.768 kHz, lsi
  32 kHz.
- The clock error is real: with `CED` clear, choosing lse or lsi under the 7.5 MHz limit sets
  `CECS` and `CEIS` and follows the clock, and `CEIS` stays until it is cleared.
- The interrupt goes through `Irqs` as IRQ 80. The line is a level on the chip but `Irqs` is edge
  like, so I raise it when the level becomes active and when a read of `RNG_DR` or a flag clear
  leaves it active, as the USART does.
- The values come from xoshiro256**, seeded by splitmix64 (`rng/prng.rs`), no crate. By default
  the seed comes from the host (the OS entropy behind the standard library's hash keys), so the
  values differ from run to run. `cmm2_memory_with_rng_seed` fixes it for tests and repeatable
  runs. There is no command line option for it: nothing in the emulator's own runs wants one.
- A seed error cannot happen on this source, but the flags and the recovery sequence are there.
  Only a test can make one.

## What the firmware does with it

- The boot code writes `RCC_D2CCIP2R` (RNGSEL 0, so hsi48), turns on `RNGEN` in `RCC_AHB2ENR`,
  writes `RNG_CR` twice, then sets `RNGEN` and polls `RNG_SR` about 14 times before `DRDY`.
- MMBasic's `RND` reads the hardware RNG: `PRINT RND` reads `RNG_SR` and then `RNG_DR` once, and
  the value follows the seed. Before the model existed it gave a fixed first value.

## What is not modelled

- The analog noise source, the health tests and the conditioning, which are replaced by the
  PRNG, and the real bias or entropy of the chip.
- The conditioning output register that holds four more words behind the output buffer. `DRDY`
  drops after four reads and the next four take the refill time.
- The shorter restart after `RNGEN` is cleared and set again (16 RNG clocks and 216 AHB clocks,
  34.3.8). It takes the refill time.
- pll1_q as a kernel clock choice: PLL1 is not worked out, so I take it as 48 MHz.
- A kernel clock that changes in the middle of a start: the work done so far is kept as cycles.
- A real seed error. The 34.3.7 recovery sequence is only exercised by the tests.
- What an access with the clock off returns beyond "invalid data" (I return 0), and CStop or
  low-power gating (`RNGLPEN`).
