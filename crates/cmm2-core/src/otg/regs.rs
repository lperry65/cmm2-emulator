//! The registers of the controller: where they are in the window, what their bits are, and the
//! values they hold after a reset.

// Register offsets from the start of the window, as the SVD gives them.
pub(super) const GOTGINT: u32 = 0x04;
pub(super) const GAHBCFG: u32 = 0x08;
pub(super) const GUSBCFG: u32 = 0x0C;
pub(super) const GRSTCTL: u32 = 0x10;
pub(super) const GINTSTS: u32 = 0x14;
pub(super) const GINTMSK: u32 = 0x18;
pub(super) const GRXSTSR: u32 = 0x1C;
pub(super) const GRXSTSP: u32 = 0x20;
pub(super) const GNPTXSTS: u32 = 0x2C;
pub(super) const GSNPSID: u32 = 0x40;
pub(super) const GHWCFG1: u32 = 0x44;
pub(super) const GHWCFG2: u32 = 0x48;
pub(super) const GHWCFG3: u32 = 0x4C;
pub(super) const GHWCFG4: u32 = 0x50;
pub(super) const HCFG: u32 = 0x400;
pub(super) const HFIR: u32 = 0x404;
pub(super) const HFNUM: u32 = 0x408;
pub(super) const HPTXSTS: u32 = 0x410;
pub(super) const HAINT: u32 = 0x414;
pub(super) const HAINTMSK: u32 = 0x418;
pub(super) const HPRT: u32 = 0x440;
pub(super) const GCCFG: u32 = 0x38;
pub(super) const DCFG: u32 = 0x800;
pub(super) const DCTL: u32 = 0x804;
pub(super) const PCGCCTL: u32 = 0xE00;
pub(super) const CHANNEL0: u32 = 0x500;
pub(super) const CHANNEL_STRIDE: u32 = 0x20;
pub(super) const FIFO: u32 = 0x1000;
pub(super) const FIFO_STRIDE: u32 = 0x1000;

// Offsets within a channel.
pub(super) const CH_HCCHAR: u32 = 0x00;
pub(super) const CH_HCINT: u32 = 0x08;
pub(super) const CH_HCINTMSK: u32 = 0x0C;
pub(super) const CH_HCTSIZ: u32 = 0x10;

pub(super) const GAHBCFG_GINT: u32 = 1;

// GRSTCTL.
pub(super) const CSRST: u32 = 1 << 0;
pub(super) const RXFFLSH: u32 = 1 << 4;
pub(super) const TXFFLSH: u32 = 1 << 5;
pub(super) const TXFNUM_SHIFT: u32 = 6;
pub(super) const TXFNUM_MASK: u32 = 0x1F;
// TXFNUM (RM0433 57.14.5, p. 2628): in host mode 0 is the non-periodic FIFO, 1 the periodic one,
// 2 to 15 are device mode only, and 0x10 flushes all of them.
pub(super) const TXFNUM_ALL: u32 = 0x10;
// Bit 29 is reserved in RM0433 (57.14.5, p. 2628), which has GRSTCTL reset to 0x80000000. The
// firmware waits for it on one path (see GSNPSID_VALUE), so the model sets it.
pub(super) const CSRST_DONE: u32 = 1 << 29;
pub(super) const AHBIDL: u32 = 1 << 31;

// GINTSTS (and GINTMSK).
pub(super) const CMOD: u32 = 1 << 0;
pub(super) const SOF: u32 = 1 << 3;
pub(super) const RXFLVL: u32 = 1 << 4;
pub(super) const NPTXFE: u32 = 1 << 5;
pub(super) const HPRTINT: u32 = 1 << 24;
pub(super) const HCINT: u32 = 1 << 25;
pub(super) const PTXFE: u32 = 1 << 26;
pub(super) const DISCINT: u32 = 1 << 29;

// HPRT.
pub(super) const PCSTS: u32 = 1 << 0;
pub(super) const PCDET: u32 = 1 << 1;
pub(super) const PENA: u32 = 1 << 2;
pub(super) const PENCHNG: u32 = 1 << 3;
pub(super) const POCCHNG: u32 = 1 << 5;
pub(super) const PRST: u32 = 1 << 8;
pub(super) const PLSTS_LOW: u32 = 2 << 10;
pub(super) const PPWR: u32 = 1 << 12;
pub(super) const PSPD_LOW: u32 = 2 << 17;

// HCCHAR.
pub(super) const CHDIS: u32 = 1 << 30;
pub(super) const CHENA: u32 = 1 << 31;

// HCINT.
pub(super) const XFRC: u32 = 1 << 0;
pub(super) const CHH: u32 = 1 << 1;
pub(super) const STALL: u32 = 1 << 3;
pub(super) const NAK: u32 = 1 << 4;
pub(super) const ACK: u32 = 1 << 5;
pub(super) const TXERR: u32 = 1 << 7;

