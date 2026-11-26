# How fast it is, where the time goes, and what real speed needs (issues 15, 16 and 74)

## Decision (2026-11-14, issue 74)

I decided to build the x86-64 translation (option B in section 4.10) directly. There are no cheap
interim steps first (sections 4.1 to 4.5), and no block-interpreter stage (option A). The record
is in [decisions/0002-jit-first.md](decisions/0002-jit-first.md). The reasoning:

- The interim steps add up to an estimate of about 2.7 to 2.8 times (section 4.8: the product is
  2.72). Only 4.1 (2.0 times) and 4.2 (9 percent) were measured, on prototypes; 4.3, 4.4 and 4.5 are
  estimates from the profiles. That would take the BASIC interpreter from about 25 million
  instructions a second to about 70, and I put the board at 215 to 270 million (sections 2 and 6, an
  estimate). They cannot reach the board's speed, and I would still have to do B afterwards, so I
  would not spend the weeks on them first.
- Option A is estimated at about 150 million a second (section 4.10), which is also short of it.
- Option B is estimated at 400 million to 1 billion instructions a second, and at 6 to 10 weeks.
  Both are the figures in the table in section 4.10. They are my estimates: section 8 says I did
  not prototype A or B, so neither is a measurement.
- What stays, because it is about correctness and the translation needs it: the memory wait states
  (issue 65), `TIMER` (issue 70, done), the DWT cycle counter and SysTick. Building the frame
  off the CPU thread (section 4.9) is independent and waits for a profile.
- The existing interpreter stays. It is the fallback for hosts the translation does not cover, it
  is what GDB and the trace use, and it is the oracle: the plan is to run both one instruction at
  a time and compare registers, flags and memory.
- The emulator must also build and run on Windows (issue 75). So the choice of code generator has
  to weigh portability: Cranelift targets more hosts than a hand-written x86-64 emitter does. I
  have not chosen between them yet.

The other options in this document (all of section 4, the idle time warp C included) are kept as
the record of what I looked at and why. They are not the plan.

### Which of the speed figures are measurements

- **Measured:** the host's instruction rate, such as 54.5 million a second at the prompt
  (2026-11-13) and the figures in section 2, which carry the plus or minus 20 percent of section 1.
- **Measured:** an empty `FOR` loop of 400,000 iterations took 255.7 ms on the emulator's
  clock, which I read from `TIMER` after the timer fix (I ran it again on 2026-11-14 and got
  255.707). The board took 44.944 ms for 100,000 iterations (my measurement on the
  board, issue 70), so 179.8 ms for 400,000 at the same rate. That is about 1.4 times as long as
  the board on that loop. `TIMER` counts the machine's own clock, so this compares the cycles
  the emulated machine spends with the board's. It is not a measure of how fast the host
  runs the emulation.
- **Estimate:** the board's instruction rate, 215 to 300 million a second (section 2; section 6
  gives 165 to 300 across the mixes). It comes from zmu's cycle estimate and I have not measured it
  on the board.
- **Estimate:** "about four times too slow", which is the measured 54.5 million divided by the
  estimated 220 million. It is an estimate made from instruction counts, not something I
  measured on a program. I have not measured the host's wall time against real time for the empty
  loop, so I do not claim a ratio for it.

### What has changed since the text below was written

The text below is as I wrote it at HEAD 2c084d9, with short notes added in sections 1, 5, 7, 8
and 9. Four things have been overtaken:

- The `Program damaged: symbol table` failure of the game's loader (sections 1 and 7) was a core
  bug: `SMULL` multiplied unsigned. It was fixed in 4f7f7bc (issue 64, closed).
- `TIMER` counts properly now (issues 28 and 70, commit 1905ce2, [timers.md](timers.md)). The
  figures in this document are in instructions and cycles of the machine, so they do not change.
- The last item of section 5 says interrupt entry costs no cycles and tail-chaining is missing.
  The core counts a 12-cycle entry and a 6-cycle tail-chained entry now (da46634 and the notes in
  `crates/zmu_cortex_m/NOTICE.md`; the 6 is ARM's Cortex-M3 and M4 figure, which I have not
  verified for the M7). WFI is still not modelled (issue 72), and the wait states are still
  missing (issue 65).
