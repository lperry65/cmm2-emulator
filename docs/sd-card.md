# How the firmware drives the SD card

This is the spike for issue 42. I wanted to know what the firmware (v7.00.00b5) does to the SD
card so that I can model a card and its controller. The short answer is not what I expected:

**The firmware does not use the STM32's SDMMC controller at all.** It talks to the card in SPI
mode. By default it does that by wiggling GPIO pins in software ("bit-banging"); with
`OPTION SD TIMING FAST` it uses the SPI3 peripheral instead. The SDMMC1 and SDMMC2 windows
(0x52007000 and 0x48022400) are never touched, and the code never even forms those addresses.
So the smallest sufficient model is an SPI-mode card plus the GPIO pins, with SPI3 as a second
step, and not an SDMMC model with a card behind it.

## How I found this

- With the real firmware and a flash file that has been through first boot, I ran to the prompt
  (about 411.5 million instructions) with `--trace-mmio`, and typed commands with `--console
  --on`. No access to the SDMMC windows appeared in any run, and `RCC_AHB3ENR` (SDMMC1EN is bit 16)
  and `RCC_AHB2ENR` (SDMMC2EN, bit 9) were never written with those bits.
- I disassembled the `.bin` (linear Thumb-2 disassembly, read by me; no source was used or copied).
  No literal pool holds 0x52007000 or 0x48022400, and there is no `movw`/`movt` pair that builds the
  SDMMC1 address. It does hold GPIOC, GPIOD and GPIOH base addresses next to a driver whose
  structure is the familiar one for a FatFS disk layer over an SPI card (select, send command,
  wait ready, receive data block, transmit data block, and a 1 kHz timer tick that polls the
  card-detect and write-protect pins).
- To see what happens after the first command, a card that never answers is no use, so I built a
  scratch copy of the repo outside the tree (in a `mktemp -d` directory) with two throwaway
  additions: a byte-level SPI-mode SD card driven from the GPIO bit-bang pins, and a minimal SPI3
  register model with a hook into the same card. I ran the real firmware against a 64 MB FAT32
  image made with `mkfs.fat`. Both additions were deleted with the scratch copy; nothing of it is
  in the repository. Everything below that says "observed" comes from those runs; the rest is
  marked as read from the disassembly.

## 1. Controller, pins, clocks

The card slot is the full-size SD socket (`SDCard`, DM1AA-SF-PEJ) in the Gen 2 schematic. The
pins on it that go to the STM32 are:

| Socket pin | STM32 pin | SPI-mode meaning | Use in the firmware |
|---|---|---|---|
| 1 | PD2 | CS | GPIO output, driven from `GPIOD_BSRR` (bit 2 set = deselected) |
| 2 | PC12 | MOSI (DI) | GPIOC bit-bang output, or SPI3_MOSI (AF6) |
| 5 | PC10 | SCK | GPIOC bit-bang output, or SPI3_SCK (AF6) |
| 7 | PC11 | MISO (DO) | GPIOC input read from `GPIOC_IDR` bit 11, or SPI3_MISO (AF6) |
| 10 | PD3 | card-detect switch | input with pull-up, low = card in |
| 11 | PH12 | write-protect switch | input with pull-up, high = protected |

Socket pins 8 and 9 are drawn with no-connect marks. I did not trace pins 3, 4 and 6 on the
drawing (they should be the supplies in SPI mode, but I did not check). The pin order (CS, DI, CLK,
DO) is the SPI-mode one, and it is why SDMMC1's PC8/PC9 (D0/D1) are not on the socket.

Register values I observed at boot, before the first prompt:

- Default (bit-bang): `GPIOC_MODER` ends at 0xFD1FFFFE (PC10 and PC12 outputs, PC11 input),
  `GPIOC_PUPDR` 0x00400000 (pull-up on PC11), `GPIOC_OSPEEDR` 0x02200001, `GPIOC_OTYPER` 0.
  `GPIOD_MODER` 0xAFEAEF1A (PD2 output, PD3 input), `GPIOD_PUPDR` 0x40 (pull-up on PD3).
  `GPIOH_MODER` 0xB8EAABAF (PH12 input), `GPIOH_PUPDR` 0x01000000 (pull-up on PH12). `AHB4ENR` has
  GPIOC, GPIOD and GPIOH on already (final 0x100001FF).
