# CMM2 Gen 2 hardware map

What the emulator has to model. Sources are marked: **[board]** is read from a real CMM2 Gen 2
(MMBasic 7.00.00b5, idle at the prompt, 2026-11-12), **[BOM]** is the Gen 2 parts list,
**[memmap]** is the Back Shed document "CMM2 Firmware and MMBasic Memory Map" (a user-written
summary of how the firmware uses the STM32H743; not committed here), **[fw]** is the firmware binary.

## Parts [BOM]

STM32H743IIT6 (Cortex-M7, 480 MHz), 32 MB SDRAM (FMC bank 2), CH340C USB-serial (console on
USART1), DS3231 RTC, 8 MHz oscillator, 16 MHz and 32.768 kHz crystals.
The firmware is built with ST's GNU toolchain 13.3 and newlib, on the ST HAL [fw].

## Address map [memmap, confirmed in part by board]

| Range | What | Notes |
|---|---|---|
| 0x0000_0000, 64 KB | ITCM RAM | |
| 0x0800_0000, 1152 KB | Flash: firmware | The `.bin` is 1,179,432 bytes |
| 0x0812_0000 to 0x081D_FFFF | Flash: stored BASIC program | Six 128 KB pages. Needs flash erase/program emulation |
| 0x081E_0000, 128 KB | Flash: saved options | `OPTION` settings persist here |
| 0x2000_0000, 128 KB | DTCM RAM | Firmware variables and C stack |
| 0x2400_0000, 512 KB | AXI SRAM: display page 0 | LTDC framebuffer at boot **[board]** |
| 0x3000_0000, 256 KB | SRAM1/2: MMBasic heap | |
| 0x3004_0000, 32 KB | SRAM3: SUB/FUNCTION hash table | |
| 0x3800_0000, 64 KB | SRAM4: variable table | |
| 0x3880_0000, 4 KB | Backup SRAM | `VAR SAVE` |
| 0x4000_0000 to 0x5FFF_FFFF | Peripherals | |
| 0xD000_0000, 32 MB | SDRAM (FMC bank 2) | First 7.5 MB display pages, then program RAM, then the 24 MB heap |

## Clocks [board]

HSE 8 MHz. PLL1: 960 MHz, /2 gives 480 MHz CPU. Buses 240 MHz (AHB) and 120 MHz (APB).
PLL2: 960 MHz VCO. PLL3: 792 MHz VCO, R output /16 gives the 49.5 MHz LTDC pixel clock.

## Peripherals enabled at the idle prompt [board]

FMC, LTDC, GPIO A to I, USART1, TIM2/4/5/16, DAC (both channels), RNG, USB OTG (embedded FS PHY),
backup SRAM, SYSCFG, RTC. Not enabled: DMA1/2, MDMA, DMA2D, SDMMC (probably enabled only when used).

## Video: LTDC [board]

800x600 active, total 1056 x 625, HSYNC 80, back porch 160, VSYNC 3, back porch 21, sync polarity
active high, 49.5 MHz pixel clock = 75 Hz. Layer 1 only, pixel format L8 with CLUT (so the 8-bit
RGB332 mode is index-to-colour through the CLUT), framebuffer 0x2400_0000, pitch 800.
Output is three 8-bit resistor ladders.

## SDRAM: FMC [board]

Bank 2, 16-bit, 13 row and 9 column bits, 4 banks, CAS latency 3 = 32 MB.
SDCR1 0x3AD0, SDCR2 0x1D9, SDTR1 0x0F1F6FFF, SDTR2 0x01010361, SDRTR 0x754, BCR1 0x820030DB.

## Console: USART1 [board]

CR1 0x2000002D (UE, RE, TE, RXNEIE, FIFOEN), BRR 0x104. This is the CH340 serial link at 115200
baud (needs the kernel clock prescaler to explain the BRR value; not yet checked).

## Timers [board]

| Timer | State | Notes |
|---|---|---|
| TIM5 | Running (CR1 0x81), 32-bit, PSC 0, ARR 0xFFFFFFFF, update interrupt on | Looks like a free-running timebase |
| TIM16 | Running, PSC 239, ARR 49999, update interrupt on | At a 240 MHz timer clock this is a 20 Hz tick; unconfirmed |
| TIM4 | Stopped, PSC 1249, ARR 1 | Configured but idle; possibly audio |
| TIM2 | Stopped, 32-bit | |

## Audio and RNG [board]

DAC CR 0x10001 and MCR 0x10001: both channels enabled. RNG CR 4: enabled.

## USB [board]

OTG controller at 0x4008_0000, with a keyboard plugged in: GUSBCFG 0x20001847 (forced host mode,
embedded full-speed PHY), GAHBCFG 1 (global interrupt on, **DMA off**, so FIFO/slave mode),
GCCFG 0x10000, HCFG 0x206, HPRT 0x41805 (device connected and enabled, **low speed**), GINTMSK
0x23000014. No DMA is a favourable fact for emulation.

## GPIO [board]

Raw MODER/AFRL/AFRH for ports A to I were read (not yet decoded into a pin table). Examples that
match the expected functions: PA9 and PA10 are AF7 (USART1), PA11 and PA12 are AF10 (USB OTG FS).
The external I/O connector's pin-to-GPIO assignments are in the memory map document.

## Not readable from BASIC

The Cortex-M private peripherals (NVIC at 0xE000E100 and so on) return an address error from
`PEEK`. Interrupt use has to come from tracing the emulated firmware, or from the memory map document.