- The game now loads and draws its sprites and colours (my report). It used to stop at an ARMv7E-M
  DSP instruction the core did not have, at 0x08146b6c: it was UQSUB8, not UQADD8, and the whole
  extension was missing (issue 77, batches 1 to 4 committed). A run of commit aafbce9 (2026-11-18)
  went on for 470 s of host time with no fault. I do not know what ended my own first run. It is
  far too slow to play. See section 6.

## What this spike was

This is a measurement spike. I changed nothing in the repository: everything below was measured in a
scratch copy of HEAD 2c084d9 (release builds, firmware 7.00.00b5, a Ryzen 5 5600X, 6 cores / 12
threads). Where I tried a change it was in that copy, and the number is labelled as a prototype.

The short version:

- Today the emulator runs 3.4 to 11 times slower than the 480 MHz board's cycle estimate, depending
  on what the firmware is doing, and 17 times slower on memory-fill code. The worst cases are the
  BASIC interpreter and CSUB code, because **all code that is not in flash is decoded again on every
  instruction**. The firmware's interpreter loop and the CSUBs run from ITCM, so that is exactly the
  code that matters for the game.
- A decoded-instruction cache for ITCM, as a lazily filled prototype of about 40 lines, made BASIC
  loops 2.0 times faster and double-precision BASIC 2.2 times faster. Nothing else in the current
  interpreter is worth more than about 10 percent each, and together the quick wins reach about 3
  times. Real time needs about 9 times for BASIC, so the quick wins are necessary and not
  sufficient.
- Peripheral models are not a hotspot (below 0.5 percent in every profile). The bus path matters
  only for store-heavy code (about 20 percent of the time in a page-fill benchmark).
- The cycle model is wrong by 6 to 8 times for anything that touches SDRAM: the board takes 7.4
  cycles per pixel to fill a page and the emulator about 1. That is a fidelity gap (issue 16) that
  also changes how many instructions a real-time run has to execute.
- I could not run Galactic Raiders itself. Its loader (a 122 KB BASIC program) failed to run in the
  emulator with `Program damaged: symbol table`, which was a separate bug (section 7; fixed since,
  issue 64).

## 1. How I measured

- `perf` is not usable: `kernel.perf_event_paranoid` is 4 and I did not change system settings.
  There is no valgrind. So I used (all in the copy):
  - a SIGPROF sampler inside the emulator (1 kHz, frame-pointer stack walk, symbolised afterwards
    with `addr2line -i`), started when the guest printed a marker so that boot is not in the
    profile. The numbers are shares of samples;
  - counters: guest PC region per step and how many steps found no decoded instruction, accesses per
    memory region and per peripheral block, and the size of the cache entry;
  - host CPU time per run, from `CLOCK_PROCESS_CPUTIME_ID`, with the guest's instruction count and
    cycle count logged every 5 million instructions and at two markers.
- A workload is typed at the console after the banner. It prints `GO`, runs, and prints `DONE`; the
  markers make the run start and stop at those texts, so first boot, the typing and the idle before
  the program are out of the rate. Programs, all after a first boot that is saved in a flash file:
  - loop: `FOR i=1 TO 100000:a=a+1:NEXT`
  - float: `FOR i=1 TO 20000:x=SIN(i)*SQR(i)+x/1.0001+LOG(i):NEXT`
  - double: `x=1.5:FOR i=1 TO 20000:x=x*1.0001+i/3:NEXT`
  - idle: `PAUSE 1000`, and the idle prompt (the 400 million instructions after the banner)
  - graphics: `MODE 8,12`, then 150 times: `PAGE WRITE`, `CLS`, 20 `BOX` with a random fill,
    `PAGE DISPLAY`. `MODE 8,12` is 640x480 at 12 bits (docs/video.md), the game's mode.
  - CSUB: the game repo's `board/gen/b5_px.bas` (its CSUB fills a 640x480 page in 16, 32 and 64-bit
    stores and with `stm` bursts, and reads one back, timing each with the DWT cycle counter), and a
    loop of 25 calls of that CSUB plus 25 `MEMORY SET SHORT` and `MEMORY COPY` of a page.