- With `OPTION SD TIMING FAST`: the same, except PC10 to PC12 are alternate function:
  `GPIOC_AFRH` 0x00066600 (AF6 on pins 10, 11 and 12) and `GPIOC_MODER` 0xFEAFFFFE. The HAL pin
  setup function in the firmware passes alternate function 6 for those three pins on SPI3.
  `RCC_APB1LENR` bit 15 (SPI3EN) is set just before the first SPI3 register access, and not in
  default mode. I take AF6 on PC10 to PC12 to be SPI3 from the datasheet's alternate function
  table; I have not got the table here, I only saw the firmware use AF6 with SPI3.
- SPI3 kernel clock: `RCC_D2CCIP1R` is written 0, so `SPI123SEL` = 0. As I remember RM0433, that
  selects PLL1 Q (the SVD only calls the field "kernel clock source selection"; I have not
  re-checked the manual). `RCC_PLLCKSELR` 0x12 and `RCC_PLL1DIVR` 0x01130277 give, from the SVD
  field layout: HSE source, DIVM1 = 1, DIVN1 = 0x77 (N = 120), DIVP1 = 1 (/2), DIVQ1 = 0x13
  (/20), DIVR1 = 1 (/2). That is 8 MHz x 120 = 960 MHz, 480 MHz on P and 48 MHz on Q. So SPI3's
  kernel clock is 48 MHz (assuming the SPI123SEL mapping above).
- SPI3 `CFG1` is 0x700001E7 during card init and 0x000001E7 after it: `MBR` = 7 then 0,
  `DSIZE` = 7 (8-bit frames), `FTHVL` = 15, no CRC, no DMA bits. From RM0433 (not checked here)
  MBR 7 is /256 and 0 is /2, which would be 187.5 kHz for init and 24 MHz after. `CFG2` is
  0x84400000 (`AFCNTR`, `SSM`, `MASTER`; CPOL = CPHA = 0, MSB first, full duplex), or 0x84420000
  (`COMM` = 01, transmit only) when a command or data block is sent without caring about the reply.
  `CR1` is 0x1000 (`SSI`) when idle, with `SPE` and then `CSTART` (bit 9) set per transfer; `CR2`
  `TSIZE` holds the byte count (1 for a single exchange, 6 for a command, 0x200 for a block).
- Bit-bang timing, measured in instructions: in the slow phase (until the card is initialised)
  one byte costs about 48,000 instructions (two software delays of 10 units per clock, the delay
  routine at 0x0809ADC8). The firmware then sets a flag (0x20015FA0) and the same routines run
  without the delays: 69 instructions a byte, about 36,000 for a 512-byte block.

## 2. When it starts, and what MMIO comes first

- **Nothing happens at boot.** The pins are set up before the prompt (above), but no card
  traffic occurs until the first pass of the main loop after the first prompt. In an idle run
  (no keys typed) the first CS-low is at instruction 412,140,585, 0.58 million after the prompt
  (411,563,043). It is one attempt, in the slow phase, and it is not repeated while idle.
- With a card that answers, the whole init plus the first mount reads (sectors 0 and 1) is done
  by 417.3 million: about 5.1 million instructions, nearly all of it the slow bit-bang.
