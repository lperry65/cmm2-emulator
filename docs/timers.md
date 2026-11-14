# The timers

This is the model of the STM32H743 general-purpose timers the machine maps, TIM5 and TIM16,
with their update interrupt and an event scheduler (issues 28 and 70). The sources are ST's
RM0433 Rev 9, chapters 39 (TIM2 to TIM5) and 41 (TIM15 to TIM17), and the interrupt table in
section 19, which I read from `reference/manuals/rm0433.pdf`. Nothing here is copied from it.

## Why it exists

MMBasic's `TIMER` (function at 0x080685D0) is `(N * 50000 + TIM16_CNT) / 1000.0`. N is a 64-bit
count that the TIM16 update interrupt handler (IRQ 117, handler 0xF79 in ITCM) adds one to.
The firmware runs TIM16 at 1 MHz with ARR 49999, so it wraps every 50 ms.

The timer model only counted. A wrap set `SR.UIF` and nothing raised the interrupt, so N stayed
0 and `TIMER` wrapped every 50 ms: `PAUSE 100` printed 0.01 where the board prints 100.013, and
`TIMER` went backwards over any longer interval. TIM5's overflow interrupt was missing the same
way (every 17.9 s).

## What the chip does

- A timer counts prescaler ticks: the kernel clock divided by PSC + 1. On the board that is the
  240 MHz timer clock, half the 480 MHz core. The counter counts from 0 to ARR and wraps, which
  is an overflow (39.3.1, 39.3.2, 41.4.1, 41.4.2). TIM5 can count down as well, from ARR to 0,
  with an underflow back to ARR (DIR in CR1).
- An overflow or underflow is an update event (UEV) unless `CR1.UDIS` is set. A UEV loads the
  buffers (ARR into the active reload, PSC into the prescaler) and sets `SR.UIF`.
- ARR is buffered only when `CR1.ARPE` is set; with ARPE clear a write to it is active at once.
  PSC is always buffered: a write waits for the next UEV.
- `EGR.UG` starts the counter and the prescaler again, and with UDIS clear it is a UEV too. It
  sets UIF only when `CR1.URS` is clear. With UDIS set it restarts the count and loads nothing.
  Counting down it restarts from ARR.
- `CR1.OPM` stops the counter (clears CEN) at the next UEV.
- The interrupt is the level of `SR.UIF` and `DIER.UIE`. Software clears UIF by writing 0 to it
  (writing 1 does nothing, so the HAL writes `~UIF`).
- The update interrupt shares the timer's global line on TIM5 and TIM16. From the NVIC table
  (Table 147 in this revision of the manual, and the SVD agrees): TIM5 is IRQ 50 (`tim5_gbl_it`)
  and TIM16 is IRQ 117 (`tim16_gbl_it`). TIM16 has its own line on the H743; there is no
  TIM1_UP_TIM16 sharing as on other STM32 parts. The other update lines, for when those timers
  are mapped: TIM1_UP 25, TIM2 28, TIM3 29, TIM4 30, TIM15 116, TIM17 118.

## What the emulator models

The model is `crates/cmm2-core/src/timer.rs`, with its tests in `timer/tests.rs`. It is wired
for both timers in `machine.rs`, from a `TimerSpec` (base, width, line, whether it counts down).

- Up counting for both, down counting for TIM5 (DIR), a 16-bit or 32-bit wrap, and the SVD reset
  values. CR1 keeps only the bits that exist: TIM16 has no DIR or CMS.
- The ARR and PSC buffers as above, UG with URS and UDIS, UDIS on overflow, and OPM.
- The interrupt through `Irqs`: IRQ 117 for TIM16 and IRQ 50 for TIM5, from `TIM16_SPEC` and
  `TIM5_SPEC`. The line is a level on the chip but `Irqs` is edge like, so I raise it when the
  level becomes active (UIF rising with UIE set, or UIE set with UIF up), and again when a write
  to `SR` leaves it active. That is how the USART and the RNG do it. A handler that returns
  with UIF still set is not entered again by itself.
- The event scheduler. The count is brought up to date from the core cycle count whenever a
  register is touched, but nothing touches a timer during a long BASIC loop, so the timer asks
  `Clock::wake_at` for a wake at its next overflow, `anchor + (period - count) * divisor` core
  cycles (counting down: `count + 1` ticks). The CPU ticks the timer when the cycle comes, the
  tick catches up and raises the line, and asks for the next wake. It asks again after every
  write and in every tick. It asks only when an overflow would raise the interrupt: CEN and UIE
  set, UDIS and UIF clear. For TIM16 that is a wake about every 24 million core cycles (50 ms).
  An earlier wake that is no longer right is left alone: the tick it gives does nothing and
  asks again.
- Many wraps between two looks are done in one step, and the buffers are applied at the first.