- **The machine was shared.** Other work was running on it (load average 40 on 12 threads for most
  of the second half of the session). The first runs were on a quiet machine, and the BASIC loop
  gave 24 to 25 million instructions a second; later the same binary gave 12.5. So I give quiet
  numbers where I have them, say where I do not, and compare changes only between runs made one
  after the other. Treat every absolute rate as about plus or minus 20 percent.
- The guest-side figures (instruction counts, cycles per instruction, emulated time) do not depend
  on the host and are exact.

## 2. Instructions per second today

| Workload | Guest instructions | Million instr/s | Estimated cycles per instr | Emulated time | Host time per emulated second |
|---|---|---|---|---|---|
| Boot to the banner (flash code only) | 411.4 M | 70 to 77 | 1.62 | 1.39 s | 3.8 to 4.3 |
| Idle prompt | 380 M | 49.9 | 2.23 | 1.77 s | 4.3 |
| `PAUSE 1000` | 165.7 M | 48.7 | 2.90 | 1.000 s | 3.4 |
| BASIC `FOR` loop, `a=a+1` | 132.8 M (1,328 per iteration) | 24.1 to 25.2 | 1.80 | 0.498 s | 10.8 |
| Float loop (SIN, SQR, LOG) | 90.3 M (4,515 per iteration) | 23.1 | 1.78 | 0.335 s | 11.6 |
| Double arithmetic loop | 46.2 M (2,310 per iteration) | 12.6 | 1.76 | 0.170 s | 21.6 |
| 640x480 12-bit boxes and page flips | 378 M for 150 frames | 23.1 | 2.67 | 2.11 s | 7.8 |
| CSUB page fills plus `MEMORY SET` and `COPY` | 72.6 M | 15.3 (loaded machine; about 30 quiet, by the loop's ratio) | 1.84 | 0.278 s | 17 (about 8 quiet) |

"Cycles per instruction" is the core's own estimate, which is also what the clock, the timers and
the pacing run from. The idle prompt and the boot are cached flash code and run fastest. Everything
that runs the BASIC interpreter or a CSUB runs from ITCM and is slow. Note the double loop: its code
mix has more 32-bit Thumb instructions, which cost more to decode, so it is the slowest.

The real-time requirement is `480 MHz / cycles per instruction`: 215 to 300 million instructions a
second, so the shortfall is the last column, 3.4 to 22 times, and about 9 for the BASIC interpreter.

## 3. Where the host time goes

Shares of host time (SIGPROF samples). "Fetch" is `fetch_code`, which reads the opcode through the
bus; "decode" includes `instruction_size`.

| | Boot (flash) | `PAUSE` idle (flash) | BASIC loop | Double loop | Boxes + flips | CSUB fills |
|---|---|---|---|---|---|---|
| Fetch + decode | 0 | 0 | 49 (27 + 22) | 54 (31 + 22) | 24 (15 + 9) | 32 (22 + 10) |
| Execute (`execute_internal` and the handlers) | 47 | 53 | 25 | 22 | 41 | 40 |
| SysTick step (`syst_step`) | 3 | 11 | 3 | 2 | 9 | 3 |
| Wake check (`take_due`) | 6 | 5 | 3 | 2 | 3 | 3 |
| Data bus (`read32_data`, `write32_data`, map) | about 8 | 16 | 4 | 3 | 14 | 20 |
| Peripheral models | below 0.5 | below 0.5 | below 0.5 | below 0.5 | below 0.5 | below 0.5 |
| Frame building | not in this profile (section 4.9) | | | | | |

(The columns do not add up to 100: the rest is the step loop, cache lookup and copy, and
bookkeeping, and some of the buckets overlap, for example `in_range` is inside both fetch and the
data bus.)

Counters that explain the table:

- Steps by where the PC is: during boot 410.7 M steps in flash, 0.67 M in ITCM (0x00000000). During
  the BASIC loop 57 percent of steps were in ITCM and all of those were uncached. No step ever ran
  from AXI SRAM, SRAM or SDRAM in any workload I ran, and the CSUB-fill profile also shows uncached
  decoding (22 percent), so the small CSUB of `b5_px.bas` ran from the ITCM range as well. I do not
  know where the game's 46 KB CSUB would be put, so the cache has to cover the RAM regions too. Only
  the flash is pre-decoded.
- Memory map traffic: in the 1.28 G instruction CSUB run, 70 M accesses went through the map (5.5
  percent of instructions) and the peripherals took a few hundred thousand: GPIOC (the SD card's
  bit-banged SPI) and a timer's counter register (0x40014400, 12.8 M reads, the firmware's timing
  loops). Everything else (DTCM and flash) is inside the core.
