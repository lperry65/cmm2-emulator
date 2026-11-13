# What the firmware does at boot

This is the real firmware (v7.00.00b5) running on the emulator with every peripheral register
served by the plain stub: a register reads back what was last written, or its reset value from
the SVD if it has not been written (0 where the SVD names no register). Nothing is modelled
yet, so the run shows what the firmware asks of the hardware and where it first gets stuck.
Measured with the register reset values in place, over 3 million instructions, with:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --run 3000000 --trace trace.txt --trace-mmio
```

## Where it blocks

**At about instruction 225,585 the firmware polls `PWR_CSR1` and does not get out within the run.**

The loop is at `0x08005BFC`: `ldr r3, [r4, #4]`, `lsl r3, r3, #18`, `bpl`, then a read of the tick
counter and a compare against 1000. Shifting left by 18 moves bit 13 into the sign bit, so it waits
for bit 13 of `PWR_CSR1` to be set. That is `ACTVOSRDY`, the flag that says the voltage scaling has
settled. The firmware had just read `PWR_CR3` (its reset value `0x6`) and written `0x2` to it. The
stub reads back the reset value `0x4000` every time, bit 13 stays clear, and the loop runs for the
rest of the 3 million instructions: 273,329 reads in all.

The loop gives up after 1000 ticks of the counter the SysTick handler advances, which at about
29,000 instructions a tick is some 29 million instructions. A run of 60 million ended at
`0x0809953E`, inside the loop at `0x0809953C` that waits for bit 13 of `PWR_D3CR` the same way
(`VOSRDY`). Its reset value is `0x4000`, so it stalls there too. I have not traced that far.

So nothing before this point needs a modelled peripheral, and this is the first thing that does.

**Later, with the power controller modelled:** both power waits pass. The 3 million instruction run
then ends in a bounded poll of `RCC_CR` bit 17 (`HSERDY`, which gives up after 100 ticks), and
after 60 million instructions the firmware is polling `RTC_ISR` (`0x5800400C`), which reads its
reset value `0x0003FF5F` every time: 5,026,089 reads. The clock controller and the RTC are the next
two models.

**Then, with the clock controller modelled as well:** the clock waits pass (`RCC_CR` is read 37
times in the 3 million instruction run, not hundreds of thousands). The 60 million instruction run
spends its end polling `RTC_ISR` bit 5 (`RSF`, registers synchronised) in a loop at `0x08007AEC`,
5.4 million reads, with a 1000 tick timeout. The tick is advancing: with the PLL configured the
firmware sets SysTick for 1 ms at 480 MHz, so a tick now takes about 232,000 emulated instructions
(it was about 29,000 at the start-up clock), and the timeout would need about 232 million
instructions to fire. The RTC model is next.

**Then, with the RTC modelled:** both RTC waits pass (22 accesses in the 3 million instruction
run). The firmware now ends in a tight poll of a RAM byte at `0x08094DD6`: `cbz`, `ldrb`, `and`,
`cmp`, `beq`, about 228,000 instructions in and for the rest of the 250 million instruction run,
touching no peripheral. An earlier spike identified the same loop as the flash sector erase wait:
the firmware starts an erase of the option or program sector and waits for the flash interrupt
handler to set the byte. The flash controller is the next model.

**Then, with the flash controller modelled:** the erase completes. The firmware unlocks bank 2
with the two keys, erases sector 7 (`0x081E0000`, the saved options) and sector 1 (`0x08120000`, the
program area), then programs both with stores, setting and clearing `PG` around each 32 byte word
(31 programming sequences on sector 7 and 4 on sector 1 in the first 3 million instructions). The
flash interrupt handler runs once per erase. The run then settles in a microsecond delay routine at
`0x0809ADE2` (`ldr` of `TIM16_CNT`, `sub`, `uxth`, `cmp #1000`, `bcc`), waiting for the 16 bit
difference to reach 1000: 49.6 million reads of `TIM16_CNT` in a 250 million instruction run. It also
polls `RNG_SR` (44,757 reads). One read goes outside the flash, 2 bytes at `0x08200000` from
`0x0800419E`, and the machine logs it as unmapped. The next models are a counting `TIM16` (and
`TIM5`, the free-running counter) and the RNG.

**Then, with the counting timers modelled:** the delay loops complete, and the firmware's longer
behaviour shows. The loop at `0x0809DE8A` reads `GPIOB_IDR` bit 13 (pin PB13, `HAL_GPIO_ReadPin`
on `GPIOB`, `0x2000`); while it reads low the firmware waits 300 ms (about 94 million emulated
instructions, since a millisecond is 1000 timer ticks of 504 core cycles) and tries again, up to
three times, the first at about 1.75 million instructions and the retries at about 96 and 191
million. The stub reads the pin low every time. At about 288.8 million instructions, soon after the
third retry, the core takes an undefined instruction hard fault with `pc=0x40017414`, an address in
the peripheral window, while the SysTick handler is active, and the run ends in the default
fault handler. I have not yet found how execution got there. One guess is that a pin that stays low
sends the firmware down a path such as entering the ROM bootloader, which cannot run here (the
board has a programming jumper); another is a core bug. The first thing to try is a `GPIOB_IDR`
that reads the way the board does, so the next model is the GPIO ports, with the input levels the
board really has.

**Resolved: the GPIO ports, and a core bug.** The pin is GPIOB pin 13, set up as an input with a
pull-up (`GPIOB_PUPDR` `0x04100000`, `GPIOB_MODER` `0xA0BABABE`) just before the retry loop.
Undriven it reads high, so with the GPIO ports modelled (`docs/hardware-map.md` has the board's pin
levels) the loop exits at once and the 300 ms retries are gone. The hard fault was not about that pin,
and it was not a peripheral at all. It is a bug in the zmu core. When SysTick arrived right after an
`ite` instruction, the core neither saved the Thumb IT state in the stacked xPSR nor cleared it for the
handler, so the handler's first instruction (`push {R3, LR}` in the vector stub at `0x2B8`) was skipped as
the else part of the interrupted block while its `pop` ran, loaded a bogus return address, and execution
ran off into peripheral memory. With the long waits gone it shows up at about 6 million instructions
(it was the same bug, which just needed a SysTick at the wrong instruction, at 288.8 million before).
The core now saves the IT state on exception entry and restores it on return, as the architecture says.

With both in, the 400 million instruction run has no fault. It ends in the UART wait-on-flag helper at
`0x08009D2A` (it tests `flags wanted AND NOT ISR` and loops), called from `0x08009DE2` with the
`TEACK` bit (bit 21) of `USART1_ISR` as the flag and a timeout of about 33 million ticks. `USART1_ISR`
reads `0xC0` (`TXE` and `TC`), where the real board reads `0x08E210D0` with `TEACK` and `REACK` set:
36 million reads of it in the run. USART1 is the next model.

**Then, with USART1 modelled:** the `TEACK` and `REACK` wait passes at once (`USART1_ISR` is read 4
times) and the firmware reaches its first-boot setup. Our flash starts erased, like a freshly
flashed board, so it prints "Initial setup - all parameters can be changed later" and "Keyboard type:
1=UK, 2=US, 3=DE, 4=FR, 5=ES, 6=BE ?" and waits for a key. The wait is a loop at `0x0809CB5A` to
`0x0809CB66` around the get-character routine at `0x0809A894`, which returns -1 while nothing is queued;
it accepts `1` to `6`. Each character is printed to the video (`0x0807A8E0`) and, when bit 0 of the byte at
`0x20000ABC` is set (it reads 3), to the serial port: the routine at `0x08098DC8` puts the byte in a ring
buffer and sets `TCIE` in `USART1_CR1`, and the USART1 interrupt handler writes `USART1_TDR`. No USART
interrupt is raised yet, so nothing reaches `TDR` and the console stays empty. The firmware toggles
`RXNEIE` in `CR1` as it polls (9 million `CR1` accesses in 400 million instructions). The next step is the
USART interrupts (transmit empty, transmission complete, receive) and a way for the host to type.

**Then, with the USART interrupt raised:** the serial driver works, and the firmware's own start-up text
comes out of the emulated chip. About 196.5 million instructions in (roughly 10 seconds of wall time
today, most of it spent in millisecond delays and polling) it sends, with the terminal escape codes
`ESC[?25h ESC[37m ESC[m` first:

```
504MHz Colour Maximite 2
MMBasic Version 7.00.00b5
Copyright 2011-2026 Geoff Graham
Copyright 2016-2026 Peter Mather
Copyright 2025-2026 Gerry Allardice

Initial setup - all parameters can be changed later
Keyboard type: 1=UK, 2=US, 3=DE , 4=FR, 5=ES , 6=BE ?
```

and waits for a key. A `1` pushed into the console is echoed and answered with
`Enter time (24hr clock) HH:MM:SS ? `, the next first-boot question. Two lines differ from the real
board, whose `OPTION LIST` begins `480MHz Colour Maximite 2 G2`: the firmware here reports 504 MHz and
no `G2`. How it works out the clock and the board generation is the next thing to find out.

**The first-boot dialogue, driven from scripted input** (`--console --on "Keyboard type" "1" --on
"HH:MM:SS" "12:00:00\r" --on "DD/MM/YY" "12/11/26\r"`; the whole run to the date prompt is about four
seconds in a release build): the firmware asks, in order, `Keyboard type: 1=UK, 2=US, 3=DE , 4=FR, 5=ES ,
6=BE ?`, `Enter time (24hr clock) HH:MM:SS ?` and `Enter date DD/MM/YY ?` (the date field takes 8
characters, so `12/11/2026` is cut to `12/11/20`). After the date it prints nothing more, through three
billion instructions, with no fault and no `>` prompt: it accepts the date and then waits for something the
emulator does not provide yet. The time and date are probably written to the clock chip, which the board has
as a DS3231 on an I2C bus, so I2C is the first suspect.

**Corrected: the firmware reboots itself.** I2C was the wrong guess. A trace taken from instruction 230
million (the new `--trace-from` option, with the scripted dialogue) shows only a two instruction loop,
`nop` and `b .` at `0x0809AE34`, and the code just before it is the standard `NVIC_SystemReset()`: `dsb`, read
`SCB->AIRCR`, keep `PRIGROUP`, or in `0x05FA0004` (the key and `SYSRESETREQ`), write it back, `dsb`, then wait
in `nop` for the reset. After the date prompt the firmware saves its options and asks the chip to reset, so it
restarts with the options it just stored. The core stores the `AIRCR` write and does nothing with
`SYSRESETREQ`, so the emulated chip never restarts. A reset has to restart the CPU from the vector table and
reset the peripherals, but not the flash, the RAM, the RTC or the backup domain, and the reset cause the
firmware reads from `RCC_RSR` (`PINRSTF`, `PORRSTF`, `SFTRSTF`) has to say what happened.

**With the system reset: the BASIC prompt.** The core now flags `SYSRESETREQ`, and the CPU wrapper restarts the
machine: the peripherals go back to their reset values (the RTC and `RCC_BDCR` do not, they are in the backup
domain), pending interrupts and queued flash operations are dropped, the interrupt controller and SysTick
start clean, and the core restarts from the vector table. Flash and RAM keep their contents, so the options
the firmware just saved are still there. `RCC_RSR` reads the power-on flags at first and `SFTRSTF | PINRSTF`
after the reset. The firmware boots a second time with no questions and prints:

```
480MHz Colour Maximite 2 G2
MMBasic Version 7.00.00b5
Copyright 2011-2026 Geoff Graham
Copyright 2016-2026 Peter Mather
Copyright 2025-2026 Gerry Allardice

>
```

That first line is exactly what the real board's `OPTION LIST` begins with. The first boot's "504MHz Colour
Maximite 2" was just the factory defaults before the options were saved, not a difference in the emulated
hardware. The whole path, from power on to the prompt with the scripted answers, is about 600 million
instructions: the first boot reaches the keyboard question at about 198 million, the date prompt at about 200
million, the reset at about 201 million, and the second boot reaches the prompt at about 612 million. In a
release build that is 20 to 30 seconds. After the prompt the firmware sits in an idle polling loop.

## What happens before it

| Instruction | What |
|---|---|
| 1 to 40 | The reset handler resets the clock tree: sets `HSION` in `RCC_CR` (the first read returns the reset value `0x83`, which already has it), then writes 0 to `RCC_CFGR`, the three domain clock configuration registers, the PLL source and configuration registers, the three PLL divider and fraction registers and `RCC_CIER`. It reads `DBGMCU_IDC` and writes `AXI_AXI_TARG7_FN_MOD` (the AXI interconnect), then falls into the C startup. |
| 41 to 211,595 | The C startup copies initialised data and code (including the ITCM code) and clears the BSS. No peripheral access at all, about 211,000 instructions. |
| 211,596 to 211,650 | Cache setup through the Cortex-M7 cache maintenance registers, which the SVD does not describe: write `0xE000EF50` (instruction cache invalidate), write `0xE000ED84` and read `0xE000ED80` (the cache size selection and size ID registers), write `0xE000EF60` (data cache invalidate by set and way). |
| 211,648 to 211,669 | Reads `RCC_CFGR`, `RCC_CR` and `RCC_D1CFGR`: a check of the current clock. |
| 211,752 to 212,300 | Turns on the `SYSCFG` clock (`RCC_APB4ENR` bit 1) and the `GPIOA` clock (`RCC_AHB4ENR` bit 0), then sets pull-downs on `GPIOA` pins 11 and 12 (those pins are the USB data lines on this chip). `GPIOA_PUPDR` reads `0x12100000` and is written as `0x12900000`, then again unchanged, because the reset value already has pin 12. `GPIOA_MODER` reads `0xABFFFFFF` and is written as `0xAB3FFFFF`, then `0xA83FFFFF`. |
| 212,291 to 212,427 | Turns on the `GPIOF` clock (`RCC_AHB4ENR` to `0x21`), sets a pull-up on `GPIOF` pin 9 (`GPIOF_PUPDR` from `0x12100000` to `0x12140000`) and makes the pin an input (`GPIOF_MODER` from `0xABFFFFFF` to `0xABF3FFFF`). |
| 212,439 to 224,427 | Reads `GPIOF_IDR` exactly 1000 times. It is a fixed-length loop, so the 0 it reads gets through it. |
| 224,547 to 224,559 | Configures `GPIOE`: reads `GPIOE_PUPDR` (`0x12100000`) and `GPIOE_MODER` (`0xABFFFFFF`) and writes each back unchanged; pin 9 is already in analog mode in the reset value. |
| 225,573 to 225,579 | Reads `PWR_CR3` twice (`0x6`) and writes `0x2` to it. |
| 225,585 on | The `PWR_CSR1` poll. |

## SysTick already works

Every 29,000 instructions or so the poll is interrupted by a short burst that reads `GPIOH_IDR`
and then `GPIOD_IDR`, 95 times each in the 3 million instructions. That is the SysTick handler:
vector 15 in the table points at `0x2B9`, the handler starts at `0x2B8` in ITCM, it starts exactly 95
times in the run (the first at instruction 242,500), and each start is followed by the two port
reads, through a short chain that ends in flash at `0x0809F3A8`. The core delivers it with nothing
added by the machine: the firmware started SysTick during startup and zmu runs the timer and its
interrupt.

## The 33 distinct registers touched, in first-touch order

`RCC_CR`, `RCC_CFGR`, `RCC_D1CFGR`, `RCC_D2CFGR`, `RCC_D3CFGR`, `RCC_PLLCKSELR`, `RCC_PLLCFGR`,
`RCC_PLL1DIVR`, `RCC_PLL1FRACR`, `RCC_PLL2DIVR`, `RCC_PLL2FRACR`, `RCC_PLL3DIVR`, `RCC_PLL3FRACR`,
`RCC_CIER`, `DBGMCU_IDC`, `AXI_AXI_TARG7_FN_MOD`, four cache-maintenance addresses not in the SVD
(`0xE000EF50`, `0xE000ED84`, `0xE000ED80`, `0xE000EF60`), `RCC_APB4ENR`, `RCC_AHB4ENR`,
`GPIOA_PUPDR`, `GPIOA_MODER`, `GPIOF_PUPDR`, `GPIOF_MODER`, `GPIOF_IDR`, `GPIOE_PUPDR`,
`GPIOE_MODER`, `PWR_CR3`, `PWR_CSR1`, `GPIOH_IDR`, `GPIOD_IDR`.

All accesses were 32 bits wide.

## What to expect next

I have not run past the `PWR_CSR1` and `PWR_D3CR` waits here. An earlier throwaway spike on a different CPU core
got further by supplying fake values, and it stopped at these points, in this order (addresses
are the polling loops; to be confirmed on this emulator):

| Wait | Needs | Loop at |
|---|---|---|
| `PWR_CR2` backup regulator ready (bit 16) | the bit set | `0x08005C2E` |
| `RCC_CSR` low-speed internal oscillator ready (bit 1) | the bit set | `0x080060D2` |
| `RCC_CR` ready bits follow the matching on bits, and clear again when switched off | each `*RDY` bit tracks its `*ON` bit | `0x080061D8` |
| `RTC_ISR` initialisation flags | `INITF` follows the written `INIT`; the flags are write-zero-to-clear, so a plain copy of the written value is wrong | `0x08007AEE`, `0x08007E94` |
| Flash sector erase | the controller completes the erase, sets the end-of-operation flag, raises flash interrupt 4 and clears `START` | wait on a flag the interrupt sets, `0x08094DD6` |
| `RNG_SR` data ready | bit 0 set, a new word per read | `0x08007A48` |
| `USART1_ISR` | transmit empty and complete, and the transmit and receive enable acknowledge bits (`0x006000C0`) | `0x08009D3A` |

After those, the firmware reaches its idle loops near `0x0807A7EC` and `0x0809A352`, polling `GPIOH`,
`GPIOD` and `TIM5_CNT` and toggling `USART1_CR1.RXNEIE`, tens of millions of times. No text has
appeared on `USART1_TDR` at that point. The first flash erases are of `0x081E0000` (saved options)
and `0x08120000` (the program area).

## What this means for the next iteration

- The boot is steady and cheap until the first wait: about 225,000 instructions, 211,000 of them
  plain data copying.
- Each wait above wants a small real peripheral model, not a canned number: the power controller,
  the reset and clock controller, the RTC, the RNG, the flash controller (with its interrupt) and
  USART1. USART1 is also where the console comes in.
- The cache maintenance registers need to answer sensibly (size ID reads), and the SVD does not name them.
- Polling reads of `GPIOH_IDR`, `GPIOD_IDR` and `TIM5_CNT` are going to dominate the idle
  loops, so the cost of a peripheral read matters (see the speed ticket).

## The LTDC line interrupt (issue 35)

With the counting RTC the firmware reaches the second-boot `>` prompt (about 612 million
instructions) and reads typed keys into its ring buffer, but did not run them. The main thread
looped at `0x0809A508` to `0x0809A50C`, storing 0 to the byte at `0x20015584` and spinning
until it was non-zero. I had guessed that this was the LTDC and have now confirmed it from the
code and a trace of the LTDC registers (the firmware's display code runs from ITCM, which a
disassembly of the flash image alone does not show; I dumped ITCM after the C startup):

- Vector 88 (the LTDC global interrupt) is `0xF35`, in ITCM, which loads `0x20011E0C` into r0
  and jumps to `0x4354`. That is the vendor HAL's LTDC interrupt handler. The object at
  `0x20011E0C` is the HAL handle; its first word is `0x50001000`, the LTDC base.
- The handler reads `LTDC_ISR` and `LTDC_IER`. For each of transfer error, FIFO underrun,
  line and register reload it checks that the flag and its enable are both set, clears the
  enable in `IER`, writes the flag to `ICR` and calls the callback. The line callback is
  reached through `0x93D0`, which is a jump to `0x08097DD4`. The register reload callback is
  empty.
- `0x08097DD4` sets the byte at `0x20015584` to 1 (the flag the main thread waits on) and then
  programs the next line event: it loads `AccumulatedActiveH + 1` (`0x270`, 624) from the
  handle and jumps to the HAL's program-line-event function at `0x4570`, which clears
  `IER.LIE`, writes `LTDC_LIPCR` and sets `LIE` again.

So the wait is a wait for vertical blank, once per frame, after the active area. The firmware
programs it during start-up like this (the writes in order):

| Register | Value | |
|---|---|---|
| `LTDC_GCR` | `0xC0002220` | polarities set, `LTDCEN` clear |
| `LTDC_SSCR`, `BPCR`, `AWCR`, `TWCR` | `0x004F0002`, `0x00EF0017`, `0x040F026F`, `0x041F0270` | 1056 x 625 total, 800 x 600 active |
| `LTDC_IER` | `0x6` | FIFO underrun and transfer error |
| `LTDC_GCR` | `0xC0002221` | `LTDCEN` set: the frame starts |
| `LTDC_L1*` | | layer 1, L8, 800 x 600, framebuffer `0x24000000`, pitch `0x320` |
| `LTDC_L1CLUTWR` | 256 writes | the colour table |
| `LTDC_L1CR`, `LTDC_SRCR` | `0x11`, `0x1` | colour table on, reload (`IMR`) |
| `LTDC_LIPCR`, `LTDC_IER` | `0x270`, `0x7` | line 624, line interrupt on |

It never reads `CDSR` or `CPSR`, and never waits on `SRCR` clearing.

The model (`ltdc.rs`) works the frame out from `TWCR` and the 49.5 MHz pixel clock: 1056 x 625
pixels is 6.4 million core cycles, which is 75 Hz, and line 624 is reached at 6,389,760 cycles
into the frame. `ISR.LIF` is set then, whether or not `IER.LIE` is, and with `LIE` set the
interrupt is raised. Because the firmware does not touch the LTDC while it waits, the model asks
the clock for a tick at that cycle (`Clock::wake_at`, `Peripheral::tick`), which is the first
interrupt that comes from the passing of time alone. A reload through `SRCR`
completes at once and sets `ISR.RRIF`.

With that the main thread leaves the wait and the firmware takes the line, but it stopped at
`0x08062AFA` with an undefined instruction: `orn r3, r5, #127`. The vendored core decoded
ORN with an immediate as undefined, and the tokeniser uses it. That is fixed in `zmu_cortex_m`
(see its `NOTICE.md`). Then, from the second prompt at 612 million instructions:

| Typed | Printed | At |
|---|---|---|
| `PRINT 1+2` | ` 3` | 629 million |
| `PRINT TIME$` | `12:00:01` | 637 million |
| `PRINT DATE$` | `12-11-2026` | 644 million |

The time and date are the ones set in the first-boot dialogue (12:00:00 on 12/11/26), counted by
the RTC model. The first answer takes about 17 million instructions and later ones 7 to 8
million. `real_firmware_runs_a_command_at_the_prompt` in `tests/real_firmware.rs` checks these.

## The LTDC frame (issue 36)

The model now keeps what the display needs and `MemoryMap::frame()` builds the picture from it
and the emulated RAM (`display.rs`). I traced every write to the LTDC from reset to the second
prompt (the first boot's setup and the reboot run the same sequence, so each write below
happens twice in the whole run). What the firmware writes, in order, beyond the table above:

| Register | Value | |
|---|---|---|
| `BCCR` | `0` | black background |
| `L1WHPCR`, `L1WVPCR` | `0x040F00F0`, `0x026F0018` | window start 240 / 24, to 1039 / 623: 800 x 600, at the start of the active area (back porch + 1) |
| `L1PFCR` | `5` | L8 |
| `L1DCCR`, `L1CACR`, `L1BFCR` | `0xFF000000`, `0xFF`, `0x405` | default colour opaque black, constant alpha 255, blend with pixel alpha x constant alpha |
| `L1CFBAR`, `L1CFBLR`, `L1CFBLNR` | `0x24000000`, `0x03200327`, `0x258` | pitch 800, line length field 807, 600 lines |
| `L1CR`, `SRCR` | `0x1`, `0x1`, then `0x11`, `0x1` | layer on and reload, then colour table on and reload |
| `L1CLUTWR` | 256 words | `index << 24 \| RGB`: entry 0 is black, entry 1 `0x000054`, entry 255 `0xE0E0E0` |

`L1CLUTWR` is at offset `0xC4`, not `0x144` (that is layer 2's). The firmware never writes layer
2 or `CKCR`, and never reads `CDSR` or `CPSR`.

The frame is the active area (`AWCR` less `BPCR`), 800 x 600, with the layer window placed in it
and filled from `L1CFBAR` with the pitch from `L1CFBLR`. The layer registers are those as of
the last `SRCR` reload, the colour table is live. At the second prompt the frame has 6 colours:
black, white `E0E0E0`, and the four logo and status colours red, green, yellow and blue. It
shows the MaxiMite logo, the banner (`480MHz Colour Maximite 2 G2`, `MMBasic Version
7.00.00b5` and the three copyright lines) centred, and `>` with the cursor at the left below
them. Some time later (640 million instructions) the status line along the bottom row appears in
dark orange: `A:/`, `Check Disk` and the clock.

Not drawn, and reported as `FrameError::Unsupported` rather than drawn wrong: layer 2, colour
keying, constant alpha below 255, L8 without the colour table, and every pixel format with an
alpha channel (ARGB8888, ARGB1555, ARGB4444, AL44, AL88). L8, RGB565 and RGB888 are drawn. The
default colour (`L1DCCR`), the blending factors and the CLUT alpha are not used, as the firmware
sets the layer opaque. `--screenshot <file>` on the emulator writes the frame as a PPM at the
end of a run.

## Where it blocks now

It does not block at the prompt: commands run, and the frame shows the prompt. I have not run past `PRINT` to find the next wait.
