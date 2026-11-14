# How the firmware drives the USB host

This is the spike for issue 11. I wanted to know what the emulator has to do so that the real
firmware (v7.00.00b5) sees a USB keyboard, with held-key state, so that `KEYDOWN()` and `INKEY$`
work and games can be played. Today the window types into the console UART (`keys.rs`), which
has no key-up events.

The short answer is good news:

- **It is a plain slave-mode (FIFO, no DMA) host driver on the second OTG block, 0x40080000, IRQ
  101.** The manual says the USB host is TinyUSB, and what I traced behaves like that stack. The
  firmware reads almost nothing from the chip beyond the registers any DWC2-style host needs.
- **A small fake low-speed HID keyboard behind a minimal host-controller model was enough to get
  all the way to working keys**: the firmware enumerated it, printed `UK USB Keyboard Connected
  on channel 1`, and `KEYDOWN(0)` to `KEYDOWN(8)` and `INKEY$` followed the reports I sent,
  including modifiers, several keys held at once and key-up. Hot unplug and replug also worked.
  A mouse and a keyboard-plus-mouse composite device worked the same way. A generic gamepad
  enumerated but I could not get it to report (see section 8).
- The emulator never has to generate key repeat, scan codes to characters or layouts: the
  firmware does all of that. It has to supply HID reports and a believable host controller.

## How I found this

- I ran the real firmware with `--flash` (so boot goes straight to the prompt, about 412 million
  instructions) with `--trace-mmio`, streamed through `awk` into a small file instead of keeping
  the trace. That showed what the firmware does today, with the stub answering the OTG window.
- I disassembled the `.bin` (linear Thumb-2 disassembly, read by me; no source was used or
  copied). I only read the parts that the traces pointed to.
- To see what happens after the controller answers sensibly, I built a scratch copy of the repo
  outside the tree (a `mktemp -d` directory, release builds) with a throwaway OTG model of about
  700 lines: the registers, a host-channel model, an RX FIFO, a port that connects and resets, a
  1 ms start-of-frame, and a fake device behind it. The fake device had three descriptors I wrote
  myself (a keyboard, a mouse and a pair of them in one device), answered the control requests the
  firmware sent, and handed out reports from a script I gave it. I iterated until the firmware
  was happy. The model, the traces and the flash file were deleted with the scratch copy; nothing
  of it is in the repository.
- Tags below: **[trace]** is the unmodified emulator with the stub; **[scratch]** is the scratch
  model run (observed); **[disasm]** is read from the disassembly; **[manual]** is the user
  manual; **[guess]** is my inference, not seen.

## 1. Which block, which interrupt, which driver

| Fact | Value | How |
|---|---|---|
| Window the firmware uses | 0x40080000 (`OTG2_HS_GLOBAL` in the SVD, which ST's reference manual calls OTG_FS, the one with the embedded full-speed PHY) | [trace], [scratch] |
| The other block, 0x40040000 (`OTG1_HS_GLOBAL`) | never touched in 450 million instructions | [trace] |
| NVIC line | 101 (`OTG_FS` global interrupt). The SVD lists lines 98 to 100 (FS endpoint 1 in/out, wake-up) and nothing here uses them | [disasm], [scratch] |
| Vector 117 | 0x0808B6FB, which loads r0 = 0 and r1 = 1 and branches to 0x0800FA60, the host interrupt handler | [disasm] |
| Per-port table | 0x080CA454, three words each: register base, IRQ number, flags. Entry 0 is 0x40080000 and IRQ 0x65; entry 1 is 0x40040000 and IRQ 0x4D (77). The firmware only ever uses entry 0 | [disasm] |
| Driver | Slave mode (no DMA): `GAHBCFG` ends as 1, no `HCDMA` writes, data moves through the FIFO windows | [trace], [scratch] |
| Core | Identified with `GSNPSID` (0x40080040) and `GHWCFG2` (0x40080048) only. No other `GHWCFG` register is read | [scratch] |
| Interrupt mask ends as | `GINTMSK` 0x23000014 (OTGINT, RXFLVL, HPRTINT, HCINT, DISCINT), `GAHBCFG` 1, `GCCFG` 0x10000, `HCFG` 6 once the port is up, `HPRT` 0x41805 with a low-speed device. All of these agree with what I read from the real board (docs/hardware-map.md), except `GUSBCFG` and `HCFG`: see section 2 | [scratch] |

The base address in `hardware-map.md` (0x4008_0000) is right.

## 2. What the firmware does with the OTG block at boot today (nothing attached, stub answering)

[trace] Over 450 million instructions (to the prompt and past it) the firmware touches the OTG
window **three times, all reads**, at instruction 31,915,462 (cycle 51.7 million, about 108 ms
after reset):

| pc | Access |
|---|---|
| 0x0800EAD4 | read 0x40080048 (`GHWCFG2`), returns 0 |
| 0x0800F280 | read 0x40080048 again |
| 0x0800EB00 | read 0x40080040 (`GSNPSID`), returns 0 |

After that, nothing. [disasm] The code compares the top 16 bits of `GSNPSID` with 0x4F54, 0x5531
and 0x5532. If none matches it calls a do-nothing function twice (0x0808B6F8, a `bx lr`; I take it
to be an assert) and returns failure, and the USB stack never comes up. So today the firmware
believes there is no USB controller, never sees a keyboard, and nothing hangs. That is why the
current boot works.

[scratch] What would hang if I answered only half of it. With `GSNPSID` answered but every other
register still the stub:

- `GRSTCTL` (0x40080010) has the SVD reset value 0x20000000, so bit 31 (`AHBIDL`) is 0. The
  firmware polls that bit at 0x0800E9B4 in a loop with no counter and no timeout, so it spins
  forever (I watched 4 million instructions of it). Reading 0x80000000 passes at once.
- After that it sets `CSRST` (bit 0) and polls until it clears (0x0800E9CE), then `AHBIDL` again.
  Then it writes the TX and RX flush bits and polls them clear (0x0800EB9A, 0x0800EBA4). A
  register that reads `AHBIDL` set and everything else clear satisfies every one of these.
- [disasm] If the low half of `GSNPSID` is above 0x4209, a different path waits for bit 29
  (`CSRST_DONE`) instead. I used 0x4F54310A (a guess for the value) and the other path was not
  exercised. A model should set bit 29 as well, to be safe.

The core and host initialisation as the firmware does it [scratch] (first run, nothing else in
the way; offsets from 0x40080000; all of this takes about 500 cycles of the 480 MHz clock):

| Order | pc | Access |
|---|---|---|
| 1 | 0x0800EB00 | read `GSNPSID`, the check above |
| 2 | 0x0800EB24 | `GAHBCFG` read, write 0 (global interrupt off) |
| 3 | 0x0800EB2C | `GUSBCFG` read 0xA00 (SVD reset), write 0xA40 (`PHYSEL`, embedded FS PHY) |
| 4 | 0x0800E9F4 | `GCCFG` write 0x10000 (`PWRDWN`) |
| 5 | 0x0800E9B4 to E9D4 | core reset: wait `AHBIDL`, write `GRSTCTL` bit 0, wait it clear, wait `AHBIDL` |
| 6 | 0x0800EBEE to EB86 | `GUSBCFG` edited in three writes (0x1640, 0x1A40, 0x1A47): turnaround time and timeout calibration |
| 7 | 0x0800EB88 | `PCGCCTL` (0xE00) read and write 0 |
| 8 | 0x0800EB98, EBA2 | flush: `GRSTCTL` write 0x420 (TX flush, all FIFOs), write 0x10 (RX flush), each waits for clear |
| 9 | 0x0800EBB0 to EBB4 | `GINTSTS` write 0xFFFFFFFF, `GOTGINT` write 0xFFFFFFFF, `GINTMSK` write 0 |
| 10 | 0x0800EBF8 | `GINTMSK` write 0x10 (RXFLVL) |
| 11 | 0x0800F294 to F2B2 | host init: read `HFIR` (0xEA60), write 0x1EA60 (`RLDCTRL`); `GUSBCFG` \|= 0x20000000 (forced host mode); read `GINTSTS`, `GCCFG`, `CID` |
| 12 | 0x0800F2E8 | `HCFG` write 4 (`FSLSS`) |
| 13 | 0x0800F36E to F37E | FIFO sizes: `GDFIFOCFG` (0x5C) 0x04000400, `GRXFSIZ` 0x90, `HNPTXFSIZ` (0x28) 0x002003E0, `HPTXFSIZ` (0x100) 0x03500090 |
| 14 | 0x0800F320, F328 | `HPRT` write 0xAE (clear the change bits), then 0x1000 (`PPWR`, port power) |
| 15 | 0x0800F336, F342 | `GINTMSK` \|= 0x23000004, then `GAHBCFG` write 1 (global interrupt on) |

The FIFO split adds up to 0x400 words: receive 0x90 (144) at 0, periodic TX 0x350 (848) at 0x90,
non-periodic TX 0x20 (32) at 0x3E0. The last write is the `GAHBCFG` 1: from then on the firmware
does not touch the block again until an interrupt arrives. With nothing connected, I ran 1.5
billion instructions with the model and no device and it stayed silent. The firmware does not
poll for a device: it waits for the interrupt, so a model with no device and no interrupts is a
quiet boot, which is why it is safe to have the model always present.

Two differences from the real board's readings, both explained by the stub's reset values (the SVD
says `GUSBCFG` 0xA00 and `HCFG` 0, which cannot be what the silicon resets to):

- The firmware ends with `GUSBCFG` 0x20001A47 where the board reads 0x20001847. The extra bit (0x200) was already in the SVD reset value and
  the firmware only edits some of the fields. The silicon's reset value is probably 0x1440 (my
  guess: it explains the 0x1847 the board shows). RM0433 57.14.4 (p. 2625) gives 0b0X00 0000 0000 0000 0001 0100 0001 0000, that is 0x1410 with bit 30
  (`FDMOD`) undefined, which has bit 4, reserved in the manual, where the board shows `PHYSEL` (bit 6); that would not leave
  0x1847 after the firmware's edits, so I keep 0x1440.