- A decoded entry (`CachedInstruction`) is 48 bytes, for each halfword of the 2 MB flash: a 48 MB
  table (the process is 86 MB resident). The startup costs 0.14 s CPU, which is fine.

## 4. Hotspots, causes and fixes

Ranked by gain per effort. "Measured" means a prototype in the copy, interleaved with the unchanged
build on the same machine; "estimated" means from the profile only.

### 4.1 Cache decoded instructions of ITCM (and any RAM code): measured 2.0 to 2.2 times on BASIC

Cause: `Processor::step` in `crates/zmu_cortex_m/src/executor/mod.rs` looks the PC up in
`instruction_cache`, which is built once for the flash. Outside it (`None`) it fetches the
halfword(s) through `Bridge` and the `RefCell<MemoryMap>`, decodes with `decode_16`/`decode_32`
(large match tables) and executes. The BASIC interpreter and the CSUBs live in ITCM, so that is the
common case. The decision record's guess (RAM code decoded each step) was right, and it is bigger
than the 32-byte copy it blamed.

Prototype: a second cache `Vec<Option<CachedInstruction>>` of 32,768 entries for 0x00000000 to
0x0000FFFF, filled when an instruction is first executed, thrown away completely whenever anything
writes to ITCM (a counter bumped in `MemoryMap::write_bytes` for region 0). About 40 lines.
Interleaved runs on the same loaded machine, host CPU seconds for the same guest work:

| Workload | unchanged | with the ITCM cache |
|---|---|---|
| BASIC loop (53 M instr) | 4.23 s (12.6 M/s) | 2.11 s (25.2 M/s) |
| BASIC loop, three repeats | 6.4 / 6.4 / 6.6 s | 3.2 / 3.0 / 3.2 s |
| Double loop (23 M instr) | 2.04 s | 0.93 s |

So 2.0 and 2.2 times. The quiet rate for the unchanged build is 24 to 25, so I expect about 50
million a second quiet, but I have not measured that (the machine was never quiet again). That is
about the speed of the flash-only code today (boot: 70 to 77), which is the ceiling of this
interpreter design.

Proper version (effort 1 to 2 days): one cache for every executable region (ITCM, AXI SRAM, SRAM,
SDRAM) in pages of, say, 4 KB, filled on demand; a bitmap of "pages that hold decoded code" checked
on every store (a one-byte table lookup by `addr >> 12`; the game's stores go to frame buffers, not
to its code) so self-modifying code and a CSUB being loaded invalidate just the page. Shrink the
entry at the same time (see 4.5). The generation-counter trick in the prototype is enough for the
firmware as it is today, because it only writes ITCM during boot.

### 4.2 SysTick: count in closed form, not one cycle at a time: measured 9 percent on idle

Cause: `syst_step` in `peripheral/systick.rs` loops `for _ in 0..cycles`, one count per core cycle,
for every instruction. It is 11 percent of the idle profile and 9 of the graphics one.

Prototype: `let d = left.min(cvr); cvr -= d; left -= d` with the reload and the interrupt at zero,
same behaviour. `PAUSE`: 2.65 s to 2.41 s (9 percent); the BASIC loop, no change within noise. About
15 lines. Better, do it only when the counter is read or when its next expiry cycle is reached.

### 4.3 Do the per-step bookkeeping only when something happened: estimated 10 to 15 percent

