# What the firmware does at boot

This is the real firmware (v7.00.00b5) running on the emulator with every peripheral register
served by the plain stub: a register reads back what was last written, or 0. Nothing is modelled
yet, so the run shows what the firmware asks of the hardware and where it first gets stuck.
Measured at commit 5099547, 3 million instructions, with:

```
cmm2-emulator --firmware CMM2_MMBasic_7.00.00b5.bin --run 3000000 --trace trace.txt --trace-mmio
```

## Where it blocks

**At about instruction 225,608 the firmware polls `PWR_D3CR` and never gets out.**

The loop is three instructions at `0x0809953C` (`ldr r3, [r2, #24]`, `lsl r3, r3, #18`, `bpl`).
Shifting left by 18 moves bit 13 into the sign bit, so it waits for bit 13 of `PWR_D3CR` to
be set. That is `VOSRDY`, the flag that says the voltage scaling has settled. The firmware had just
written `0xC000` to `PWR_D3CR` (the highest voltage scale) and set `SYSCFG_PWRCR` bit 0 (overdrive).
The stub hands back `0xC000`, bit 13 stays clear, and the loop runs for the rest of the 3 million
instructions: 912,243 reads in all.

So nothing before this point needs a modelled peripheral, and this is the first thing that does.

## What happens before it

| Instruction | What |
|---|---|
| 1 to 40 | The reset handler resets the clock tree: sets `HSION` in `RCC_CR`, then writes 0 to `RCC_CFGR`, the three domain clock configuration registers, the PLL source and configuration registers, the three PLL divider and fraction registers and `RCC_CIER`. It reads `DBGMCU_IDC` and writes `AXI_AXI_TARG7_FN_MOD` (the AXI interconnect), then falls into the C startup. |
| 41 to 211,595 | The C startup copies initialised data and code (including the ITCM code) and clears the BSS. No peripheral access at all, about 211,000 instructions. |
| 211,596 to 211,650 | Cache setup through the Cortex-M7 cache maintenance registers, which the SVD does not describe: write `0xE000EF50` (instruction cache invalidate), write `0xE000ED84` and read `0xE000ED80` (the cache size selection and size ID registers), write `0xE000EF60` (data cache invalidate by set and way). |
| 211,648 to 211,669 | Reads `RCC_CFGR`, `RCC_CR` and `RCC_D1CFGR`: a check of the current clock. |
| 211,752 to 212,300 | Turns on the `SYSCFG` clock (`RCC_APB4ENR` bit 1) and the `GPIOA` clock (`RCC_AHB4ENR` bit 0), then sets pull-downs on `GPIOA` pins 11 and 12 (`GPIOA_PUPDR` becomes `0x00800000`, then `0x02800000`; those pins are the USB data lines on this chip) and writes `GPIOA_MODER` back unchanged. |
| 212,291 to 212,427 | Turns on the `GPIOF` clock (`RCC_AHB4ENR` to `0x21`), sets a pull-up on `GPIOF` pin 9 (`GPIOF_PUPDR` = `0x00040000`) and writes `GPIOF_MODER` as 0 (inputs). |
| 212,439 to 224,427 | Reads `GPIOF_IDR` exactly 1000 times. It is a fixed-length loop, so the stub's 0 gets through it. |
| 224,547 to 224,559 | Configures `GPIOE`: `GPIOE_PUPDR` = 0, then `GPIOE_MODER` = `0x000C0000` (pin 9 in analog mode). |
| 225,573 to 225,603 | Reads `PWR_CR3` twice and `DBGMCU_IDC`, then the `PWR_D3CR` write above and the `SYSCFG_PWRCR` overdrive write. |
| 225,608 on | The `PWR_D3CR` poll. |

## SysTick already works

Every 32,000 instructions or so the poll is interrupted by a short burst that reads `GPIOH_IDR`
and then `GPIOD_IDR`, 87 times each in the 3 million instructions. That is the SysTick handler:
vector 15 in the table points at `0x2B9`, the handler starts at `0x2B8` in ITCM, it starts exactly 87
times in the run (the first at instruction 244,194), and each start is followed by the two port
reads, through a short chain that ends in flash at `0x0809F3A8`. The core delivers it with nothing
added by the machine: the firmware started SysTick during startup and zmu runs the timer and its
interrupt.

## The 31 distinct registers touched, in first-touch order

`RCC_CR`, `RCC_CFGR`, `RCC_D1CFGR`, `RCC_D2CFGR`, `RCC_D3CFGR`, `RCC_PLLCKSELR`, `RCC_PLLCFGR`,
`RCC_PLL1DIVR`, `RCC_PLL1FRACR`, `RCC_PLL2DIVR`, `RCC_PLL2FRACR`, `RCC_PLL3DIVR`, `RCC_PLL3FRACR`,
`RCC_CIER`, `DBGMCU_IDC`, `AXI_AXI_TARG7_FN_MOD`, four cache-maintenance addresses not in the SVD
(`0xE000EF50`, `0xE000ED84`, `0xE000ED80`, `0xE000EF60`), `RCC_APB4ENR`, `RCC_AHB4ENR`,
`GPIOA_PUPDR`, `GPIOA_MODER`, `GPIOF_PUPDR`, `GPIOF_MODER`, `GPIOF_IDR`, `GPIOE_PUPDR`,
`GPIOE_MODER`, `PWR_CR3`, `PWR_D3CR`, `SYSCFG_PWRCR`, `GPIOH_IDR`, `GPIOD_IDR`.

All accesses were 32 bits wide.

## What to expect next

I have not run past the `PWR_D3CR` wait here. An earlier throwaway spike on a different CPU core
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