- `HCFG` ends 6 where the board reads 0x206. The silicon's reset value is probably 0x200 (my
  guess, same reasoning); RM0433 57.14.22 (p. 2651) gives 0, which would not leave 0x206 either.

I would set both reset values to those and check against the board (see the open questions).

## 3. Connect, reset, speed detect, enumeration

Everything in this section is [scratch], with a fake low-speed keyboard that is attached 200 µs
after the firmware sets `PPWR`. The cycle numbers are the 480 MHz machine clock. The device
descriptors are in section 9.

**Connect.** The model sets `HPRT.PCSTS` and `PCDET` and the interrupt (`GINTSTS.HPRTINT`, bit 24)
fires on line 101. The handler (0x0800FA60) reads `GINTMSK`, `GINTSTS` and `HPRT`, and writes `HPRT`
back with `PCDET` set to clear it (0x1803: it keeps `PPWR` and leaves `PENA` alone, because
`PENA` is write-one-to-disable). It then posts an event and returns. **The firmware does not do
anything about the new device for another 1.4 s** (cycle 738.6 million, instruction 443.9
million, shortly after the prompt appears): that is when its USB task runs. A model that attaches at
power on is therefore the same thing as the real board with a keyboard plugged in at power on.

**Port reset.** The task reads `HPRT` twice (`PCSTS` set), then writes `HPRT` 0x1901 (`PRST` set,
`PPWR` kept). It holds the reset for 24.96 million cycles (52 ms), then clears `PRST`. The
model has to answer by setting `PENA` and `PENCHNG` (and so `HPRTINT`) when `PRST` goes back
to 0; real hardware does this about 2 µs after the reset ends. Speed detect: the handler reads
`HPRT` bits 18:17 (`PSPD`: 0 high, 1 full, 2 low) and **for a low-speed device writes `HCFG` 6
(`FSLSPCS` = 6 MHz) and `HFIR` 0x1176F (`RLDCTRL`, 5999)**; for full speed I would expect 48 MHz
and 47999 [guess; I never ran one]. It then clears `PENCHNG` (`HPRT` write 0x41809, which keeps
`PENA` off in the write so it stays enabled). About 4 ms later it reads `HPRT` again (0x41805), and
**about 106 ms after the reset ended** it starts the first control transfer.

**The control transfers**, in the order the firmware issues them (the bytes of the SETUP stage; the
data comes in 8-byte packets because the endpoint 0 size is 8):

| Step | SETUP bytes | Meaning | Notes |
|---|---|---|---|
| 1 | 80 06 00 01 00 00 08 00 | GET_DESCRIPTOR device, 8 bytes, address 0 | gets `bMaxPacketSize0` |
| 2 | 00 05 01 00 00 00 00 00 | SET_ADDRESS 1 | **No second port reset.** The device takes address 1 after the status stage. About 6 ms later: |
| 3 | 80 06 00 01 00 00 12 00 | GET_DESCRIPTOR device, 18 bytes, address 1 | 3 IN packets |
| 4 | 80 06 00 03 00 00 FF 00 | GET_DESCRIPTOR string 0 (language IDs), 255 bytes | works with a 4-byte answer; a STALL is also tolerated (I tried both) |
| 5 | 80 06 00 02 00 00 09 00 | GET_DESCRIPTOR configuration, 9 bytes | |
| 6 | 80 06 00 02 00 00 22 00 | GET_DESCRIPTOR configuration, wTotalLength (34 here) | |
| 7 | 00 09 01 00 00 00 00 00 | SET_CONFIGURATION 1 | |
| 8 | 21 0A 00 00 00 00 00 00 | HID SET_IDLE (duration 0, report 0), interface 0 | so a keyboard reports only on change |
| 9 | 21 0B 00 00 00 00 00 00 | HID SET_PROTOCOL 0 (boot protocol), interface 0 | |
| 10 | 81 06 00 22 00 00 3F 00 | GET_DESCRIPTOR HID report descriptor, wDescriptorLength | the firmware really parses it (below) |

Each control transfer is SETUP (OUT, `DPID` = 3, 8 bytes), data stage if any (IN, first packet
DATA1), and a status stage (the opposite direction, zero length, DATA1). The steps are about 2 ms
apart: the firmware does one stage per task wake-up, not back to back. Step 6 re-reads the whole
configuration, with the length it got from step 5. The firmware then prints
`UK USB Keyboard Connected on channel 1` on the console, at cycle 839 million (the "UK" is its
`OPTION USBKEYBOARD`, which comes from the first-boot question).