## How TIMER is built on this firmware

At boot the firmware runs `HAL_TIM_Base_Init` on TIM16 twice, once with PSC 0 and then with the
real value. A trace of the writes shows, at both boots: ARR 0xC34F (49999), PSC, RCR 0, CR1 =
URS, EGR = UG (which loads the buffers and, with URS set, leaves UIF clear), CR1 = 0, then
`DIER.UIE` and `CR1.CEN`. The UG after each PSC write loads it, so the timer never runs with a
stale prescaler. PSC is 0xFB (251) in the first, setup boot, which assumes a 252 MHz timer
clock, and 0xEF (239) after the reset. TIM5 gets ARR 0xFFFFFFFF, PSC 0, URS, UG, ARPE, UIE, then
CEN, and the firmware then writes CNT to 0. The model keeps the buffering rule the manual gives
(PSC and ARPE-set ARR wait for an update event).

The interrupt handlers in ITCM both call the HAL update handler (0x9650), which clears UIF by
writing `~UIF` to `SR` and runs the period callback, and then add to a 64-bit counter. TIM16's
(0xF79) adds one to two 64-bit counters. N, the one `TIMER` reads, is at 0x20013430: `TIMER`
(0x080685D0) re-reads it with `ldrd` in a loop until it is stable, and reads `TIM16_CNT` through
the handle at 0x20015840. 0x2000B7E8 is `TIMER`'s float result slot, not N. `TIMER` is N times
50000 plus `TIM16_CNT`, in microseconds, divided by 1000.

TIM5's handler (IRQ 50, 0xFB1) calls the HAL handler and then adds one to the high word of a
64-bit counter at 0x20013420 (the high word is at 0x20013424). That is an overflow extension
of TIM5 into 64 bits, and it is the
right thing to do on each overflow. TIM5 now overflows every 17.9 s of machine time. I did not
trace what reads that counter, so nothing visible changes from it yet.

Which timers the firmware starts at boot: TIM2, TIM3, TIM4 and TIM8 are configured (ARR, PSC,
UG) and left stopped; TIM3 and TIM8 also get compare set up (PWM, not modelled). I traced every
write to a timer's DIER (every timer window) across both boots to the second prompt. The only
ones are TIM5 and TIM16, each set to 1 (UIE), at both boots, and both are wired. I also read
the NVIC enable registers at the second prompt: the lines on include TIM2 (28), TIM4 (30),
TIM5 (50), TIM6 (54) and TIM16 (117). TIM2, TIM4 and TIM6 have their NVIC line on but never get
an interrupt enable, so they never raise one. Whether the SPI3, USART or GPIO code expects a
timer interrupt I only checked this way: I did not trace their code.

Measured on the emulator with the real firmware: `PAUSE 100` gives 100.006 (the board 100.013),
`PAUSE 60` gives 60.005, and three `TIMER` reads in one statement are 6 to 7 us apart (the board 4
to 7 us). The emulated machine takes longer than the board on the empty 100,000 iteration FOR loop
(about 64 ms against 44.944 ms on the board; a later run of 400,000 iterations took 255.7 ms, the
same rate). That is the cycles the model charges for the loop, and not the timer. I have not traced
why it is more than the board's; the cycle model's gaps are tracked in issues 16 and 65.

## What is not modelled

- Capture/compare, PWM and DMA requests. Those registers (CCMR, CCER, CCRx, the DMA bits of DIER and
  the other SR flags) are plain stored registers, and the design leaves room: another flag raised in
  `catch_up` and another term in `interrupt_active`.
- Center-aligned counting (CMS). The bits are stored, but counting follows DIR. The manual does not
  make clear on which edges the update flag is set for each mode (39.3.2).
- The repetition counter of TIM16 (RCR, 41.4.3): it is stored, and I treat it as 0.
- UIFREMAP (UIF in bit 31 of CNT), the slave-mode controller and external clocks, the master mode
  outputs, and the break and dead-time features.
- A count above ARR. The chip counts on to the end of the width before it wraps; I wrap at the next
  look. The firmware never does it.
- CKD, which only matters for the filters; it is stored.
- One-pulse mode with a down count stops with the count at ARR; the manual does not say.
- The timer clock is fixed at 240 MHz. It does not follow the RCC (issue 76, open). In the first
  (setup) boot the firmware writes TIM16 PSC 251, which assumes a 252 MHz timer clock, and only
  after the reset writes 239. With 240 MHz fixed, `TIMER` runs about 4.8% slow in the setup boot.

## RM0433 sections used

39.3.1, 39.3.2, 39.4.1 to 39.4.6 (TIM2 to TIM5), 41.4.1, 41.4.2, 41.6.1 to 41.6.5 (TIM16), and
the NVIC table (Table 147) for the lines.