Cause: every instruction does, in `Cpu::step` (`crates/cmm2-core/src/cpu.rs`): `pc.set`, `clock.set`
(two `Rc<Cell>`), `clock.take_due()` (an `Rc<Cell<Option<u64>>>` load and compare), `irqs.take()`,
`flash_ops.take()` and `take_pending_fault_trap()` (each a `Cell::replace` or an `Option::take`),
and inside the core `dwt_ctrl` and `syst_csr` tests, `pending_exception_count` and `sleeping`. In
the boot profile `step` itself is 22 percent, `take_due` 6, `Option::copied` 4, `Cell::replace` and
`take` about 3.

Fix: let the core run a batch (`run_batch(n)`) with one inner loop that tests a single `next_event`
cycle (the minimum of the next device wake and the next SysTick expiry) and a single dirty flag that
a device sets when it raises an interrupt or queues a flash op, and set `pc` and `clock` only at the
edges of a batch (a device only looks at them from inside a bus access, so set them lazily from
there or before each access that reaches a device). Effort half a day to a day. Do it after 4.1,
because it touches the same loop.

### 4.4 A direct path for RAM in the bus: estimated 15 to 20 percent on store-heavy code

Cause: a data access outside the flash and DTCM goes: `Processor::read32` (`map_address` on the
`Option<MemoryMapConfig>`, `sram.in_range`, `code.in_range`) then `device.in_range` (a
`Box<dyn Bus>` call, which is `!core_register(addr)`, a match of about 20 ranges) then
`device.read32` (another virtual call) then `Bridge`, `Rc<RefCell<MemoryMap>>::borrow_mut`,
`read_bytes::<N>`, `locate` (a linear scan over 7 regions with `checked_sub`; SDRAM is the last one)
and a copy. On a miss it goes through `window()` (a scan of every peripheral window), then the
peripheral trait object, then the trace hook test. In the CSUB-fill profile `read32_data` and
`write32_data` are 20 percent, `locate` 6, `in_range` plus `core_register` 8, `Iterator::try_fold`
6.5 (the scans).

Fix: (a) in the core, test the address's top nibble first so that 0xE0000000 is the only place
`core_register` is asked, and give the bus a `fn ram(&mut self, addr, len) -> Option<*mut [u8]>`
that returns a slice directly; (b) in the map, a 256-entry table by `addr >> 24` (or `>> 20`) giving
the region index, so `locate` is one lookup; (c) keep the `RefCell`, but borrow once per step, not
once per access. A guest store of 32 bits into SDRAM should be about ten instructions on the host.
Effort 1 day. Gain is about 1.15 to 1.2 times on the drawing part of the game, and 5 percent on
BASIC.

### 4.5 Execute by reference and shrink the cache entry: estimated 3 to 5 percent

`instruction_cache.get(pc).copied()` copies a 48-byte enum (`Option::copied` is 1.6 to 3.9 percent
of the profiles) and `execute_internal` is one giant `match` with one indirect jump for every
instruction (21.9 percent of the boot profile is the match line alone: a single, badly predicted
dispatch). Match on a reference, and move to a compact 16-byte entry or to a table of function
pointers (the entry holds the handler and its operands). 1 day, and it is a step towards 5.1.

### 4.6 Build flags: nothing

`lto = "fat"` and `codegen-units = 1`, measured against the default build: BASIC loop 24.7 against
25.3 M/s, `PAUSE` 26.5 against 27.5 (the unchanged cache). No gain; I would not bother. (The release
profile is the default today.)

### 4.7 Not hotspots

Peripheral models, the clock wake (`take_due` is 3 to 6 percent but that is the check, not any
model: no `tick` is ever visible), the NVIC and `check_exceptions` (0.1 percent), the trace hook (it
is a `None` test), `Rc` clones (no clones on the hot path, only derefs). The firmware's PAUSE and
busy-wait loops poll a timer counter, and that is a lot of accesses (12.8 M in one run) but at a few
instructions each it is cheap. So "batching peripheral time" would not help yet.

### 4.8 What the quick wins add up to

