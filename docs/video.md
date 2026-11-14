# Video modes (issue 58)

What the firmware writes to the LTDC and to the RCC for each `MODE`, and how the emulator turns
that into a frame and a frame rate. I traced every write to `LTDC_*` and `RCC_*` for `MODE r,bits`
from the idle prompt (second boot, saved flash), and for `PAGE WRITE` and `PAGE DISPLAY` in
`MODE 8,12`. The manual is the b5 user manual (MODE, PAGE, Video Modes and Pages).

## Timing: PLL3 and the frame rate

At boot the firmware sets PLL3 twice (first 800 MHz / 20, then 792 MHz / 16) and leaves
`PLLCKSELR` = `0x00101012` (source HSE, DIVM1/2/3 = 1) and `PLLCFGR` = `0x013F0CDD` (PLL3 VCO
range 8 to 16 MHz, wide VCO, fraction off, DIVR3EN on, DIVP3EN and DIVQ3EN off). `RCC_PLL3FRACR`
is written 0 and stays so. Each `MODE` that needs another pixel clock stops the LTDC (`GCR`
`LTDCEN` clear), clears `CR.PLL3ON`, writes `PLL3DIVR`, sets `PLL3ON` again, waits for `PLL3RDY`,
and writes the new `SSCR`, `BPCR`, `AWCR`, `TWCR` before enabling the LTDC again. `D1CCIPR` and
the other kernel clock selects are not touched for the LTDC: its pixel clock is PLL3's R output
directly.

Pixel clock = 8 MHz (HSE) / DIVM3 x (DIVN3 + 1) / (DIVR3 + 1), with DIVN3 = `PLL3DIVR`[8:0] and
DIVR3 = `PLL3DIVR`[30:24]. The frame rate is clock / (total width x total height). The resolution
is the active area the LTDC sees (the firmware doubles the lines of the small modes in memory, see
below). Measured on the board by the game: 748 ticks in 10 s at 640x480, which is 74.8 +- 0.1 Hz.

| MODE | size | LTDC active | SSCR | BPCR | AWCR | TWCR | PLL3DIVR | pixel clock | total | frame |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 800x600 | 800x600 | `0x004f0002` | `0x00ef0017` | `0x040f026f` | `0x041f0270` | `0x0f0f1e62` | 8 MHz x 99 / 16 = 49.5000 MHz | 1056x625 | 75.000 Hz |
| 2 | 640x400 | 640x400 | `0x003f0002` | `0x00b7003a` | `0x033701ca` | `0x034701f3` | `0x19193265` | 8 MHz x 102 / 26 = 31.3846 MHz | 840x500 | 74.725 Hz |
| 3 | 320x200 | 320x400 | `0x001f0002` | `0x005b003a` | `0x019b01ca` | `0x01a301f3` | `0x34346867` | 8 MHz x 104 / 53 = 15.6981 MHz | 420x500 | 74.753 Hz |
| 4 | 480x432 | 480x432 | `0x002f0002` | `0x0089002a` | `0x026901da` | `0x027501f3` | `0x22224466` | 8 MHz x 103 / 35 = 23.5429 MHz | 630x500 | 74.739 Hz |
| 5 | 240x216 | 240x432 | `0x00170002` | `0x0043002a` | `0x013301da` | `0x013901f3` | `0x45458a66` | 8 MHz x 103 / 70 = 11.7714 MHz | 314x500 | 74.977 Hz |
| 6 | 256x240 | 256x480 | `0x00190002` | `0x00490012` | `0x014901f2` | `0x014f01f3` | `0x41418267` | 8 MHz x 104 / 66 = 12.6061 MHz | 336x500 | 75.036 Hz |
| 7 | 320x240 | 320x480 | `0x001f0002` | `0x005b0012` | `0x019b01f2` | `0x01a301f3` | `0x34346867` | 8 MHz x 104 / 53 = 15.6981 MHz | 420x500 | 74.753 Hz |
| 8 | 640x480 | 640x480 | `0x003f0002` | `0x00b70012` | `0x033701f2` | `0x034701f3` | `0x19193265` | 8 MHz x 102 / 26 = 31.3846 MHz | 840x500 | 74.725 Hz |
| 9 | 1024x768 | 1024x768 | `0x00870005` | `0x01270022` | `0x05270322` | `0x053f0325` | `0x0a0a1458` | 8 MHz x 89 / 11 = 64.7273 MHz | 1344x806 | 59.752 Hz |
| 10 | 848x480 | 848x480 | `0x006f0007` | `0x00df001e` | `0x042f01fe` | `0x043f0204` | `0x17172e64` | 8 MHz x 101 / 24 = 33.6667 MHz | 1088x517 | 59.852 Hz |
| 11 | 1280x720 | 1280x720 | `0x00270004` | `0x01030018` | `0x060302e8` | `0x067102ed` | `0x0a0a1465` | 8 MHz x 102 / 11 = 74.1818 MHz | 1650x750 | 59.945 Hz |
| 12 | 960x540 dup | 960x1080 | `0x00150004` | `0x005f0028` | `0x041f0460` | `0x044b0464` | `0x0a0a1465` | 8 MHz x 102 / 11 = 74.1818 MHz | 1100x1125 | 59.945 Hz |
| 13 | 400x300 | 400x600 | `0x00270002` | `0x00770017` | `0x0207026f` | `0x020f0270` | `0x1f1f3e62` | 8 MHz x 99 / 32 = 24.7500 MHz | 528x625 | 75.000 Hz |
| 14 | 960x540 | 960x540 | `0x00270004` | `0x005f0013` | `0x041f022f` | `0x044b0238` | `0x15152a67` | 8 MHz x 104 / 22 = 37.8182 MHz | 1100x569 | 60.422 Hz |
| 15 | 1280x1024 | 1280x1024 | `0x006f0002` | `0x01670028` | `0x06670428` | `0x06970429` | `0x05050a50` | 8 MHz x 81 / 6 = 108.0000 MHz | 1688x1066 | 60.020 Hz |
| 16 | 1920x1080 | 1920x1080 | `0x002b0004` | `0x00bf0028` | `0x083f0460` | `0x08970464` | `0x0404085c` | 8 MHz x 93 / 5 = 148.8000 MHz | 2200x1125 | 60.121 Hz |
| 17 | 384x240 | 384x480 | `0x00250002` | `0x006d0012` | `0x01ed01f2` | `0x01f701f3` | `0x2b2b5667` | 8 MHz x 104 / 44 = 18.9091 MHz | 504x500 | 75.036 Hz |
| 18 | 1024x600 | 1024x600 | `0x00870003` | `0x0127001a` | `0x05270272` | `0x053f0273` | `0x0f0f1e64` | 8 MHz x 101 / 16 = 50.5000 MHz | 1344x628 | 59.832 Hz |