**A second gap.** After the last step the firmware does nothing with USB for another 1.03 s. At
cycle 1.334 billion (instruction about 710 million) it sends **SET_REPORT** for the keyboard LEDs
(21 09 00 02 00 00 01 00, one data byte, 0x00: all LEDs off), starts the first interrupt IN
transfer, and only then are keys read. So a keyboard becomes usable about 2.78 s of emulated time
after reset, which is about 30 s of wall time today (the host runs about 25 million instructions
a second). That is worth knowing for tests and for what the window does before then (section 7).

The firmware sends `SET_REPORT` again whenever a lock-key LED changes, [guess, not tested]. The
device only has to accept it.

**Hot plug.** [scratch] I detached the device 300 ms after the first poll (`PCSTS` and `PENA` clear,
`PENCHNG` set, `GINTSTS.DISCINT`, bit 29, set and `HPRTINT` pending). The firmware read `HPRT`
(0x1008), cleared `DISCINT` by writing 0x20000000, cleared `HAINTMSK`, masked `HCINT` and `SOF` in
`GINTMSK` and printed `USB Keyboard Disconnected`. 1.2 s later I attached it again and the
whole thing ran again from the reset, ending in another `Connected` line. So the model has to
clear its channels and queues on detach, and a re-attach is a full re-enumeration.

## 4. How keys are read

**Interrupt IN polling**, 8-byte boot-protocol reports, endpoint 1, as the descriptor says.

- [scratch] The firmware reads the report descriptor and takes it seriously (the mouse made it
  print `its reports: X 8 bits at bit 8, Y 8 bits at bit 16, a wheel`), but the keyboard report
  was read as the standard 8-byte boot report: byte 0 modifier bits, byte 1 reserved, bytes 2 to
  7 key usage codes. I did not try an unusual descriptor.
- The polling is driven by **start-of-frame interrupts, which the firmware only unmasks while an
  interrupt endpoint is waiting** (it writes `GINTSTS` 8 to clear `SOF` and sets bit 3 of `GINTMSK`;
  it removes it again when no periodic transfer is left). Each SOF the handler reads `HPRT` (for
  the speed) and counts an interval down; when it reaches zero it starts the channel. **Without
  SOF interrupts only the very first poll happens and keys never arrive** (tested: with the SOF
  disabled in the model, exactly one interrupt IN started in 1.5 billion instructions). So SOF has
  to fire every 480,000 cycles (1 ms) while it is unmasked, and `HFNUM` has to advance with it:
  the firmware reads `HFNUM` (0x40080408) to pick the odd/even frame bit in `HCCHAR` (`ODDFRM`) when
  it starts each transfer. I did not need `HFIR` to do anything.