4.1 (2.0x) x 4.2 (1.09) x 4.3 (1.12) x 4.4 (1.07 on average, 1.2 on drawing) x 4.5 (1.04) is about
2.7 times (4.1 and 4.2 measured on prototypes, the rest estimated). BASIC would go from 25 to about
70 million a second and idle from 50 to about 130. The requirement is 215 to 270 million, so the
interpreter work gets to about a quarter to a third of real time on the BASIC loader (70 against 215
to 270), and not there on the busy-wait (next sections).

### 4.9 The window path (frame building)

Measured with `SDL_VIDEODRIVER=dummy`, under load, in wall time (so an upper bound): building a
frame (`MemoryMap::frame`, two ARGB4444 layers into RGB) took 2.6 ms, and the whole `Window::update`
call (frame, texture upload, scale, present) 4.5 ms. The redraw interval is 16 ms of wall time, so a
paced run does about 60 of them a second: 16 to 28 percent of the host thread, on the same thread as
the CPU. Fix: composite on a second thread from a copy of the two pages (1.2 MB copy per frame), or
only when the LTDC base address, a layer register or the framebuffer content changed, and upload
12-bit pixels as ARGB4444 textures (SDL supports that format) so that no conversion runs at all.
Effort 1 to 2 days, and it frees 15 to 25 percent of a core, so it is worth it for real time but it
is second order.

### 4.10 Structural steps beyond the interpreter

| Step | What | Expected speed (cycles per instruction on the host: today about 60) | Effort | Risk |
|---|---|---|---|---|
| A. Pre-decoded blocks for ITCM, RAM and flash | decode once into basic blocks of micro-ops, flags computed lazily, direct threaded dispatch, instruction count and cycle count added per block, interrupt and wake checks only at block ends and after a store to a device | 2 to 3 times over 4.8: about 150 million a second | 2 to 4 weeks | the IT block, exceptions between the instructions of a load/store multiple, faults inside a block need care; GDB and the trace need the single-step path kept |
| B. Block translation to x86-64 | same blocks emitted as machine code (cranelift or a small hand-written emitter), the register file in a struct, loads and stores through the fast RAM path with the device slow path as a call | 400 million to 1 billion a second | 6 to 10 weeks | the FPU (fpv5 double with Arm NaN and rounding rules), precise exceptions, self-modifying code, and testing against the interpreter instruction by instruction |
| C. Idle time warp | when the guest only polls (a loop with no stores and no device writes, waiting for the clock) advance the clock to the next device event without executing | removes the busy-wait entirely | days, but fragile | changes what a program sees (poll counts); I would make it opt-in and exact for known loops only |

My reading of "real speed" for Galactic Raiders (section 6) is that the BASIC busy-wait is most of
the instructions, so B or C is what finally gets there; A plus 4.1 to 4.5 gets close for the game's
heavy frames.

## 5. Fidelity gaps that cost speed or accuracy (issue 16, the speed parts)

- **No wait states for memory.** `Cpu::step` charges every load 2 cycles and every store about 1,
  whatever it touches. Measured on the same CSUB, board against emulator (the board numbers are from
  the game repo, `docs/hardware-verification.md`, B5 and B14; the emulator's are the DWT counter and
  `TIMER` read inside the emulator, so they are exact):

  | Operation | Board | Emulator | Ratio |
  |---|---|---|---|
  | Fill a 640x480 page, 32-bit stores | 4.74 ms (2.28 M cycles) | 0.67 ms (322,386 cycles) | 7.1 |
  | Fill, 16-bit stores | 4.74 ms | 1.34 ms (641,587) | 3.5 |
  | Fill, 64-bit stores | 4.74 ms | 0.34 ms (163,202) | 14 |
  | Fill, `stm` bursts | 4.74 ms | 0.60 ms (288,794) | 7.9 |
  | Read a whole page, 32-bit | 7.4 ms | 0.99 ms (474,385) | 7.5 |
  | `MEMORY SET SHORT`, one page | 4.72 ms | 0.589 ms | 8.0 |
  | `MEMORY COPY`, one page | 13.8 ms | 2.40 ms | 5.8 |
  | `PAGE COPY 3 TO 2` | 13.8 ms | 2.39 ms | 5.8 |

  The board's SDRAM costs 7.4 cycles per pixel however it is written (the same 4.74 ms for 16, 32
  and 64-bit stores and `stm`). The model costs a store by its instruction, so wide stores look
  free. To fix it, add a cost by region to the data access: SDRAM about 15 cycles per 32-bit store
  and 7.4 per 16-bit (the board numbers give the rule: roughly 1 SDRAM write per 7.4 cycles whatever
  its width, and bursts do not help), flash fetches, AXI SRAM. All five rows above are checks that
  can be rerun in seconds, so they are the acceptance test. This is cheap for the speed too: code
  that waits on SDRAM burns more emulated cycles per instruction, so fewer instructions have to run
  per real second.