(The registers in this table were traced at 8 bits, and I did not trace them again at the other
depths; the sizes and the `CFBLR`/`CFBLNR` writes below were, for every mode and depth, and agree
with the table. Modes 3, 5, 6, 7, 12, 13 and 17 have the lines of page 0 doubled in memory, so the
LTDC reads twice the lines. Mode 12 is the manual's `960x540, lines duplicated`, and mode 16 its
`1920x1080`.)

Which depths each mode takes (traced by typing `MODE n,d` at the prompt from `MODE 1,8`, for n = 1
to 18 and d = 8, 12, 16 and 32, and looking at the message and at the frame size the model shows
afterwards): modes 1 to 8, 10, 13, 14, 17 and 18 take all four depths at the size in the table.
Modes 9, 11 and 12 take 8 and 16 bits; at 12 and 32 bits they leave the 800x600 frame of
`MODE 1`, with the message `12 and 32-bit colour not available for mode n` for 9,32, 11,12, 11,32,
12,12 and 12,32, and no message at all for 9,12. Modes 15 and 16 take 8 bits only: 15,12, 15,16,
15,32, 16,12, 16,16 and 16,32 print `Mode n has 8 and 1-bit colour only` and leave 800x600. So
there is no 12-bit frame bigger than 1024x600 (mode 18) and no 16 or 32-bit one bigger than
1280x720 (mode 11) and 848x480 (mode 10). I did not trace the 1-bit depth.

Against the board: the game measured 748 ticks in 10 s for `MODE 8,12`, which is 74.8 +- 0.1 Hz,
and the manual says 75 Hz. The model's 640x480 frame is 840 x 500 pixels at 31,384,615 Hz, which
is 6,423,530 core cycles, 74.725 Hz. That is consistent with the board's figure, but the
measurement is good to about a tenth of a Hz, so it does not confirm the three decimals.
800x600 is 75.000 Hz as before. A real-firmware test counts the line interrupt handler's `LIPCR`
writes over 20 frames and gets both, and a unit test measures the line interrupt's period through
the model from the RCC writes the firmware makes.

## Layers, formats, blending