- After that, each command that needs the card goes to FatFS, which asks the disk layer for the
  card's status; the status byte (0x2000039B: bit 0 not initialised, bit 1 no disk, bit 2
  protected) is updated by the 1 kHz tick function at 0x0809F3A8, which reads `GPIOD_IDR` bit 3
  and `GPIOH_IDR` bit 12 each time (I measured gaps of 214,000 to 240,000 instructions between
  reads, about 1 ms at the clock model's 2.2 cycles an instruction). A card is initialised again
  only if the status says it is not (no card at the earlier attempt, or card-detect went high).
- The first MMIO a card command causes is `GPIOC_IDR`/`GPIOD_IDR` reads, then `GPIOD_BSRR` (CS
  low) and a stream of `GPIOC_BSRR` writes (bit 10 clock, bit 12 MOSI) interleaved with
  `GPIOC_IDR` reads (bit 11 MISO). In FAST mode it is `SPI3_*` instead of `GPIOC_BSRR`, with
  `GPIOD_BSRR` for CS in both.
- **No interrupts and no DMA.** `SPI3_IER` is only ever written 0 (98 writes in the boot and
  first mount), no `DMA`, `DMAMUX` or `MDMA` register is touched, `CFG1.TXDMAEN`/`RXDMAEN` stay 0,
  and no NVIC enable is written for the card. SPI3's interrupt (IRQ 51 in the SVD) is unused.
  Everything is polling. In FAST mode the loop reads `SPI3_SR` (about 3,500 reads in the boot and
  mount, about three per byte), writes `SPI3_TXDR` (1109 byte writes plus 11 word and 11
  halfword writes: the six-byte command frame goes as 4 + 2 bytes), reads `SPI3_RXDR`, then
  writes `SPI3_IFCR` with 0x08 (`EOTC`) and 0x10 (`TXTFC`) and clears `SPE`. HAL timeout is 500
  ticks.
- Bus width: not applicable (one data line). There is no `WIDBUS`.

## 3. The command sequence

This is what I observed from mount of a 64 MB SDHC-style card (CMD frames are 0x40 | index, four
argument bytes, a CRC byte; the reply is polled with 0xFF bytes):

| Step | Frame | Reply | What the firmware does with it |
|---|---|---|---|
| 80 clocks | 10 bytes of 0xFF, CS high | none | wake-up |
| CMD0 | arg 0, CRC 0x95 | R1, must be 0x01 | anything else: init fails |
| CMD8 | arg 0x000001AA, CRC 0x87 | R1 0x01, then 4 bytes | R7 byte 2 must be 0x01 and byte 3 0xAA, else it takes the version-1 / MMC path |
| CMD55 + ACMD41 | arg 0, then arg 0x40000000 | R1 | repeats until R1 is 0, or 1000 ticks pass |
| CMD58 | arg 0 | R1 0x00, then 4 bytes OCR | bit 30 of the OCR (CCS) set: block addressing, type 12; clear: byte addressing, type 4 |
| CMD17 | arg 0, then arg 1 | R1 0, token 0xFE, 512 bytes, 2 CRC | boot sector, then FSInfo, by FatFS's mount |
| (then) CMD17 | arg = sector | same | directory and file reads |
| CMD24 | arg = sector | R1 0, then token 0xFE, 512 bytes, 2 bytes 0xFF | data response `& 0x1F` must be 5, then busy (0x00) until 0xFF |

In bit-bang mode the CRC byte is 0x95 for CMD0, 0x87 for CMD8 and 0x01 for everything else. In
FAST mode (SPI3) the firmware sends real CRC7 values (CMD55 0x65, ACMD41 0x77, CMD58 0xFD,
CMD17 for sector 0 0x55); the card never has to check them. The data CRC16 is sent as 0xFF 0xFF and
ignored on receive, and my card never sent a correct one either.

Observed totals for the commands I ran against my scratch card: `DRIVE "A:"` (nothing beyond mount),
`FILES` (one CMD17 of the directory sector; the first listing was `0x802` on that image),
`OPEN ... FOR OUTPUT` / `PRINT #` / `CLOSE` (a CMD17 and a CMD24 for each of the directory entry,
the data sector, and both FAT copies, all single block), `SAVE IMAGE "x.bmp"` (single-block
writes only, 66 CMD24 for 14,204 bytes), `COPY`, `LOAD IMAGE`, `RUN "t.bas"`, `EDIT "t.bas"`
(reads the directory sector and the file sector), `MKDIR`, `KILL`, `MM.INFO(DISK SIZE)` and
`MM.INFO(FREE SPACE)` (hundreds of CMD17 for the FAT scan; 66,059,264 bytes on that image),
`MM.INFO$(SDCARD)` ("Ready" with a card, "Unused" without) and `OPTION LIST`. The set of commands
the card ever saw: CMD0, CMD8, CMD55, ACMD41, CMD58, CMD17, CMD24.

Read from the disassembly, and **not observed** because nothing I typed reached them:

- CMD18 (multi-block read, followed by CMD12 stop), CMD25 (multi-block write, with ACMD23 first
  and the 0xFC / 0xFD tokens). The disk layer uses them when FatFS asks for more than one sector.
  Every file operation I ran went a sector at a time, including a 300 KB file written and read
  back and a 14 KB image saved and loaded.
- CMD9 (CSD), CMD10 (CID), CMD58 (OCR) and ACMD13 (the 64-byte SD status), in the
  `disk_ioctl` function at 0x0809F88C, which handles the commands FatFS defines (sync, sector
  count, block size, and the ones that read the CSD, CID, OCR and SD status). Capacity comes from
  the CSD there, with a version-1 and a version-2 layout. No command I ran called it, because
  FatFS gets the size from the boot sector. It is probably reached only by a format.
- The version-1 SD and MMC fallbacks (ACMD41 arg 0, CMD1, CMD16 arg 512).
- No CMD6 (high speed), no voltage switching (CMD11), no ADMA-style anything, and no CMD13, CMD59,
  or CMD23. The firmware never reads the SCR or sets a bus width.

## 4. Timeouts, and "SD Card not found"

Observed against a card that never answers (the line stays high, which is what the pull-up on PC11
gives):

- CS goes low, two 0xFF bytes, CMD0 (0x40 0 0 0 0 0x95), then 100 reads of 0xFF, then CS high.
  That is one attempt, about 5.2 million instructions (nearly all of it 100 slow bytes).
  The 100 is the loop counter in the send-command routine: it waits at most 100 bytes for a reply
  whose top bit is clear. No reply means the init function returns "not initialised".
- FatFS turns that into result 3 (not ready). The firmware keeps its own table of error text
  indexed by that result. Entry 3 is "SD Card not found" and entry 20 is "SD card not present".
  I confirmed both by running:
  - card-detect low (PD3 low, as the model has it today) and no card answering: "SD Card not found".
  - PD3 forced high (scratch run): "SD card not present", with no card traffic at all (no CS, no
    clock).
  So the message you get today is: `GPIOD_IDR` bit 3 clear (card in), `GPIOC_IDR` bit 11 never
  low (nothing answers), so CMD0 times out.
- Write-protect: with `GPIOH_IDR` bit 12 set, the first write is refused with "The physical drive
  is write protected" and no CMD24 is sent.

Read from the disassembly, not exercised because my card always answered at once: the other
timeouts are all in ticks of the 1 kHz function: waiting for the card to be ready before a command
(500), for the data token of a read (100), for the ready after a write (500), and the ACMD41 loop
(1000). A model that answers at once never hits them.

Two oddities I could not explain, which are probably not about the card:

- The first error message after boot is wrong. With no card, the first `FILES` prints an empty
  line and the later ones print "SD Card not found"; a first `NEW` prints `Error:1` (with or
  without a card). I saw `Error:1` once after the first `NEW` or `FILES` in every run, and not
  after a first `PRINT`. I did not find the cause and I do not think the SD driver is involved,
  since it appears with no card and with a perfect one.
- The firmware has exFAT strings ("exFAT not supported" and a FAT32 and EXFAT type name), so
  large cards matter to FatFS, but the card layer itself sees only sector numbers.

## 5. What the model has to implement

The smallest sufficient subset for FatFS-style access with the default settings:

1. **GPIO side.** Port C pins 10, 12 and port D pin 2 as outputs the card watches, port C pin 11
   as an input the card drives, port D pin 3 (card-detect, low = in) and port H pin 12
   (write-protect, high = protected) as inputs. The existing GPIO model already gives PD3 low and
   PH12 low from the board's measurements; those fixed bits become the card's state. Mode 0 SPI:
   data is sampled on the rising edge of PC10, MSB first. The card changes its output on the
   falling edge, and the firmware reads `GPIOC_IDR` while the clock is high.
2. **A card state machine in SPI mode**, byte at a time: idle until a byte with 0b01 in the top two
   bits, collect six bytes, pad with one 0xFF of reply latency, and answer. Required: CMD0 (R1 =
   0x01), CMD8 (R1 0x01 then 00 00 01 AA), CMD55/ACMD41 (R1 0x01 at least once, then 0x00),
   CMD58 (R1 0, OCR with bit 30 set, 0xC0FF8000 worked), CMD17 (R1, 0xFE, 512 bytes, two
   CRC bytes) and CMD24 (R1, accept token 0xFE, take 512 + 2 bytes, answer 0xE5 and some 0x00
   busy bytes). CMD18/12 and CMD25 with the 0xFC and 0xFD tokens are cheap to add and are the
   right next step because the firmware has the code for them. An unknown command should get R1
   with the illegal-command bit (0x04).
3. **CS (PD2) high** drops any command in progress and makes MISO read 0xFF.
4. **SPI3, for `OPTION SD TIMING FAST`.** `CR1` (`SPE`, `CSTART`), `CR2.TSIZE`, `CFG1`, `CFG2`
   (stored), `SR`, `IFCR` and `TXDR`/`RXDR` at 8, 16 and 32 bits. Instant completion works: each
   byte written to `TXDR` exchanges one byte with the card and queues the reply; `SR.TXP` set while
   the transfer has bytes left, `SR.EOT` set once `TSIZE` bytes have gone, `SR.RXWNE` and `SR.RXPLVL`
   from the receive queue length (the HAL reads 32 bits when four bytes are there, 16 for two
   or three, 8 for one), `SR.TXC` set; `IFCR.EOTC` clears `EOT`; `CR1.SPE` clear resets it.
   That was enough for the firmware to mount and list in my scratch model. The SPI3 hardware-
   rate difference does not matter to the card.
5. **CSD, CID, SCR:** the firmware parses none of them in anything I ran. Return a version-2 CSD
   (CSD_STRUCTURE 1, `C_SIZE` such that (C_SIZE + 1) x 1024 is the sector count, TRAN_SPEED 0x32 or
   0x5A) for CMD9 so that a format or a size query is right, and an arbitrary CID for CMD10. I did
   not make anything read the SCR.

Surprising, and worth knowing before starting:

- It is not SDMMC, so no data timer, no `DCTRL`/`DLEN` and no `IDMA` registers, and no
  voltage switching, high speed, or linked-list DMA to build.
- Default mode needs no peripheral at all other than the GPIO ports: the cost is the 48,000
  instructions a byte in the slow phase. The firmware keeps checking the 1 kHz tick for card-detect
  and write-protect; the model must keep PD3 and PH12 consistent with the card state.
- A card that is not answering costs 5.2 million instructions per attempt, and the firmware retries
  at each command that needs the card.
- Capacity, speed class and CSD are never read in normal use; the FAT volume decides everything.
  The card can be as large as the image: SDHC addressing (CCS set in the OCR) means CMD17/24 take
  a sector number, and that is what the firmware will do for any OCR with bit 30 set.

## 6. Where it goes in the code (recommendation)

- **A shared card handle**, like `console()` and `flash_ops()`: a new `sd.rs` with a cheap `SdCard`
  handle (an `Rc<RefCell<..>>`) holding the card state machine, the image and the two switches
  (inserted, write-protected). `MemoryMap` gets an `sd_card()` accessor next to `console()`. The
  front end owns the other end: it chooses the image (a `--sd FILE` option) and writes it back
  when the run ends, the same way `--flash` does, through a temporary file and rename. The core
  keeps the image as bytes and a dirty flag; it does no file I/O, so it stays free of the host.
- **The card is byte-level and has no clock.** It exposes `select(bool)`, `exchange(mosi) -> miso`
  and, for the bit-bang, the byte-assembling shim below. Instant completion is right: nothing
  in the firmware depends on how long the card takes, only on the replies and on the order of
  bytes (the firmware's own ticks run on the existing clock model and it simply sees the card
  answer at once).
- **Bit-bang goes through the GPIO model.** `Gpio::new(port)` builds each port in a loop in
  `machine.rs`; ports C, D and H need the handle. On a `BSRR` or `ODR` write the port tells the
  handle the new output levels; on an `IDR` read for port C, pin 11 comes from the card (the
  `input_level` / `HELD_LOW` / `AF_HIGH` logic stays for all other pins). PD3 and PH12 come from the
  handle's switches instead of the `HELD_LOW` bits. The handle watches PC10 for rising and
  falling edges, assembles a byte from PC12, and picks the reply byte at the first rising edge of each byte
  (the card never needs the byte being sent to decide what it answers) and drives PC11 with it.
- **SPI3 is a separate `Peripheral`, not a `Registers` model.** `Registers::read` has no access
  size, and `RXDR` has to pop one, two or four bytes depending on the access, so it must
  implement `Peripheral` directly (`read(offset, size)`). Map it at 0x40003C00 before
  `map_unmodelled_peripherals`, share the same `SdCard` handle, and leave the other SPI windows as
  stubs. The size of a `TXDR` write gives the number of frames.
- **Interrupts and `Irqs`:** none needed (the firmware polls, `IER` is 0). If IRQ 51 is wanted
  later, raise it through `Irqs` from the SPI3 model when `IER` has a bit set and the flag is up.
  There is no DMA to emulate, so nothing needs the clock's `wake_at`.
- **Tests:** feed the card the byte sequences in section 3 and check the replies (unit tests in
  `sd.rs`); a GPIO test that clocks a CMD0 out of PC10/PC12 and reads R1 from PC11; and an
  ignored real-firmware test that boots with a small FAT image and checks that `FILES` lists it.
  The boot to the prompt is about 412 million instructions, roughly ten seconds in a release build.

## 7. What I could not establish

- The first-error-message oddity and the `Error:1` text (section 4).
- Whether any command at the prompt reaches CMD18, CMD25, CMD9, CMD10 or ACMD13. I tried the
  common ones. A format (not found in the command list) or a very large sequential read or write
  may be what triggers them.
- Which SPI3 `SR` values the real silicon returns during a transfer. My model's values were
  enough for the HAL's loops, but I have not seen a real board's `SR`.
- The pin alternate function number (AF6) and the baud divider (`MBR`) and kernel clock selection
  (`SPI123SEL` = 0) are read against RM0433 and the datasheet from memory, not checked here.
- The exact point in the main loop that starts the first mount; I only know it is the first
  idle pass after the prompt, reached through the call chain 0x0809EA2C, 0x0809C502, 0x0809AC6C,
  0x0809A4F4, 0x08042880, 0x080427C6, 0x080933A2 and 0x08091E92 into the init function at 0x0809F568.
- Behaviour with an MBR-partitioned card or an exFAT card: my image was FAT32 with no partition
  table. FatFS will read sector 0 as an MBR and then the partition's boot sector; the card side is
  the same CMD17.
- The Gen 2 pin 3, 4 and 6 connections on the socket (not traced).

## 8. What is built

Issue 43 is the first step of section 6: the byte-level card (`sd.rs`), the socket with the
bit-bang pins (`sd_slot.rs`, read and driven by the GPIO ports C and D), the `--sd` option, and an
ignored test that boots the real firmware on a FAT32 image from `mkfs.vfat`, lists it, saves a
program with `AUTOSAVE`, lists and runs it back and finds the file in the image.

- The card answers CMD0, CMD8, CMD55/ACMD41, CMD58, CMD17 and CMD24, with R1 `0x04` (illegal
  command) for the rest, and R1 `0x40` for a block past the end. CMD9, CMD10, CMD18/12, CMD25 and
  ACMD23 are not there, as nothing I ran needed them. SPI3 (`OPTION SD TIMING FAST`) is not there.
- PD3 reads low with a card in the socket and high without, and then the firmware says "SD card
  not present" without sending anything to the card. PH12 still reads low, as the board does, so a
  card is never write protected.
- Measured on the real firmware: the first command that reaches the card is 5.9 million
  instructions, nearly all of it the slow bit-bang of the initialisation; a `FILES` after that is
  about 0.4 to 0.5 million, an `AUTOSAVE` that writes a file about 6 million and a `RUN` of it
  about 4 million.