- **WFI does nothing for time.** `exec_wfi` sets `sleeping`, but `Cpu::run_loop` always calls
  `Processor::step`, which keeps executing the next instruction, and no time is skipped to the next
  wake. I did not find the firmware using WFI in the idle paths I profiled (`PAUSE` and the prompt
  poll), so it costs nothing today, but a program or a firmware path that sleeps would burn host
  time.
- Interrupt entry costs no cycles, tail-chaining is missing (already listed in the decision record;
  both are counted now, see the top of this document).

## 6. What "real speed" requires

**The board's instruction rate is unknown.** From the core's estimate the firmware averages 1.6
cycles per instruction at boot, 1.8 in the BASIC interpreter, 2.2 to 2.9 idle, and 2.7 in drawing,
so the board does 165 to 300 million instructions a second depending on the mix: 267 for the BASIC
loop and 215 idle. That is an estimate from the model, and the model is known to be wrong for SDRAM
(section 5). For code that runs in ITCM and DTCM, where nothing waits, I expect the model to be
within a factor of 1.5 (an M7 dual-issues, so it could be faster, but branches and loads in the
interpreter cost more than 1).

**Is pacing to the emulated cycles at 480 MHz right?** Yes for what the guest can see. `TIMER`,
SysTick, the 74.7 Hz LTDC frame interrupt, the USART baud rate and `PAUSE` are all derived from the
same cycle count, so holding that count to the wall clock gives the guest a correct 74.7 ticks a
second, a correct `TIMER`, and correct keyboard and serial timing, whatever the instruction rate. An
instruction rate is not what has to match; the cycles per instruction (and the wait states) are. If
they are wrong, a program that is CPU-bound on the board takes a different share of a tick, and a
game that drops frames on the board might not drop them in the emulator. Pacing is already built
(`--console`, `--window` without `--unpaced`); today it does nothing, because the host is slower
than the clock.

**What it takes for the game to run at real speed.** The game's loader (`grd_bench.bas` and
`gr_demo.bas`, both checked) waits for the tick in a BASIC loop (`DO WHILE tk% = last%: LOOP`), and
the board's CSUB frame (5.9 ms) uses less than half of the 13.4 ms tick. So more than half of the
instructions in a real-time run are the interpreter spinning in that loop, and the emulator has to
execute those at 215 to 270 million a second, like everything else, while building a frame for the
window. Today the interpreter runs at 25. The plan in section 4 gets it to about 70, then to about
150 with A, then past 400 with B. Where the game is actually spending its time on the board (the
clear, the blits, the HUD) is SDRAM-bound, so fixing the wait-state model (section 5) shrinks the
instructions per tick for that part and is the cheapest accuracy fix.

I could not run the game's frame loop (section 7), so I do not have a measured instructions-per-tick
figure for it. From the board's numbers: one tick is 13.4 ms, which is 6.4 M cycles, which at 2
cycles per instruction is 3.2 M instructions per tick, 240 M a second, and the CSUB's own share is
44 percent of the cycles but, being SDRAM-bound, a smaller share of the instructions.

**Measuring the board's rate later.** I would run the same program on the board and in the emulator
and compare the *emulated* clock, which is deterministic, so no host noise is involved:

1. BASIC: the three programs above, `TIMER` before and after, on the board. The emulator gives: loop
   100,000 iterations = 132.8 M instructions, 0.498 s; float 20,000 iterations = 90.3 M, 0.335 s;
   double 20,000 iterations = 46.2 M, 0.170 s. The instruction counts are the same on the board
   because it is the same firmware, so `board_seconds * 480 M / instructions` is the real cycles per
   instruction, directly.
2. CSUB: `b5_px.bas` already reports the DWT cycle counter on the board (1,000-iteration empty loop:
   emulator 4,001 cycles; the five fills; `MEMORY SET`, `MEMORY COPY`, `PAGE COPY`); the board's
   numbers are in the game repo. I would add a CSUB with fixed instruction sequences (an ALU chain,
   a load chain in DTCM, ITCM, AXI SRAM and SDRAM, taken and not-taken branches, a multiply and a
   double-precision FPU chain) of 1 million iterations each, read with DWT, and run it in both. The
   per-class cycles per instruction replace the guessed table in the core.
3. The game: `grd_bench.bas` reports `call_ms`, `logic_ms` and `draw_ms` per frame, with the DWT
   counts, on the board (5.87 ms, 0.002 ms, 5.08 ms). Once the loader runs in the emulator, the same
   three numbers from the emulator against those are the end-to-end check.

## 7. What I found that blocks measuring the game

This is fixed: it was `SMULL` multiplying unsigned (issue 64, 4f7f7bc). The text is what I knew
at the time.

`RUN "grd_bench.bas"` (I built four trimmed variants of the generated 122 KB file, all with the
loader and the CSUB, none running the 10 s heartbeat) prints nothing and returns to the prompt, and
the error appears later as `Program damaged: symbol table`. A bisection of the file by line count:
the first 160 lines run, the first 200 do not (7.5 KB of file). A 118 KB file of comment lines runs,
and so does a 8 KB file of other BASIC, so it is not the size of the file, and it is not a CSUB
problem (the file without its CSUB fails the same way). I did not get further. It may be the symbol
table the firmware builds for the program (constants, subs, variables) being checked against memory
that the emulator does not give it, or the flash programming of the saved program. This is a
separate issue, and is the thing to fix before the game can run; it is also why the numbers above
come from `b5_px.bas` and my own programs, not the game.

## 8. What I could not measure

- The game. Section 7 (the loader bug is fixed now, issue 64; the game runs, see the top).
- Quiet-machine absolute numbers for the prototype (the machine was loaded for most of the session).
  The ratios between runs made one after the other are good; the absolute millions per second are
  not, and the quiet ones for the unchanged build are from before the load began.
- The frame-building cost on a real display (I used SDL's dummy driver and wall time under load).
- The board's real rates (sections 5 and 6). Everything about the board here comes from the game
  repo's recorded measurements.
- An A or B prototype. The estimates for them are from the cost per instruction I measured (13 ns
  per instruction for cached flash code, about 60 host cycles) and from what such designs usually
  reach.
- Whether the firmware uses WFI anywhere.

## 9. How to repeat it

```
CMM2_FW=firmware/v7.00.00b5/bin/CMM2_MMBasic_7.00.00b5.bin
# a flash file that has been through first boot (answer the keyboard, time and date questions)
cmm2-emulator --firmware $CMM2_FW --console --unpaced --flash b.flash --run 600000000 \
  --on "Keyboard type" "1\r" --on "HH:MM:SS" "12:00:00\r" --on "DD/MM/YY" "01/01/26\r"
# a workload: type it after the banner
cmm2-emulator --firmware $CMM2_FW --console --unpaced --flash b.flash --run 3000000000 \
  --on "Allardice" 'PRINT "G"+"O":FOR i=1 TO 100000:a=a+1:NEXT:PRINT TIMER\r'
```

The markers, the periodic log, the sampler and the counters were not committed: they are a few dozen
lines in `main.rs`, `script.rs` and the core as they were at 2c084d9 (`main.rs` is a short entry
point over modules since the file split), and I can put the useful ones (an instruction and cycle
log, and a stop-on-text) behind a flag if I want to keep measuring.