The firmware writes, per mode and depth, for layer 1 (and layer 2 where noted). Each layer's
registers are written to 0 and then the value (the HAL's order), the window is the whole active
area (`WHPCR`/`WVPCR` start at porch + 1), `DCCR` is `0xFF000000` for layer 1 and 0 for layer 2,
and the layer is reloaded with `SRCR`.IMR.

| depth | format (`PFCR`) | `CACR` | `BFCR` | `CFBAR` | pitch (bytes) |
|---|---|---|---|---|---|
| 8 | 5 L8, `CR` = `0x11` (CLUT), 256 `CLUTWR` | `0xFF` | `0x405` | `0x24000000` (AXI SRAM) for modes 1 to 8, 13, 14 and 17; SDRAM `0xD0000000` for the big ones (9, 11, 12, 15, 16, 18) | width |
| 12 | 4 ARGB4444, layer 1 and layer 2 | `0xFF` both | `0x607` both | L1 page 0 and L2 page 1, in SDRAM from `0xD0000000` in the big modes, and for the small ones L1 is `0x24000000` and L2 `0xD0000000` or `0x24040000` | 2 x width |
| 16 | 2 RGB565 | `0xE0` | `0x405` | `0xD0000000` | 2 x width |
| 32 | 0 ARGB8888 | `0xE0` | `0x405` | `0xD0000000` | 4 x width |

`BFCR` = `0x405`: BF1 = constant alpha (4), BF2 = 1 - constant alpha (5). `0x607`: BF1 = pixel
alpha x constant alpha (6), BF2 = 1 - that (7). The constant alpha of 0xE0 in the 16 and 32-bit
depths dims the picture to the 0xE0 top level that the 8-bit colour table has (its entries top
out at `0xE0E0E0`): red `RGB(255,0,0)` shows as `(224, 0, 0)` in both. At 12 bits white is
`0xFFFFFF`. The `bg` of `MODE r,12,bg` goes to `BCCR` (24 bits, not quantised). Colour keying
(`CKCR`) and layer 2's colour table are never written, and layer 2 is only used in the 12-bit
depth.

`CFBLR` is the pitch in bits 28:16 and `CFBLL` in bits 12:0, and `CFBLNR` the number of lines. For
every mode and depth that the firmware accepts (all of the above) it writes `CFBLL` = pitch + 7
with the pitch the window's bytes a line (`CFBLR` `0x03200327` for 800 L8 pixels: pitch `0x320` =
800, `CFBLL` `0x327` = 807), and `CFBLNR` the lines of the active area (`0x258` = 600, `0x1E0` =
480 for 640x480, doubled for the small modes), for layer 1 and, in 12 bits, for layer 2 too. When
it turns layer 2 off on leaving the 12-bit depth it writes `CFBLR` = `0x00000007`. The +7 is what
RM0433 gives for the H7 (32.7.23, p. 1229: the line length in bytes plus 7, with the examples
`0x02000207` for 256 RGB565 pixels and `0x03C003C7` for 320 RGB888); the F4 and F7 had 3. The
model fetches `CFBLL` - 7 bytes a line. The manual says a line length longer than the window needs
is harmless (the useless data is read and discarded, not displayed, 32.4.2, p. 1211) and a shorter
one raises a FIFO underrun; a window wider than the fetched line is `FrameError::Unsupported`,
because the manual does not say what is shown then. A window is fetched from its own first pixel,
so a window that starts before the active area has its first pixels fetched and not shown.