// GHWCFG1 to 4 and GSNPSID are not in RM0433: its register map (Table 523, pp. 2614 and 2615)
// goes from OTG_GCCFG (0x38) and OTG_CID (0x3C) to OTG_GLPMCFG (0x54) and has nothing at 0x40 to
// 0x50. They are the Synopsys core's own registers, which the firmware reads, and what they hold
// is a guess: I have not read them from the board, and the firmware reads only GHWCFG2. I
// picked them to describe the H743's OTG with the embedded full-speed PHY by the Synopsys
// core's usual layout, and they agree with how the firmware sets the core up.
//  - GHWCFG1: 0, every endpoint bidirectional.
//  - GHWCFG2: slave mode only (OTG_ARCH 0), dedicated FS PHY (bits 9:8 = 1), 6 device
//    endpoints (bits 13:10 = 5, the count after endpoint 0), 12 host channels (bits 17:14 = 11),
//    periodic support (bit 18), dynamic FIFO sizing (bit 19), and queues of 8 entries (2 in
//    bits 23:22 and 25:24); the rest, the OTG mode, the multiprocessor interrupt (bit 20) and
//    the token queue depth, 0.
//  - GHWCFG3: a data FIFO of 1024 words, the width of the transfer and packet counters
//    (HCTSIZ: 19 and 10 bits), and OTG enabled.
//  - GHWCFG4: 0.
pub(super) const GHWCFG2_VALUE: u32 = 0x028E_D500;
pub(super) const GHWCFG3_VALUE: u32 = 0x0400_00E8;
// GSNPSID is a guess too (see above). The firmware wants the top half to be one of three
// Synopsys ids, and for the low half not to be above 0x4209, or it waits for CSRST_DONE in
// GRSTCTL instead of AHBIDL (which I answer as well).
pub(super) const GSNPSID_VALUE: u32 = 0x4F54_310A;
// The room in the non-periodic and periodic transmit queues, in words, and how many requests
// they can hold. The model takes a packet as it is written, so they are never full.
pub(super) const GNPTXSTS_VALUE: u32 = 0x0008_0200;
pub(super) const HPTXSTS_VALUE: u32 = 0x0008_0100;

// What the silicon resets to, as far as I can tell from what the board reads after the firmware
// has edited them; the SVD's values would give a different result. RM0433 gives other values
// (GUSBCFG 0b0X00...0001 0100 0001 0000 in 57.14.4, p. 2625, that is 0x1410 with bit 30 undefined, with bit 4 where the board has PHYSEL, bit 6, and
// HCFG 0 in 57.14.22, p. 2651), which would not leave what the board reads once the firmware
// has edited them, so the board's wins.
pub(super) const GUSBCFG_RESET: u32 = 0x1440;
pub(super) const HCFG_RESET: u32 = 0x200;
// What a core soft reset does not clear (RM0433 57.14.5, p. 2629), as register and mask:
// HCFG.FSLSPCS, the clock for the speed of the PHY, DCFG.DSPD, DCTL.SDIS, PCGCCTL.GATEHCLK and
// STPPCLK, and all of GCCFG.
pub(super) const CSRST_KEEPS: [(u32, u32); 5] = [
    (HCFG, 3),
    (DCFG, 3),
    (DCTL, 1 << 1),
    (PCGCCTL, 3),
    (GCCFG, !0),
];

// HPRT: the bits the firmware can set and clear as it likes, and the bits that clear when a 1
// is written to them.
const PRES: u32 = 1 << 6;
pub(super) const PSUSP: u32 = 1 << 7;
const PTCTL: u32 = 0xF << 13;
pub(super) const HPRT_RW: u32 = PPWR | PRST | PRES | PSUSP | PTCTL;
pub(super) const HPRT_W1C: u32 = PCDET | PENCHNG | POCCHNG;

// HCTSIZ: the size in bytes, the packet count, and the data toggle (DPID).
pub(super) const XFRSIZ_MASK: u32 = 0x7_FFFF;
pub(super) const PKTCNT_SHIFT: u32 = 19;
pub(super) const PKTCNT_MASK: u32 = 0x3FF;
pub(super) const DPID_SHIFT: u32 = 29;
pub(super) const DATA0: u32 = 0;
pub(super) const DATA1: u32 = 2;
pub(super) const SETUP_DPID: u32 = 3;

// HCCHAR.
pub(super) const MPSIZ_MASK: u32 = 0x7FF;
pub(super) const EPNUM_SHIFT: u32 = 11;
pub(super) const EPTYP_SHIFT: u32 = 18;
pub(super) const EPDIR_IN: u32 = 1 << 15;
pub(super) const DAD_SHIFT: u32 = 22;

// The packet status of the words in the RX FIFO.
pub(super) const PKTSTS_DATA: u32 = 2 << 17;
pub(super) const PKTSTS_COMPLETE: u32 = 3 << 17;