- Each poll: write `HCCHAR` (device address 1, endpoint 1, IN, interrupt, size 8, `CHENA`; the
  value was 0xA04E8808 or 0x804E8808, the top bit pair alternating with the frame) and `HCTSIZ`
  (`PKTCNT` 1, `XFRSIZ` 8, `DPID` DATA0 or DATA1). With nothing to report the device NAKs: the model
  sets `HCINT.NAK` (bit 4). The firmware answers by requesting a channel halt (`HCCHAR` |= `CHDIS`,
  after setting `CHH` in `HCINTMSK`), the model sets `CHH` (bit 1) and clears `CHENA`, and the firmware
  waits for the next interval. [scratch] The poll period was **6 ms for `bInterval` 10** and
  16 ms for 20; 4 gave 1 ms and 32 gave 1 ms. I do not understand why it is not 10 (the firmware's
  own interval arithmetic; the model's SOF was exactly 1 ms). It does not matter for the model,
  which just supplies SOF, but it means the key latency is a few ms and I should not promise the
  real board's 10.
- **Data toggle.** The firmware does not keep the toggle itself: it starts each poll with the
  `DPID` it reads back from `HCTSIZ` after the previous transfer completed (read at 0x0801015C).
  So after a data packet the model has to leave the next expected toggle in `HCTSIZ.DPID`. When I
  forgot, the firmware used DATA0 every time, which a toggling device would have rejected on
  alternate reports.

**From reports to `KEYDOWN()` and `INKEY$`** (all [scratch], real firmware, one report per
change, 100 ms apart, `PAUSE 100` loop printing the values):

| Report sent (keys) | `KEYDOWN(0)`, `(1)`, `(2)`, `(7)` |
|---|---|
| none | 0, 0, 0, 0 |
| a | 1, 97, 0, 0 |
| a, b | 2, 98, 97, 0 |
| b only | 1, 98, 0, 0 |
| left Shift + b | 1, 66, 0, 8 |
| released | 0, 0, 0, 0 |
| b, a (reversed in the report) | 2, 97, 98 |
| b, a, c | 3, 99, 97, 98 |

So key-up detection is just the next report no longer containing the key; modifiers come from byte
0 (left Shift is bit 1, and shows as 8 in `KEYDOWN(7)`); `KEYDOWN(0)` counts the non-modifier
keys. **The firmware lists the keys of the report from the last slot to the first**: the key at the
end of the report is `KEYDOWN(1)`. The manual says "in the order pressed", and a real keyboard
puts the newest key last, so that would make `KEYDOWN(1)` the newest, which I have not confirmed on
the board (open question 3). It matters only for how I order the report array: put keys in press
order, oldest first, like a keyboard does, and the firmware's choice carries over.

- `INKEY$` returned the typed character for a key that goes down, and **the firmware generates the
  repeat itself**: held `a` repeated about every 3 or 4 iterations of a 20 ms loop after a first
  delay of about half a second, with no further reports from the device. I did not measure it
  exactly; I take it to follow `OPTION KEYBOARD REPEAT` [guess]. At the prompt, a key-down report
  typed the character into the console. `KEYDOWN()` empties the console input buffer, as the
  manual says, so `INKEY$` after it returned nothing in my loop.
- With the idle rate set to 0 and boot protocol, a real keyboard answers NAK until something
  changes, so a held key is not re-sent. I rely on this, because the model sent exactly one report
  per change and nothing else, and the games' held-key behaviour came out right.
- Lock keys: `KEYDOWN(8)` returned 0 throughout; I did not press Caps Lock or Num Lock. The firmware
  keeps the locks itself, toggling on the key down, and shows them to the keyboard with SET_REPORT
  (the LED byte), which is how the window can tell what state it has them in (section 12).

## 5. The slave-mode mechanics the model needs

The smallest set I used, and the order the firmware expects. Offsets from 0x40080000.

**Registers the model has to compute** (everything else is plain storage, the stub as today):

| Register | Offset | Behaviour |
|---|---|---|
| `GAHBCFG` | 0x08 | store; bit 0 gates the interrupt |
| `GRSTCTL` | 0x10 | read 0x80000000 (`AHBIDL`; the manual's reset value, 57.14.5, p. 2627) plus `CSRST_DONE`, bit 29, which the manual lists as reserved and the firmware's other path waits for; a write of `CSRST` and the flush bits completes at once |
| `GINTSTS` | 0x14 | computed. Bit 0 `CMOD` always 1 (host). Bit 3 `SOF` latched each ms, write-one-to-clear. Bit 4 `RXFLVL` while the RX status queue is not empty. Bit 24 `HPRTINT` while `HPRT` has `PCDET`, `PENCHNG` or `POCCHNG` set. Bit 25 `HCINT` while `HAINT` is not zero. Bit 29 `DISCINT` latched, write-one-to-clear. Bits 5 and 26 (TX FIFO empty) set; they are masked |
| `GINTMSK` | 0x18 | store |
| `GRXSTSR`, `GRXSTSP` | 0x1C, 0x20 | peek and pop the RX status queue |
| `GNPTXSTS` | 0x2C | 0x00080200: non-periodic queue space 8, FIFO space 0x200 |
| `GSNPSID` | 0x40 | top 16 bits 0x4F54 (the low half is open); not in RM0433 |
| `GHWCFG2` | 0x48 | 0x028ED500 (a guess, see section 12): FS PHY, 12 host channels; not in RM0433 |
| `HFNUM` | 0x408 | frame number from the clock, one per millisecond, counting 0 to 0x3FFF (57.14.24, p. 2653); the top half non-zero |
| `HPTXSTS` | 0x410 | 0x00080100 |
| `HAINT` | 0x414 | bit n set while channel n has `HCINT` and `HCINTMSK` bits in common |
| `HPRT` | 0x440 | as in section 3; `PENA` is write-one-to-clear; `PCDET`, `PENCHNG` write-one-to-clear; `PSPD` 2 and `PLSTS` 2 for a low-speed device |
| Channel n | 0x500 + 0x20 n | `HCCHAR`, `HCSPLT` (store), `HCINT` (write-one-to-clear), `HCINTMSK`, `HCTSIZ`; `HCDMA` unused |
| FIFO windows | 0x1000 + 0x1000 n | write: TX data for the OUT transaction; read: pops the RX data FIFO (the firmware always read 0x1000 whichever channel) |

A channel number above the hardware's (0 to 11) can be ignored; the firmware used channels 0 and 1.

**GHWCFG2 and GSNPSID are guesses**: I never read them from the board. The firmware only needs
the fields above (the FS PHY type, 12 or more host channels, a recognised `GSNPSID`). The scratch
model answered 0x0002C100 for `GHWCFG2`, which has those fields and not much else (and not the
periodic support bit, which an interrupt endpoint needs); the core now answers 0x028ED500, the whole
register laid out as I remember the Synopsys core's (section 12). RM0433 does not document it, nor
`GSNPSID` or `GHWCFG1` to `GHWCFG4`: its register map (Table 523, pp. 2614 and 2615) goes from
`GCCFG` (0x38) and `CID` (0x3C) straight to `GLPMCFG` (0x54), with nothing at 0x40 to 0x50. So
there is nothing to check them against in the manual, and every field but the three the firmware
needs is a guess.

**An OUT or SETUP transfer** (the order the firmware does it):

1. `HCINT` write 0xFFFFFFFF; `HCINTMSK` write (0x4D9: `XFRC`, `STALL`, `NAK`, `NYET`, `TXERR`, `DTERR`;
   control IN transfers write 0x5B9, which adds `ACK` and `BBERR`, and interrupt IN 0x7B9, which adds
   bit 9 as well); `HAINTMSK` bit for the channel.
2. `GAHBCFG` read then write 0 (a critical section; it writes the old value back afterwards).
3. `HCTSIZ` write (`DPID` 3 for SETUP, 2 for DATA1; `PKTCNT`; `XFRSIZ`), `HCCHAR` write with `CHENA`.
4. Read `GNPTXSTS` for space, then write the data words to the FIFO window (a SETUP is two
   words: 0x01000680 and 0x00080000 for the first request).
5. The model completes the transaction when the data has arrived and sets `HCINT.XFRC` and `ACK`
   (0x21), tens of microseconds later. The interrupt arrives on line 101.

[guess] A real core sets `CHH` as well and clears `CHENA` when an OUT transfer completes. I did not,
and the firmware did not mind. A faithful model would.

**An IN transfer** (control data stage or interrupt IN): the same register setup (no FIFO write);
then the model pushes, for each packet the device returns, one RX status word and the data words:
`GRXSTSP` = `BCNT << 4 | channel | DPID << 15 | 2 << 17` (packet status 2, data received), then the
words, then at the end one status word `channel | 3 << 17` (transfer completed). The model also
updates `HCTSIZ`: `XFRSIZ` goes down by the bytes received, `PKTCNT` by the packets, and `DPID` to the
next toggle. Then `HCINT` gets `XFRC` and `ACK`. A short packet ends the transfer early, with
`XFRSIZ` and `PKTCNT` left non-zero: I saw this with the 4-byte language-ID answer to a 255-byte
request and the firmware coped. The firmware's handler pops the RX status (it masks `RXFLVL` while
doing so), reads the data from the FIFO window, handles `XFRC` and then **halts the channel**
(`CHDIS | CHENA` in `HCCHAR` with `CHH` unmasked): the model sets `CHH` and clears `CHENA`.

**NAK** on an interrupt IN: `HCINT.NAK`. The firmware halts the channel (as above) and starts it
again at the next interval. **STALL** (`HCINT` bit 3) is how an unsupported control request is
answered. Packet status 5 (toggle error) and 7 (channel halted) never occurred.

**Timing of the replies.** I delayed `XFRC` by 30 to 100 µs after the transfer started and `NAK` by
300 µs. I did not tune these and nothing depended on them. Real transfers at low speed take
longer; the firmware does not look.

**Interrupts and the clock.** `Irqs` is edge-triggered, and the firmware's ISR re-reads everything, so
the model raised 101 after every event and after every write that could leave the line active
(`GINTMSK`, `GAHBCFG`, `HCINT`, `HCINTMSK`, `HPRT`, `HCCHAR`, `GRXSTSP` pops), like `usart.rs` does.
I used `clock.wake_at` for the 1 ms SOF, for the transfer completions and for the reset
and attach delays. **One bug of mine worth noting:** `Clock::take_due` forgets the wake once due, so a device
that has several events in the future has to ask again for the earliest remaining one at the end
of its `tick`. I did not at first; transfers then completed up to a millisecond late (at the next
SOF tick), and the poll period looked like 13 ms instead of 6. The enumeration still worked,
which is why I noticed so late.

## 6. A mouse, and one device with both

[scratch] A low-speed HID boot mouse (3 buttons and a wheel, 4-byte reports) enumerated with the
same steps, plus one more: after the report descriptor the firmware sends **SET_PROTOCOL 1
(report protocol)** as well. It printed `USB Mouse Connected on channel 2 (low speed)` and `its
reports: X 8 bits at bit 8, Y 8 bits at bit 16, a wheel`. Scripted reports moved the pointer:
`DEVICE(MOUSE 2, X)` and `Y` started at 400 and 300 (the centre of the 800 by 600 screen), a
report with dx = 5 and dy = 5 gave 402 and 302 (so the default sensitivity seems to divide by 2
[guess]), the right
button and the wheel showed in `R` and `W`.

I also tried **one low-speed device with two HID interfaces** (a keyboard on endpoint 0x81, a boot
mouse on 0x82, one configuration of 59 bytes, two interrupt channels). It enumerated, both
interfaces got their SET_IDLE, SET_PROTOCOL and report-descriptor requests (the report descriptor
request carries the interface number in `wIndex`), both messages were printed
(`UK USB Keyboard Connected on channel 1`, `USB Mouse Connected on channel 2 (low speed)`), and
`KEYDOWN()` and `DEVICE(MOUSE 2, ...)` both worked in the same program.

This matters because **the firmware refuses a low-speed device behind a hub** (manual: "USB
low-speed device behind a hub is not supported"), so keyboard and mouse together on the real board
need either a full-speed hub with full-speed devices or one device that is both. A composite
device avoids modelling a hub, and the firmware handles it.

## 7. What the window has to supply

- **SDL scancodes, not keycodes or text.** SDL's scancode numbers for the main keyboard are the
  USB HID usage numbers (A is 4, Return 40, F1 58, left Ctrl 224, right GUI 231). So the mapping is
  nearly the identity. The firmware turns usages into characters with its own layout (`OPTION
  USBKEYBOARD`), as it does for a real keyboard. This means **the host's layout no longer matters,
  but the emulated one has to match the physical keys the user presses**: someone with a German
  keyboard who leaves `OPTION USBKEYBOARD UK` will type `z` where they see `y`. The window's current
  text input path (which applies the host layout) goes away for USB.
- Key down and key up events, ignoring SDL's repeat events (the firmware repeats). Held-key state:
  an ordered list of up to six usages, oldest first, plus the modifier byte (usages 0xE0 to 0xE7
  are bits 0 to 7). Send a report only when the state changes. On window focus loss, release
  everything. More than six keys: ignore the extra (a real keyboard sends a roll-over error).
- Deliver the **current state at the first poll** after enumeration, not just changes: a key held
  during the 2.78 s before the first poll would otherwise be lost.
- **Before the keyboard is usable** (about 30 s of wall time today), the window can keep sending bytes
  to the UART. I suggest: bytes to the UART until the firmware has started polling the keyboard
  (the model knows: the first interrupt IN), HID after. Keep the UART path for `--console` in any
  case; it is how every existing test types.
- The keyboard should be attached only when there is a window (or a flag), so that no existing
  console test sees the extra `Connected` line. With the model present and no device, boot is the
  same as today (measured: 1.2 billion instructions, 20.0 s without the model, 20.3 s with it
  and a keyboard).
- Mouse: SDL relative motion to dx and dy (clamped to -127 to 127, split over several reports if
  larger), the three buttons, and the wheel; the pointer is captured by the window when the
  program wants one. Gamepad: later.

## 8. The gamepad (not done)

[scratch] A generic HID gamepad (two 8-bit axes and 8 buttons, vendor and product 0x0079:0x0011)
enumerated and the firmware printed `Generic Gamepad Connected on channel 3 (vid &H79, pid
&H11)`. Reports were delivered, but `DEVICE(GAMEPAD 3, B/LX/LY/RX/RY)` stayed 0. [manual] says
the firmware recognises DualShock 3 and 4, Switch Pro, Xbox 360, Xbox One and Series, and "a
number of generic gamepads", and that **`GAMEPAD CONFIGURE` describes a generic gamepad it does not
recognise**; so I take it that my invented pad was not in its list [guess]. I did not try
`GAMEPAD CONFIGURE`. Xbox pads are vendor-class devices, not HID, so they would need their own
protocol; the DualShock 4 is HID but has its own report layout and output reports. Which one to
emulate, and what the firmware asks of it, needs its own spike.

## 9. The descriptors I used (for the keyboard)

Device (18 bytes): `12 01 10 01 00 00 00 08 34 12 78 56 00 01 00 00 00 01` (USB 1.10, endpoint 0
size 8, VID 0x1234, PID 0x5678, one configuration).

Configuration (34 bytes): `09 02 22 00 01 01 00 A0 32` then interface `09 04 00 00 01 03 01 01 00`
(HID, boot subclass, keyboard protocol), HID descriptor `09 21 11 01 00 01 22 3F 00` (one report
descriptor of 63 bytes), endpoint `07 05 81 03 08 00 0A` (interrupt IN, 8 bytes, every 10 ms).

Report descriptor (63 bytes, the standard boot keyboard):
`05 01 09 06 A1 01 05 07 19 E0 29 E7 15 00 25 01 75 01 95 08 81 02 95 01 75 08 81 01 95 05 75 01
05 08 19 01 29 05 91 02 95 01 75 03 91 01 95 06 75 08 15 00 25 65 05 07 19 00 29 65 81 00 C0`.

For the mouse I used a 52-byte descriptor with three buttons, X, Y and wheel, in a 4-byte report.

## 10. Plan

**Where it fits.** The same pattern as the console and the SD slot:

- `crates/cmm2-core/src/usb.rs` (or `otg.rs`): `OtgRegisters`, wrapped in `WordAccess` like `Usart`,
  mapped in `cmm2_memory` at 0x40080000 with a 0x20000 window (the FIFO windows reach 0x1000 +
  0x1000 n). It takes `memory.clock()` and `irqs.clone()`. Always mapped, with no device attached it is
  silent (section 2).
- A small `UsbDevice` trait (control requests in, interrupt IN reports out), with a HID keyboard
  implementation, later a mouse, and a composite that holds several. The device and the host
  controller are separate so that the controller is tested with a scripted driver and the devices
  with plain byte tests.
- A shared handle (`UsbPort`, a cheap clone like `Console` and `SdSlot`) that `MemoryMap` hands out
  with `usb_port()`. The window calls `press(usage)`, `release(usage)` and so on; the model pulls
  the report when the firmware polls, so no wake-up is needed for input. `Irqs` and
  `clock.wake_at` for the SOF, the reset, the attach and the transfer completions.
- A system reset (`reset()`): the model goes back to its reset state and the device, being plugged
  in, attaches again after the firmware powers the port. I did not test a system reset in the
  scratch; the first-boot flow has one (at about 201 million instructions, before the USB stack
  runs at all, so it should be harmless).
- The window: `keys.rs` gets a scancode to usage path next to the console bytes; the report builder
  holds the key list and modifier byte.

**Rough size** (lines of Rust, including tests; my scratch model was about 700 without tests and
with a lot of logging): the controller and channels 500 and tests 400; keyboard device and
descriptors 150 and tests 100; handle and plumbing 80; window mapping and report builder 250
and tests 150; mouse 200; real-firmware integration tests 100 (they need the firmware and about
710 million instructions, 30 s). Gamepad: unknown until its spike.

**Ticket split** (as a suggestion):

- **5.2 OTG core, host init, connect.** The register model with the computed registers, the
  firmware accepts the core, ISR on attach, port reset and enable. Test: the firmware reaches
  `PCSTS`/`PRST` and does not hang. About 1 to 2 days.
- **5.3 Control transfers and a boot keyboard.** Host channels, FIFO windows, the RX queue, a
  control-capable device, interrupt IN with NAK and SOF. Test with the real firmware: the console
  prints `Keyboard Connected on channel 1`, hot unplug prints `Disconnected`. About 2 to 3 days. This
  is the bulk of the risk.
- **5.4 Key reports from the window.** Handle, scancode to usage, held keys, focus loss, routing
  decision (UART until polling starts), the unit tests, and an integration test with
  `KEYDOWN()` in a BASIC loop. About 2 days.
- **5.5 Mouse.** Composite keyboard plus mouse device (or a separate mouse), SDL motion and
  buttons, `DEVICE(MOUSE 2, ...)` test. About 1 to 2 days.
- **5.6 Gamepad spike, then the gamepad.** First find out which pad and what the firmware needs.
  Not estimated.

**Go or no-go.** Go for the keyboard and the mouse. I got to working `KEYDOWN()`, repeat and hot
plug in one session with a throwaway model, and every register the firmware touches is in this
document. The risks that remain are small and specific: the few values I guessed (below), the
fidelity of the model to real silicon where the firmware does not care but a future firmware might
(`CHH` after an OUT transfer, `GSNPSID` and `GHWCFG` contents), and the 30 s wait before the first
key (today's speed; the firmware's own delays at 480 MHz are about 2.8 s). No-go is not on the
table for the keyboard. The gamepad is the real unknown and should not be promised.

## 11. What I could not establish, and open questions

- **Real-board reads** (I did not touch the board, and none of these are in the traces): the
  contents of `GSNPSID` (0x40080040), `GHWCFG1` to `GHWCFG4` (0x44 to 0x50), `GUSBCFG` before the
  firmware edits it (probably 0x1440), `HCFG` before (probably 0x200), `GDFIFOCFG` (0x5C),
  `GRXFSIZ`, `HNPTXFSIZ` and `HPTXFSIZ` with the keyboard attached (expect 0x90, 0x002003E0 and
  0x03500090 from my trace), `HFIR` (expect 0x1176F with a low-speed keyboard), `HFNUM`. They would
  show whether my guesses for the identity registers match silicon and settle the `CSRST_DONE`
  path. `tools/cmm2_regs.py` is how I would read them.
- **The order of `KEYDOWN(1)` and `(2)`** on the real board when two keys are pressed in turn: I get
  the newest first from the scripted reports. `gr_keys.bas` on the board with `A` then `B` held
  would show it. If the board gives the oldest first, the real keyboard puts its keys in the
  report newest first and my report builder should do the same.
- **The poll period** (6 ms for `bInterval` 10 in the emulation): I do not know whether the real
  firmware is also 6.
- **Full-speed devices.** I never made the model report full speed. A wireless dongle or a hub is
  full speed. I expect `PSPD` 1, `HCFG` 1 (48 MHz) and `LSDEV` clear in `HCCHAR` [guess].
- **A real hub**: I did not model one; hubs need their own class requests and are not needed if the
  composite device is used.
- **Lock keys** (`KEYDOWN(8)`, the LED report) and **Caps Lock** behaviour: untested.
- **What `CHH` and `CHENA` do after an OUT transfer on real silicon**: guessed (section 5).
- **A system reset with a device attached**: not tested.
- **Why the second gap (1.03 s) exists**: I see it in the traces (no USB activity between the last
  control transfer and the first interrupt IN) but did not find which firmware wait it is.
- **The firmware's behaviour with a first-boot flash**: the keyboard question (`Keyboard type: 1=UK`)
  comes at about 198 million instructions, before USB has started (the port reset is at
  444 million), so a USB keyboard cannot answer it. The window has to use the UART for that.

## 12. What is built

Issues 50 and 51 are the first two steps of section 10: the OTG controller (`otg.rs`), the port and
the device trait (`usb.rs`), a low-speed HID boot keyboard (`usb_keyboard.rs`), and ignored tests
that boot the real firmware with the keyboard plugged in. The window drives the keys since issue 52 (below);
`KeyReport` is the shared report that it sets, and the core tests set it directly.

**What it is.** `Otg` is mapped at 0x40080000 for 0x20000 bytes, always, with IRQ 101. `UsbPort` is
a handle like `SdSlot` (`MemoryMap::usb_port()`): `attach(device)` and `detach()`, and the core
notices on its next tick, because a plug asks the clock for one at once. Plugging a device in over
another counts as pulling the first out. A device implements `UsbDevice`: `setup`, `out`, `in_token`,
`address` and `reset`, and answers `In::Data`/`Nak`/`Stall` and `Out::Ack`/`Nak`/`Stall`. The core
does the framing, the packet sizes and the toggles, so a device only knows its own protocol. The
core attaches nothing: the front end plugs the keyboard in, so that no console test sees the extra
`Connected` line. With nothing plugged in the firmware's USB init now runs (the id is valid) and the
boot still reaches the second prompt at the same instruction count as before (612 million, in
slices of a million), at the same speed.

**Where the model differs from the scratch one.**

- The RX FIFO is one queue of words, as on the chip: a status word, then for a data packet its
  words, so `GINTSTS.RXFLVL` stays set while the data is unread. The scratch model kept two queues.
- `HAINT` is worked out from `HCINT` and `HCINTMSK` of each channel, and `GINTSTS.HCINT` from
  `HAINT` and `HAINTMSK`. The firmware sets both masks, so it did not matter in the scratch model.
- `HFNUM` and `GINTSTS.SOF` are worked out from the clock: the frame is the cycle count divided by
  480,000, and `SOF` is set when the frame is not the one at which the firmware last cleared it. Nothing
  runs while `SOF` is masked. While it is unmasked the core asks the clock for a tick at each frame
  start, and asks again from the tick. No transfer needs `SOF` in this model: a transaction happens a
  fixed time after the channel is enabled, not in the frame the `ODDFRM` bit names.
- The port is powered by `HPRT.PPWR`: a device connects 200 us after the port is powered or the device
  plugged in, whichever is later (the scratch model did it 200 us after `PPWR`). A reset (`PRST`)
  resets the device when it starts, and the port is enabled 2 us after it ends. A power-down drops the
  connection.
- A detach sets `PENCHNG` (only if the port was enabled), clears `PCSTS` and `PENA`, latches
  `DISCINT`, and drops the RX FIFO, the staged FIFO data and the transactions waiting. Re-attaching is
  a full enumeration, and the firmware printed `USB Keyboard Disconnected` and then the connect
  message again.
- A transaction takes 100 us (about what eight bytes take at low speed). With no answer (no device,
  the port not enabled, a different address) the channel gets `TXERR`. A halt request (`CHDIS`) sets
  `CHH` and clears `CHENA`; a finished transfer, a NAK and a STALL leave the channel enabled until the
  firmware halts it, as the scratch model did.
- The packet status word carries the toggle of each packet, and the core toggles `HCTSIZ.DPID` after
  each packet that was acknowledged; a SETUP leaves DATA1. The core does not check the device's own
  toggle, so a device that did not toggle would go unnoticed.
- `GUSBCFG` resets to 0x1440 and `HCFG` to 0x200 (the guesses of section 2, which make the registers
  read what the board does after the firmware has edited them), not the SVD's values.
- `HFNUM.FRNUM` counts to 0x3FFF and starts again at 0, as the reference manual has it (57.14.24,
  p. 2653), not at 0xFFFF. The register resets to 0x3FFF there, and the model's frame number comes
  from the clock, so it reads 0 at the start; the frame counter reset (`GRSTCTL.FCRST`) is not
  modelled.
- The FIFO windows (0x1000 + 0x1000 n, Table 526, p. 2619: the window of channel n is written for
  an OUT channel and read for an IN one, and every read pops the one RX FIFO) take what they are
  given: a byte or halfword write adds only the bytes written (the data of a packet written with
  narrow writes arrives in the order written). RM0433 says only that the registers are to be
  accessed by words (57.13, p. 2614) and not what a narrower access does, and the firmware writes
  words; this is the one that loses nothing.
- `GRSTCTL.CSRST` resets the core, as RM0433 57.14.5 (p. 2629) gives it: it clears the
  interrupts and all the CSR bits except `GATEHCLK` and `STPPCLK` of `PCGCCTL`, `HCFG.FSLSPCS`,
  `DCFG.DSPD`, `DCTL.SDIS` and the whole of `GCCFG`, which the model keeps. The channels (all their
  registers and what is staged for them), the RX FIFO and the flags the core latched are cleared,
  any transaction waiting is dropped, the registers (`GINTMSK`, `GAHBCFG`, `HAINTMSK`, `HFIR` and so
  on) go back to their reset values except those, and it is over at once. HPRT's control and change
  bits are CSR bits that the manual does not list, so they are cleared too: the port loses its power
  (`PPWR`), a device behind it goes (a disconnect, which latches `DISCINT` after the reset has
  cleared it) and the firmware has to power the port again, which it does: it resets the core
  before it powers the port, so it boots as before (612 million instructions to the second
  prompt). (Before I checked the manual I had the port left alone through a core reset.)
  `TXFFLSH` drops the data staged for packets that have not gone, and a packet that was ready to go
  with it, in the FIFO `TXFNUM` names (57.14.5, p. 2628: in host mode 0 is the non-periodic FIFO,
  which control and bulk channels send through, 1 the periodic one, for interrupt and isochronous
  channels, and 0x10 flushes both; 2 to 15 are device mode only); an OUT transfer
  with no data has nothing in the FIFO and is left alone. `RXFFLSH` empties the RX FIFO.
- A system reset or the port losing its power (`PPWR` cleared with a device connected) resets the
  device too, as a chip reset or a power-down does to a device on the cable. It is what makes
  `KeyReport::polled()` false again until the firmware has restarted its USB stack, so the window's
  keys go back to the console in between (below).
- A NAK is reported as `HCINT.NAK` for every endpoint type, and the core does not retry it. For
  control and bulk transfers the core would, in DMA mode; in slave mode, which is what the firmware
  runs, the driver starts the channel again, and the firmware does: its interrupt handler (the one
  at 0x08010304, with the IN transfers' `NAK` branch) halts the channel on a NAK whatever the
  endpoint type, and the halt interrupt starts the transfer again. So the model leaves the transfer
  enabled, and a NAKed control IN is completed by the firmware's restart (tested). The keyboard
  never NAKs endpoint 0 anyway. I have not seen a device that does, so this is from reading the
  handler and not from a measurement.
- The keyboard clears a SET_ADDRESS that was not followed by its status stage when the next SETUP
  arrives, so the address it was asked for does not turn up later.
- The device answers a request for string 0 with a STALL, which the firmware tolerates. The keyboard
  also stalls the interrupt endpoint until `SET_CONFIGURATION`, and answers every request that is not
  one of those in section 3 with a stall.

**The guessed identity registers**, none of them read from the board:

| Register | Value | Meaning |
|---|---|---|
| `GSNPSID` | 0x4F54310A | the id the firmware checks for, and low half 0x310A (not above 0x4209) |
| `GHWCFG1` | 0 | every endpoint bidirectional |
| `GHWCFG2` | 0x028ED500 | slave mode only (bits 4:3), dedicated FS PHY (9:8), 6 device endpoints (13:10 = 5), 12 host channels (17:14 = 11), periodic support (18), dynamic FIFO sizing (19), queues of 8 (23:22 and 25:24 = 2); the OTG mode, multiprocessor interrupt (20) and token queue depth 0 |
| `GHWCFG3` | 0x040000E8 | 1024 words of FIFO, 19 bit transfer size and 10 bit packet count (the widths of `HCTSIZ`), OTG enabled |
| `GHWCFG4` | 0 | |
| `GNPTXSTS`, `HPTXSTS` | 0x00080200, 0x00080100 | room for 8 requests, 0x200 and 0x100 words |
| `GRSTCTL` | 0xA0000000 | `AHBIDL` and `CSRST_DONE`, so a core reset and the flushes are over as soon as they are asked for |

**Measured on the real firmware** (release build, a first boot with the questions answered over the
console, then the keyboard attached from reset). The cycle counts are the machine clock, 480 MHz, and
include the first boot:

| Event | Instructions | Cycle | Machine time |
|---|---|---|---|
| second prompt | 612 million | 1.038 billion | 2.16 s |
| `UK USB Keyboard Connected on channel 1` | 690 million | 1.213 billion | 2.53 s |
| first interrupt IN poll | 910 million | 1.707 billion | 3.56 s |

The connect message is exactly `UK USB Keyboard Connected on channel 1` and a line end. The gap of
1.03 s of machine time between the message and the first poll is the one of section 3. The speed
with the keyboard attached is the same as without it (54.5 million instructions a second over the 400
million after the prompt, measured both ways, twice). A key report set by the front end is seen by
the firmware within a few milliseconds of machine time: `KEYDOWN(0)` was 1 and `KEYDOWN(1)` 97 for
`a` held, 2 and `KEYDOWN(7)` 8 for left Shift with two keys, 0 after the release, `INKEY$` returned
`a`, and shift and `a` at the idle prompt typed `A`. A report that is set and set back between two
polls is never seen, which is also what a real keyboard does, so a front end should hold a key
until it has been polled (issue 52).

**The window's keys (issues 52 and after).** `cmm2-emulator` plugs a `HidKeyboard` into the port with `--window`
(unless `--console-keys`) and `usb_keys.rs` drives its `KeyReport` from SDL: scancodes are the usages
(0x04 to 0x65 and the modifiers 0xE0 to 0xE7), the report is the modifier byte and up to six keys in
press order, more than six is `0x01` in all six slots, SDL's repeats are ignored and a lost focus lets
go of everything. Three hooks in the core serve it: `KeyReport::polled()` is true from the first
interrupt IN on the keyboard's endpoint after `SET_CONFIGURATION` (and false again after a bus reset,
a system reset or the port losing its power), `KeyReport::leds()` is the LED byte the firmware set,
and `KeyReport::was_sent()` is true once the keyboard has handed over, or found the firmware already
had, the report as it now is.

**The report is the keys that are held.** Each poll the keyboard reports the keys that are down at that
moment, as a real keyboard does: the modifiers and up to six keys in press order, `0x01` in all six
slots with more than six down. A held key is in every report for as long as it is held, so the
firmware repeats it itself and `KEYDOWN()` sees it, and keys that are held together appear together.
(An earlier version queued a report for every change of the keys and sent one per poll. It could
not keep up with typing: a keystroke cost two reports of 25 ms each, so it drained about five
characters a second of host time unpaced, and it would have replayed a stall minutes late. That
version, and the one before it, which dropped a Shift as soon as its own press was sent, typed
`a` for a quick Shift and `a`, and `l` for `ll`, both of which the real firmware showed.)

The one thing added to a real keyboard, because the emulator is polled more slowly than real time: a
key that is let go before a poll has reported it stays in the report until one has, together with
the modifiers that were down when it went down, so a tap is never lost, Shift with a letter types the
capital however soon the Shift comes up, and Ctrl-C arrives. A tap is two polls, the key and then
the report without it, as on hardware. What cannot share a report waits for the next:
 - the same key again (it is reported, then let go for a poll, then reported again, so `ll` types two);
 - a key that went down with different modifiers than one still waiting (`a` and then a quick
   Shift with `B` are two reports, so `a` is not typed as `A`);
 - a seventh key at once (taps over six in one go are six and then the rest, in order);
 - everything after the first of these that waits, so the order is kept.
A modifier pressed after a waiting key is held back from the report with it, and appears in the next.
A modifier that was tapped before a key, and was up again before that key went down, is sent in a
report of its own ahead of it, and the key follows without it: the two were never held together,
and `KEYDOWN(7)` must not say they were while the key is held. A modifier tapped while a key is
held is in the report with it, as it was.
A key that is down when there are more than six is the rollover error, as before, and the keys
that were down in it are counted as told of. A key that was waiting and was let go before then was
not in that report, so it is not counted: it waits for its turn like any other tap. (This is a key
that went down with fewer than six others down. A key that goes down with six already down has no
slot, and is dealt with below.)

So typing faster than one key per poll can put two different keys in one report. They are both typed,
in the order they were pressed, as the firmware types every key that is in a report and was not in
the one before in slot order. Nothing a user types is lost this way. What can be lost: a key
pressed and let go while the firmware is not polling is kept and typed late, six at a time, when it
does (there is no bound on how many are kept; the firmware does not stop polling short of a reset,
which drops them); and a modifier that is tapped on its own and held back stays until the key before it
has gone. A key that goes down while six others are down is a seventh key, and a real keyboard sends
the rollover error (`0x01` in every slot) for as long as it is down, with no key in the report. A
seventh key that is let go while the error is the report is lost, as on a keyboard. One that is
still held when a slot frees is reported then, and typed, as a keyboard does.
The emulator is polled more slowly than real time, so the error is sent whenever a seventh key went
down with six others, until the firmware has had it once, whatever has come up since: a keyboard
polled while the seven overlapped would have sent it, so the seventh key is lost when it is let go
before then, even if the other six, or one of them, come up first. The firmware sees the error, then
the keys that remain, as it would from the keyboard. Keys that were down when the error went out, and
had not been in a report before, are not counted as seen by it: they are reported afterwards, and
typed, even if they come up before the next poll. Nothing is left waiting for the lost key, so
the window's lock following (below) is not held up by it. A Caps Lock or Num Lock is waited for
(the LED update the firmware answers it with) when it is first reported as a key, and not when it
goes down, so one lost to the error is not waited for and one held through it is. A key that was already waiting when the
seven were down, because it was tapped before the seventh key went down, was seen by the keyboard
alone and is not lost: it is typed once there is room.

Property tested (`usb_keys.rs`, 3000 random sequences of presses and releases of three keys and three
modifiers, with the firmware polling at random in between, and no more than one key going down
between two polls): a model of the firmware types exactly the keys that were pressed, in order, each
with the modifiers that were down when it was pressed. On the real firmware: a held `a` is
`KEYDOWN(1)` = 97 in every one of the readings 20 ms apart after the first (none lost in 40); a key
held at the prompt for 2 s of machine time typed 21 characters (the firmware's own repeat, with
MMBasic's defaults of a first delay of 500 ms and a repeat every 75 ms: 1 + (2000 - 500) / 75 = 21,
and the test asserts 19 to 22 so that the edges of the 2 s do not matter); a quick Shift and a
letter types the capital, Ctrl-C stops a program (also behind 40 taps), and `PRINT 1+2` is typed.

The speed, measured on the real firmware (release build) with everything pressed at once. The
machine times do not depend on the host, but the host times do: they were measured on an idle
machine, and a loaded one is slower:

| Burst | Polls | Machine time | Host time, unpaced |
|---|---|---|---|
| 25 different lowercase letters | 6 | 0.14 s | 0.63 s |
| 25 different letters with Shift | 6 | 0.15 s | 0.64 s |
| the same key 20 times | 40 | 0.99 s | 4.1 s |

The machine time is real time when the emulator is paced and keeps up; unpaced the host is 4 to 6
times slower here. So different keys go at about 180 a second paced and 40 unpaced, and the
same key twice in a row, which needs the key let go in between, 20 a second paced and 5 unpaced, which
is the limit for typing `ll`, `ss` and `oo` fast. The window also looks at the keys every 16 ms of
host time, and a report goes out at a look or an event: a quick sequence of keys one after another,
a key at a time, is one key per look at worst, 60 a second. None of this is below what a typist does.
Until `polled()` the window types console bytes, which the first-boot question needs; from then on it
sends reports only. A key held across the switch was typed on the console, so it is typed once and is
not in the report (its release is ignored and its next press is USB's); the modifiers are always in
the report. After a system reset, or the port losing its power, `polled()` is false again (the core
resets the device), the keys held and waiting are dropped, and the window is back to console keys until
the firmware starts polling again, which is also when a question asked before the USB stack is up can be
answered. Measured in the release build, with the keyboard attached from reset, the firmware polls
about 299 million instructions after the second prompt: 1.4 s of machine time, and 5.5 to 6 s of host
time unpaced.

**AltGr.** On the console path AltGr makes a character, which arrives as text, so Ctrl does not
make a control code when Right Alt is also down (Windows reports AltGr as Left Ctrl with Right Alt).
On Linux AltGr is Right Alt alone, so Ctrl with Right Alt and a letter makes no control code there
either; Ctrl with Left Alt does. On the USB path, on Windows only (`cfg(windows)`), a Right Alt that comes straight after a Left
Control (the Control the last key down) takes the Control back if the firmware has not been sent it yet:
the window handles the two events of AltGr in one batch with no machine time between, so the Control is
still waiting, and it is dropped as if it had not been pressed. A Control the firmware has had, or that
was not the last key down, is a real one. Off Windows a real Ctrl with Right Alt in one batch is what it
says. I have not tried it on Windows: it relies on Windows sending the Control and the Alt of AltGr as two
key downs in a row, which I remember and have not checked, and a window that gets the two in different
batches (a look between them) lets the firmware see a Control on its own, which is harmless.

**The lock keys.** The firmware keeps Caps Lock and Num Lock itself and sets the keyboard's LEDs
(`KeyReport::leds()`, bit 0 num lock, bit 1 caps lock). The key presses reach it, so the window and
the firmware stay in step once they agree, but not if the host's lock was already on when the window
opened or was toggled while the window had no focus. So the window reads the host's locks
(`SDL_GetModState`) and, the first time after the firmware starts polling and again whenever the
window gets the focus, taps the Caps Lock or Num Lock key if the firmware's LED does not agree with
the host's. It is one tap and not a loop, so a lock that the firmware never shows is tapped once and
left. The comparison waits until the LEDs are the firmware's: measured after
`OPTION KEYBOARD UK,1,1` and a restart (both locks on), the LEDs are 0 at the first poll and the
firmware's own SET_REPORT (both on) comes about 430,000 instructions later, so a comparison at the
first poll tapped both locks off. The keyboard counts the firmware's SET_REPORTs
(`KeyReport::led_updates()`) and its polls (`polls()`, both from the last reset), and the window waits
for the first SET_REPORT, or for 200 polls (1.2 s of machine time at 6 ms a poll) if the firmware never
sends one; after a lock key it waits for the next one, or the same 200 polls, so a lock key pressed
just before the focus came back is not tapped again; and it waits until no key is waiting to be sent and the report is sent, and while the user holds a lock key (the release would take the tap's key up). Measured: a
tap of Caps Lock sets bit 1 within a few milliseconds and a second tap clears it, and the next
letter is a capital; with both locks on at the host and the firmware, nothing is tapped. Num Lock
is handled the same way: a tap of it was measured (`real_firmware_shows_num_lock_on_the_leds_over_usb`), LED bit 0 is set and the firmware sends exactly one SET_REPORT for it. SDL does not report the host's Num
Lock on macOS (it reads as off), so there the window leaves Num Lock as the firmware has it; a
Wayland compositor that did not report it would turn the firmware's Num Lock off at each focus
change, and I do not know of one. Scroll Lock is not followed.

The mouse and the gamepad (issues after) need a composite device or a second device behind a hub,
which the port does not have.