Memory layout (`MODE 8,12`, the game's): page n at `0xD0000000` + n x 614,400 (640 x 480 x 2),
ARGB4444, page 0 is layer 1 and page 1 layer 2. At 800x600 12-bit the pages are 966,656 bytes
apart (page 1 at `0xD00EC000`).

## Page flips

- `PAGE WRITE n` writes no LTDC register: it only changes where the drawing goes.
- `PAGE DISPLAY n` (and `PAGE DISPLAY n, 1` for the upper layer) blocks for about two frames,
  then rewrites all of that layer's registers with the new `CFBAR` (page n) and reloads with
  `SRCR`.IMR. It does not use `SRCR`.VBR. The write is done from the line interrupt, after the
  active area (`LIPCR` = AAH + 1), so the immediate reload is in the blanking and the flip does
  not tear. So the flip works through the interrupt timing, and the model needs the line
  interrupt to be right (it is) and IMR to be immediate (it is).
- `VBR` is written once per `MODE`: `L1CR` = 0, IMR, `L2CR` = 0, a read of `SRCR` (0), VBR,
  a read of `CDSR`, then `GCR` off. It does not poll `SRCR` for the VBR to clear.
- `PAGE DISPLAY -1` is only valid in a program and was not traced; it would clear `GCR`, which
  the model shows as no frame.

## What the model does

- `PixelClock` (`rcc.rs`): the RCC works PLL3's R output out from `CR.PLL3ON`, `PLLCKSELR`,
  `PLLCFGR` (`DIVR3EN`, `PLL3FRACEN`), `PLL3DIVR` and `PLL3FRACR`, with HSE = 8 MHz as the only
  source (other sources, or `DIVM3` = 0, give 0). The VCO is worked out exactly (the reference is
  divided by `DIVM3` after it is multiplied, not before) and the clock is truncated to whole Hz.
  PLL3 only locks, and `PLL3RDY` only sets, for a reference (HSE / `DIVM3`) of 1 to 16 MHz, within
  the range of the VCO that `PLL3VCOSEL` picks: the wide VCO (`PLL3VCOSEL` clear) takes a reference
  of 2 to 16 MHz and makes 192 to 960 MHz, the medium one (set) takes 1 to 2 MHz and makes 150 to
  420 MHz. That is RM0433 8.5.5 (PLL block diagram and text, pp. 346 to 348) and 8.7.17 (p. 410).
  The manual does not say a PLL outside those ranges fails to lock (it says the output frequency is not
  guaranteed); not locking there is my choice, and then there is no pixel clock. `PLL3RGE` is not a
  condition: the manual only has it set to the reference's range for optimal performance
  (8.7.12, p. 401), and the model ignores it. `DIVN3` is 4 to 512 (the register
  field 3 to 0x1FF), which no VCO range can fall below, so the VCO range covers the lower limit;
  `DIVR3` is 1 to 128. The fraction (`FRACN3`, 13 bits, 8.7.18, p. 411) only counts once
  `PLL3FRACEN` goes from 0 to 1, which latches it into the sigma-delta modulator (8.5.5, p. 347);
  a new `FRACN3` written with `PLL3FRACEN` still set, or `PLL3FRACEN` cleared, changes nothing
  until the next 0 to 1, and a reset clears what was latched. The settings the manual locks while
  the PLL is on (`DIVM3`, and `PLLSRC` while any PLL is on; `PLL3RGE`, `PLL3VCOSEL` and `DIVR3EN`;
  `DIVN3` and the three dividers) are not written while `PLL3ON` is set (8.7.11, p. 398, 8.7.12 and
  8.7.17); `FRACN3` and `PLL3FRACEN` can be written at any time. The model locks at once; the
  manual says `PLL3RDY` is set when the output is within 2% of the target. The LTDC runs
  from the pixel clock as it is: the frame starts when it is enabled and the clock runs, and when
  the clock starts, stops or changes under an enabled LTDC the counters keep their position and go
  on at the new rate from that cycle (the clock logs each change with its cycle and wakes the LTDC,
  which takes every one, so a stop and a restart at the same rate are seen too), as the chip's
  counters do; while the clock is stopped they stay where they were. With no clock the frame does
  not run. The LTDC never asks for a wake less than 1000 cycles ahead, so a frame of a few pixels
  cannot make the CPU stop for it at every instruction; the flags are still worked out as they are
  read.
- Formats: ARGB8888, RGB888, RGB565, ARGB1555, ARGB4444 and L8 through the layer's own colour
  table. A channel of fewer than 8 bits is widened by bit replication, as RM0433 32.4.2 gives it
  (p. 1209: the bits are repeated until there are more than 8 and the top 8 taken; the red of
  RGB565, abcde, becomes abcdeabc), so full intensity is 255 (RGB565 and ARGB1555 5/6 bits,
  ARGB4444 nibbles); ARGB1555's alpha is its one bit repeated, 0 or 255; formats without alpha are
  opaque. Not drawn, and an explicit `FrameError::Unsupported`: AL44 and AL88, L8 without
  `CLUTEN`, colour keying, factor codes that BFCR reserves, and a window wider than the fetched
  line.
- Blending (RM0433 32.4.2, layer blending, p. 1211, and 32.7.19 to 32.7.21, pp. 1226 and 1227):
  start from `BCCR`, and for layer 1 then layer 2, `BC = BF1 x C + BF2 x Cs` with `Cs` the picture
  so far (the background colour, for layer 1). `BF1` is code 4 (constant alpha) or 6 (pixel alpha
  x constant alpha) and `BF2` is 5 (1 - constant alpha) or 7 (1 - pixel alpha x constant alpha),
  the other codes are reserved. The constant alpha in `CACR` is divided by 255 by the hardware; it
  resets to 0xFF (ST's SVD says 0, the manual wins) and `BFCR` to 0x0607. The manual's example
  (constant alpha 240, C 128, Cs 48, BC 123) is a test. Outside its window the layer's default
  colour `DCCR` (with its own alpha, 0 at reset: transparent black) stands for C, and so it does
  when the layer is disabled: the manual says the blending is always done, even for a layer that
  is off, and to keep the factors of a disabled layer at their reset value so that the default
  colour does not show (32.4.2, p. 1212). So a disabled layer blends its default colour over the
  whole active area. What I chose where the manual is silent:
  - A disabled layer with reserved factor codes adds nothing. The firmware leaves layer 2 so
    (`LEN` 0 and `BFCR` 0) on leaving the 12-bit depth; the manual gives those codes no meaning.
  - The rounding: I round to nearest, `(sum + 127) / 255`. The manual says only that the constant
    alpha is divided by 255, so the chip's rounding is not known, and a mid-range level can be one
    out; 0 and full intensity are exact.
  - What is shown for the lines of a window past `CFBLNR`: the default colour. The manual only
    says the prefetch stops at the end of the frame buffer.
  - The window registers count from the start of the sync, like the porch registers, as the manual
    has it (32.7.15 and 32.7.16, pp. 1223 and 1224): the first pixel of the active area is
    `AHBP` + 1 and the last is `AAW`, the first line `AVBP` + 1 and the last `AAH`, and a window's
    `WHSPPOS` and `WVSPPOS` are its last pixel and line. The manual wants a window inside the
    active area; one that is not is drawn clipped.
- An active area with no width or no height (`AWCR` at or before `BPCR`) is not a picture:
  `FrameError::Empty`. The window keeps the last good frame, and draws nothing for a frame or a
  window with no size.
- Speed: a frame is built a row at a time. The blend of each possible pixel alpha is worked out
  once for a layer, a pixel that replaces what is below it or leaves it alone costs no arithmetic,
  and each pixel format has its own loop. The results are the same as working every pixel out
  from the formula, which a test checks over random layers, formats, alphas, windows (also ones
  that start before or run past the active area) and `CFBLL`/`CFBLNR`, against a reference that
  works from the raw register values. The biggest frames this firmware makes take, in release
  (`compositing_speed`, best of 40, on a busy machine): 1920x1080 L8 3.3 ms, 1280x720 RGB565 5 to
  6 ms, 848x480 ARGB8888 2.1 ms, 1024x600 with two ARGB4444 layers 3 ms for drawn pixels and 7 to
  10 ms for random alpha, and the 800x600 L8 prompt 0.7 ms. (The old code took 3 to 6 ms for the
  prompt and about 38 ms for two layers at 1920x1080, a size this firmware does not have at 12 bits;
  I did not time it for the other cases.)
- `SRCR`: IMR reloads at once. VBR is held to the next vertical blank (the line after `AWCR`'s
  active height; RM0433 32.7.6, p. 1218, puts it at the beginning of the first line after the active display area; if
  that is at or beyond the last line of the frame there is no such line, and I take the blank to
  be line 0 of the next frame, which the manual does not say) and reads 1 until then; the display
  takes the registers as they are at that moment and `RRIF` is set (interrupt if `IER.RRIE`).
  `IMR` reloads at once and clears. With the LTDC off it waits for the first blank after it is
  enabled. Lines are counted from 0 at the first line of the vertical sync (32.4.1, p. 1207), so
  `LIPCR` = `AAH` is the last active line and the firmware's `AAH` + 1 the first after it; the
  manual does not say how far into the line the interrupt is, and the model flags it at the start
  of the line. The layer registers are shadowed (32.4.1, p. 1208; 32.7.6): a write goes to a copy
  and a read returns the value of the last reload, so the new value is only read back after the
  reload (`CLUTWR` is not shadowed). The registers around the layers are not.
  The clock is woken at the blank so the frame is current without any register access.

Tests: unit tests in `display.rs` (each format, layers, blending, default colour, errors),
`ltdc.rs` (frame rate for 800x600 and 640x480 and the 640x480 rate through the RCC, a pixel clock
that starts, stops or changes, line events flagged once, VBR timing, layer 2 registers and colour
table), `rcc.rs` (the pixel clock and PLL3's ranges), `window.rs` (the fit of a picture with no
size); real-firmware tests (ignored, `CMM2_FIRMWARE`) in `crates/cmm2-core/tests/real_firmware.rs`:
every depth at 800x600 and 640x480 with shapes, page flips and the two layers, and the frame rates.
`CMM2_SCREENSHOTS=<dir>` makes them leave PPMs.
