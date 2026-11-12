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
