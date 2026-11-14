//! The USB OTG controller the firmware drives as its host, at 0x4008_0000: the second OTG block
//! of the STM32H743, with the embedded full-speed PHY. The firmware runs it in slave mode, with
//! no DMA: it sets up a channel, and moves the data through the FIFO windows itself.
//!
//! The model is a host controller in front of one port, with at most one device behind it
//! (`UsbPort`). It does what docs/usb-host.md found the firmware needs, and nothing more: a core
//! that identifies itself, a port that connects, resets and enables, a start of frame, and
//! host channels that run one transaction at a time against the device.
//!
//! Everything that happens later than a register access is scheduled with `Clock::wake_at`, and
//! the clock forgets a wake once it is due, so `tick` asks again for the next one. The
//! interrupt line is a level on the chip, but `Irqs` is edge like, so the line is raised after
//! every access that can leave it active.

use std::collections::VecDeque;

use crate::clock::{CORE_HZ, Clock};
use crate::irq::Irqs;
use crate::peripheral::{ResetCause, StubRegisters};
use crate::registers::{Registers, WordAccess};
use crate::svd::Svd;
use crate::usb::{In, Out, UsbPort};

pub const OTG_BASE: u32 = 0x4008_0000;
/// The registers reach 0x680 and the FIFO windows 0x1000 + 0x1000 per channel.
pub const OTG_SIZE: u32 = 0x2_0000;

/// `OTG_FS_IRQn`.
const OTG_IRQ: u16 = 101;
const CHANNELS: usize = 12;

/// The frame number counts to 0x3FFF and starts again at 0, as RM0433 gives HFNUM.FRNUM.
const FRNUM_MASK: u64 = 0x3FFF;

const CYCLES_PER_US: u64 = CORE_HZ / 1_000_000;
const CYCLES_PER_FRAME: u64 = CORE_HZ / 1_000;
// The delays are my choices, from the spike: the firmware does not look at any of them. A
// device connects 200 us after the port is powered, the port is enabled 2 us after the reset
// ends (the board does it in about that), and a transaction takes 100 us, about what eight
// bytes take at low speed.
const CONNECT_DELAY: u64 = 200 * CYCLES_PER_US;
const ENABLE_DELAY: u64 = 2 * CYCLES_PER_US;
const TRANSACTION_DELAY: u64 = 100 * CYCLES_PER_US;

// Register offsets from the start of the window, as the SVD gives them.
const GOTGINT: u32 = 0x04;
const GAHBCFG: u32 = 0x08;
const GUSBCFG: u32 = 0x0C;
const GRSTCTL: u32 = 0x10;
const GINTSTS: u32 = 0x14;
const GINTMSK: u32 = 0x18;
const GRXSTSR: u32 = 0x1C;
const GRXSTSP: u32 = 0x20;
const GNPTXSTS: u32 = 0x2C;
const GSNPSID: u32 = 0x40;
const GHWCFG1: u32 = 0x44;
const GHWCFG2: u32 = 0x48;
const GHWCFG3: u32 = 0x4C;
const GHWCFG4: u32 = 0x50;
const HCFG: u32 = 0x400;
const HFIR: u32 = 0x404;
const HFNUM: u32 = 0x408;
const HPTXSTS: u32 = 0x410;
const HAINT: u32 = 0x414;
const HAINTMSK: u32 = 0x418;
const HPRT: u32 = 0x440;
const CHANNEL0: u32 = 0x500;
const CHANNEL_STRIDE: u32 = 0x20;
const FIFO: u32 = 0x1000;
const FIFO_STRIDE: u32 = 0x1000;

// Offsets within a channel.
const CH_HCCHAR: u32 = 0x00;
const CH_HCINT: u32 = 0x08;
const CH_HCINTMSK: u32 = 0x0C;
const CH_HCTSIZ: u32 = 0x10;

const GAHBCFG_GINT: u32 = 1;

// GRSTCTL.
const CSRST: u32 = 1 << 0;
const RXFFLSH: u32 = 1 << 4;
const TXFFLSH: u32 = 1 << 5;
const TXFNUM_SHIFT: u32 = 6;
const TXFNUM_MASK: u32 = 0x1F;
const TXFNUM_ALL: u32 = 0x10;
const CSRST_DONE: u32 = 1 << 29;
const AHBIDL: u32 = 1 << 31;

// GINTSTS (and GINTMSK).
const CMOD: u32 = 1 << 0;
const SOF: u32 = 1 << 3;
const RXFLVL: u32 = 1 << 4;
const NPTXFE: u32 = 1 << 5;
const HPRTINT: u32 = 1 << 24;
const HCINT: u32 = 1 << 25;
const PTXFE: u32 = 1 << 26;
const DISCINT: u32 = 1 << 29;

// HPRT.
const PCSTS: u32 = 1 << 0;
const PCDET: u32 = 1 << 1;
const PENA: u32 = 1 << 2;
const PENCHNG: u32 = 1 << 3;
const POCCHNG: u32 = 1 << 5;
const PRST: u32 = 1 << 8;
const PLSTS_LOW: u32 = 2 << 10;
const PPWR: u32 = 1 << 12;
const PSPD_LOW: u32 = 2 << 17;

// HCCHAR.
const CHDIS: u32 = 1 << 30;
const CHENA: u32 = 1 << 31;

// HCINT.
const XFRC: u32 = 1 << 0;
const CHH: u32 = 1 << 1;
const STALL: u32 = 1 << 3;
const NAK: u32 = 1 << 4;
const ACK: u32 = 1 << 5;
const TXERR: u32 = 1 << 7;

// GHWCFG1 to 4 are guesses: I have not read them from the board, and I do not know that the
// reference manual describes them; the firmware reads only GHWCFG2. I picked them to describe
// the H743's OTG with the embedded full-speed PHY by the Synopsys core's usual layout, and they
// agree with how the firmware sets the core up.
//  - GHWCFG1: 0, every endpoint bidirectional.
//  - GHWCFG2: slave mode only (OTG_ARCH 0), dedicated FS PHY (bits 9:8 = 1), 6 device
//    endpoints (bits 13:10 = 5, the count after endpoint 0), 12 host channels (bits 17:14 = 11),
//    periodic support (bit 18), dynamic FIFO sizing (bit 19), and queues of 8 entries (2 in
//    bits 23:22 and 25:24); the rest, the OTG mode, the multiprocessor interrupt (bit 20) and
//    the token queue depth, 0.
//  - GHWCFG3: a data FIFO of 1024 words, the width of the transfer and packet counters
//    (HCTSIZ: 19 and 10 bits), and OTG enabled.
//  - GHWCFG4: 0.
const GHWCFG2_VALUE: u32 = 0x028E_D500;
const GHWCFG3_VALUE: u32 = 0x0400_00E8;
// GSNPSID is a guess too. The firmware wants the top half to be one of three Synopsys ids,
// and for the low half not to be above 0x4209, or it waits for CSRST_DONE in GRSTCTL instead
// of AHBIDL (which I answer as well).
const GSNPSID_VALUE: u32 = 0x4F54_310A;
// The room in the non-periodic and periodic transmit queues, in words, and how many requests
// they can hold. The model takes a packet as it is written, so they are never full.
const GNPTXSTS_VALUE: u32 = 0x0008_0200;
const HPTXSTS_VALUE: u32 = 0x0008_0100;

// What the silicon resets to, as far as I can tell from what the board reads after the firmware
// has edited them; the SVD's values would give a different result.
const GUSBCFG_RESET: u32 = 0x1440;
const HCFG_RESET: u32 = 0x200;
// HCFG.FSLSPCS, the clock for the speed of the PHY.
const HCFG_FSLSPCS: u32 = 3;

// HPRT: the bits the firmware can set and clear as it likes, and the bits that clear when a 1
// is written to them.
const PRES: u32 = 1 << 6;
const PSUSP: u32 = 1 << 7;
const PTCTL: u32 = 0xF << 13;
const HPRT_RW: u32 = PPWR | PRST | PRES | PSUSP | PTCTL;
const HPRT_W1C: u32 = PCDET | PENCHNG | POCCHNG;

// HCTSIZ: the size in bytes, the packet count, and the data toggle (DPID).
const XFRSIZ_MASK: u32 = 0x7_FFFF;
const PKTCNT_SHIFT: u32 = 19;
const PKTCNT_MASK: u32 = 0x3FF;
const DPID_SHIFT: u32 = 29;
const DATA0: u32 = 0;
const DATA1: u32 = 2;
const SETUP_DPID: u32 = 3;

// HCCHAR.
const MPSIZ_MASK: u32 = 0x7FF;
const EPNUM_SHIFT: u32 = 11;
const EPTYP_SHIFT: u32 = 18;
const EPDIR_IN: u32 = 1 << 15;
const DAD_SHIFT: u32 = 22;

// The packet status of the words in the RX FIFO.
const PKTSTS_DATA: u32 = 2 << 17;
const PKTSTS_COMPLETE: u32 = 3 << 17;

/// One host channel: a transfer, in whole, that the firmware sets up and the model carries out a
/// packet at a time.
#[derive(Default)]
struct Channel {
    hcchar: u32,
    hcint: u32,
    hcintmsk: u32,
    hctsiz: u32,
    /// The bytes the firmware has written to the FIFO window for an OUT or SETUP packet that
    /// has not been sent yet.
    staged: Vec<u8>,
    /// The cycle the next transaction happens at.
    due: Option<u64>,
}

impl Channel {
    fn input(&self) -> bool {
        self.hcchar & EPDIR_IN != 0
    }

    /// Isochronous and interrupt endpoints send through the periodic FIFO.
    fn periodic(&self) -> bool {
        self.hcchar >> EPTYP_SHIFT & 1 != 0
    }

    fn max_packet(&self) -> usize {
        (self.hcchar & MPSIZ_MASK) as usize
    }

    fn size(&self) -> usize {
        (self.hctsiz & XFRSIZ_MASK) as usize
    }

    fn packets(&self) -> u32 {
        self.hctsiz >> PKTCNT_SHIFT & PKTCNT_MASK
    }

    fn dpid(&self) -> u32 {
        self.hctsiz >> DPID_SHIFT
    }

    /// How long the next packet is, going out: what is left, or a whole packet.
    fn next_out_len(&self) -> usize {
        self.size().min(self.max_packet())
    }

    /// Takes `bytes` and one packet off the transfer, and sets the toggle for the next one:
    /// what a transfer leaves in HCTSIZ for the firmware to read back.
    fn account(&mut self, bytes: usize) {
        let size = self.size().saturating_sub(bytes) as u32;
        let packets = self.packets().saturating_sub(1);
        let next = if self.dpid() == DATA1 { DATA0 } else { DATA1 };
        self.hctsiz = next << DPID_SHIFT | packets << PKTCNT_SHIFT | size;
    }
}

/// What the device had to say to a transaction.
enum Heard {
    Nothing,
    Handshake(Out),
    Packet(In),
}

#[derive(Default)]
struct State {
    /// The flags the core latches: only DISCINT. The rest of GINTSTS is worked out.
    latched: u32,
    /// The frame that was current when the firmware last cleared SOF.
    sof_seen: u64,
    /// The RX FIFO: for each packet a status word, and for a data packet its words.
    rx: VecDeque<u32>,
    hprt: u32,
    channels: [Channel; CHANNELS],
    connect_at: Option<u64>,
    enable_at: Option<u64>,
}

/// The core is not in any state that is not in the registers it hands out, apart from the
/// device's own, in the port.
pub struct OtgRegisters {
    /// The registers that only remember what was written.
    stored: StubRegisters,
    clock: Clock,
    irqs: Irqs,
    port: UsbPort,
    /// `UsbPort::plugs` as the core last knew it.
    seen_plugs: u32,
    state: State,
}

impl OtgRegisters {
    fn frame(&self) -> u64 {
        self.clock.now() / CYCLES_PER_FRAME
    }

    /// The frame number, and the time left in the frame in units of the frame interval (HFIR).
    fn hfnum(&mut self) -> u32 {
        let now = self.clock.now();
        let left = CYCLES_PER_FRAME - now % CYCLES_PER_FRAME;
        let interval = u64::from(self.stored.read(HFIR) & 0xFFFF);
        let remaining = (interval * left / CYCLES_PER_FRAME) as u32;
        remaining << 16 | (self.frame() & FRNUM_MASK) as u32
    }

    // Channel n's interrupt flags that the firmware asked to hear of.
    fn haint(&self) -> u32 {
        let pending = |ch: &Channel| ch.hcint & ch.hcintmsk != 0;
        self.state
            .channels
            .iter()
            .enumerate()
            .filter(|(_, ch)| pending(ch))
            .fold(0, |bits, (n, _)| bits | 1 << n)
    }

    fn gintsts(&mut self) -> u32 {
        let mut status = CMOD | NPTXFE | PTXFE | self.state.latched;
        if self.frame() != self.state.sof_seen {
            status |= SOF;
        }
        if !self.state.rx.is_empty() {
            status |= RXFLVL;
        }
        if self.state.hprt & HPRT_W1C != 0 {
            status |= HPRTINT;
        }
        if self.haint() & self.stored.read(HAINTMSK) != 0 {
            status |= HCINT;
        }
        status
    }

    /// Whether the interrupt line would be active.
    fn interrupt_active(&mut self) -> bool {
        self.stored.read(GAHBCFG) & GAHBCFG_GINT != 0
            && self.gintsts() & self.stored.read(GINTMSK) & !CMOD != 0
    }

    // Raises the line if it is active: after anything that can have made it so, or kept it so.
    fn interrupt(&mut self) {
        if self.interrupt_active() {
            self.irqs.raise(OTG_IRQ);
        }
    }

    /// Asks the clock for a tick at the earliest thing that has to happen: a connect, the port
    /// being enabled, a transaction, or, only while the firmware wants SOF interrupts, the next
    /// frame.
    fn schedule(&mut self) {
        let state = &self.state;
        let mut next = [state.connect_at, state.enable_at]
            .into_iter()
            .chain(state.channels.iter().map(|ch| ch.due))
            .flatten()
            .min();
        if self.stored.read(GINTMSK) & SOF != 0 {
            let frame_start = (self.frame() + 1) * CYCLES_PER_FRAME;
            next = Some(next.map_or(frame_start, |at| at.min(frame_start)));
        }
        if let Some(cycle) = next {
            self.clock.wake_at(cycle);
        }
    }

    // The registers of the core, put back as a reset leaves them.
    fn reset_values(&mut self, cause: ResetCause) {
        self.stored.reset(cause);
        self.stored.write(GUSBCFG, GUSBCFG_RESET, !0);
        self.stored.write(HCFG, HCFG_RESET, !0);
        self.state = State {
            sof_seen: self.frame(),
            ..State::default()
        };
        // The device is reset with the chip: whatever the firmware had set up with it is gone.
        self.port.with_device(|device| device.reset());
    }

    // A core soft reset: the state machines go idle and the FIFOs empty, so the channels, the
    // RX queue and the flags the core latched are cleared, and so are the registers, which go
    // back to what a reset leaves except for the speed bits of HCFG. The port keeps its state,
    // as the device stays connected. This is from my memory of RM0433, which I have not checked:
    // I remember it keeping a few more bits (the power and clock gating of PCGCCTL, and
    // DCFG.DSPD, which this core does not have). The firmware does it before it sets anything
    // up, so it cannot tell.
    fn soft_reset(&mut self) {
        let speed = self.stored.read(HCFG) & HCFG_FSLSPCS;
        self.stored.reset(ResetCause::Software);
        self.stored.write(GUSBCFG, GUSBCFG_RESET, !0);
        self.stored.write(HCFG, HCFG_RESET | speed, !0);
        let state = &mut self.state;
        state.rx.clear();
        state.latched = 0;
        state.channels = Default::default();
    }

    // The transmit FIFOs `fifo` names are emptied: 0 is the non-periodic one, which the control
    // and bulk channels send through, 1 the periodic one, for the interrupt and isochronous
    // ones, and 0x10 both. What the firmware wrote for a packet that has not gone is dropped, and
    // a packet that was ready to go has nothing left to send. An OUT transfer that sends no data
    // has nothing in the FIFO to lose, and goes on.
    fn flush_tx(&mut self, fifo: u32) {
        for channel in &mut self.state.channels {
            let named = fifo == TXFNUM_ALL || fifo == u32::from(channel.periodic());
            // Only a channel that sends has anything staged.
            if named && !channel.staged.is_empty() {
                channel.staged.clear();
                channel.due = None;
            }
        }
    }

    // The port follows the device in the socket and the power: a device that is plugged in
    // to a port with power connects, one that is pulled out or has lost its power disconnects.
    fn sync_port(&mut self) {
        let plugs = self.port.plugs();
        let replaced = plugs != self.seen_plugs;
        self.seen_plugs = plugs;
        let wanted = self.state.hprt & PPWR != 0 && self.port.attached();
        if self.state.hprt & PCSTS != 0 && (replaced || !wanted) {
            self.disconnect();
            // A device that has lost its power starts again when it gets it back.
            if !wanted {
                self.port.with_device(|device| device.reset());
            }
        }
        if !wanted {
            self.state.connect_at = None;
        } else if self.state.hprt & PCSTS == 0 && self.state.connect_at.is_none() {
            self.state.connect_at = Some(self.clock.now() + CONNECT_DELAY);
        }
    }

    fn disconnect(&mut self) {
        let state = &mut self.state;
        if state.hprt & PENA != 0 {
            state.hprt |= PENCHNG;
        }
        state.hprt &= !(PCSTS | PENA);
        state.latched |= DISCINT;
        state.enable_at = None;
        state.rx.clear();
        for channel in &mut state.channels {
            channel.due = None;
            channel.staged.clear();
        }
    }

    fn hprt_read(&self) -> u32 {
        let hprt = self.state.hprt;
        if hprt & PCSTS == 0 {
            hprt
        } else {
            hprt | PLSTS_LOW | PSPD_LOW
        }
    }

    // `value` is what the firmware wrote, with the bytes it did not write cleared.
    fn hprt_write(&mut self, value: u32, mask: u32) {
        let was = self.state.hprt;
        let mut hprt = was & !(value & HPRT_W1C);
        // Writing a 1 to PENA disables the port.
        hprt &= !(value & PENA);
        hprt = hprt & !(HPRT_RW & mask) | value & HPRT_RW;
        self.state.hprt = hprt;
        if was & PRST == 0 && hprt & PRST != 0 {
            // A reset disables the port, and resets the device.
            self.state.hprt &= !PENA;
            self.state.enable_at = None;
            self.port.with_device(|device| device.reset());
        } else if was & PRST != 0 && hprt & PRST == 0 && hprt & PCSTS != 0 {
            self.state.enable_at = Some(self.clock.now() + ENABLE_DELAY);
        }
    }

    // The channel and register of a channel that `offset` is.
    fn channel_register(offset: u32) -> Option<(usize, u32)> {
        let rel = offset.checked_sub(CHANNEL0)?;
        let n = (rel / CHANNEL_STRIDE) as usize;
        (n < CHANNELS).then_some((n, rel % CHANNEL_STRIDE))
    }

    fn channel_read(&mut self, n: usize, register: u32, offset: u32) -> u32 {
        let channel = &self.state.channels[n];
        match register {
            CH_HCCHAR => channel.hcchar,
            CH_HCINT => channel.hcint,
            CH_HCINTMSK => channel.hcintmsk,
            CH_HCTSIZ => channel.hctsiz,
            _ => self.stored.read(offset),
        }
    }

    fn channel_write(&mut self, n: usize, register: u32, offset: u32, value: u32, mask: u32) {
        let merge = |old: u32| old & !mask | value & mask;
        let channel = &mut self.state.channels[n];
        match register {
            CH_HCCHAR => {
                let hcchar = merge(channel.hcchar);
                if hcchar & CHDIS != 0 {
                    // A halt request: the transfer ends and the channel reports it.
                    channel.hcchar = hcchar & !(CHENA | CHDIS);
                    channel.hcint |= CHH;
                    channel.due = None;
                } else {
                    channel.hcchar = hcchar;
                    if hcchar & CHENA != 0 {
                        channel.staged.clear();
                        self.arm(n);
                    }
                }
            }
            CH_HCINT => channel.hcint &= !(value & mask),
            CH_HCINTMSK => channel.hcintmsk = merge(channel.hcintmsk),
            CH_HCTSIZ => channel.hctsiz = merge(channel.hctsiz),
            _ => self.stored.write(offset, value, mask),
        }
    }

    // Schedules channel n's transaction if it has what it needs: an IN transfer always does, an
    // OUT or SETUP one once the firmware has written the whole packet to the FIFO.
    fn arm(&mut self, n: usize) {
        let now = self.clock.now();
        let channel = &mut self.state.channels[n];
        let ready = channel.input() || channel.staged.len() >= channel.next_out_len();
        if channel.hcchar & CHENA != 0 && channel.packets() > 0 && ready {
            channel.due = Some(now + TRANSACTION_DELAY);
        }
    }

    // What the firmware wrote to the FIFO window of channel n: the bytes of `word` that `mask`
    // selects, as a byte or halfword write has them. The FIFO takes what it is given: a narrow
    // write is not widened to a word.
    fn stage(&mut self, n: usize, word: u32, mask: u32) {
        let Some(channel) = self.state.channels.get_mut(n) else {
            return;
        };
        if !channel.input() {
            channel.staged.extend(
                word.to_le_bytes()
                    .into_iter()
                    .zip(mask.to_le_bytes())
                    .filter(|&(_, lane)| lane == 0xFF)
                    .map(|(byte, _)| byte),
            );
            self.arm(n);
        }
    }

    // One transaction of channel n's transfer, against the device.
    fn transact(&mut self, n: usize) {
        let channel = &mut self.state.channels[n];
        channel.due = None;
        let device = (channel.hcchar >> DAD_SHIFT & 0x7F) as u8;
        let endpoint = (channel.hcchar >> EPNUM_SHIFT & 0xF) as u8;
        let max = channel.max_packet();
        let setup = channel.dpid() == SETUP_DPID;
        let out_len = channel.next_out_len().min(channel.staged.len());
        let packet: Vec<u8> = if channel.input() {
            Vec::new()
        } else {
            channel.staged.drain(..out_len).collect()
        };
        let input = channel.input();

        let heard = if self.state.hprt & PENA == 0 {
            Heard::Nothing
        } else {
            self.port
                .with_device(|d| {
                    if d.address() != device {
                        Heard::Nothing
                    } else if input {
                        Heard::Packet(d.in_token(endpoint, max))
                    } else if setup {
                        let mut bytes = [0; 8];
                        let len = packet.len().min(8);
                        bytes[..len].copy_from_slice(&packet[..len]);
                        d.setup(bytes);
                        Heard::Handshake(Out::Ack)
                    } else {
                        Heard::Handshake(d.out(endpoint, &packet))
                    }
                })
                .unwrap_or(Heard::Nothing)
        };
        self.finish(n, heard, packet.len());
    }

    // What a transaction came to, as the channel's flags and the RX FIFO.
    fn finish(&mut self, n: usize, heard: Heard, sent: usize) {
        let now = self.clock.now();
        let state = &mut self.state;
        let channel = &mut state.channels[n];
        match heard {
            Heard::Nothing => channel.hcint |= TXERR,
            Heard::Handshake(Out::Nak) | Heard::Packet(In::Nak) => channel.hcint |= NAK,
            Heard::Handshake(Out::Stall) | Heard::Packet(In::Stall) => channel.hcint |= STALL,
            Heard::Handshake(Out::Ack) => {
                channel.hcint |= ACK;
                channel.account(sent);
                if channel.packets() == 0 {
                    channel.hcint |= XFRC;
                } else {
                    self.arm(n);
                }
            }
            Heard::Packet(In::Data(data)) => {
                let status =
                    PKTSTS_DATA | channel.dpid() << 15 | (data.len() as u32) << 4 | n as u32;
                state.rx.push_back(status);
                state.rx.extend(
                    data.chunks(4)
                        .map(|word| word.iter().rev().fold(0, |w, &b| w << 8 | u32::from(b))),
                );
                channel.hcint |= ACK;
                channel.account(data.len());
                // A full packet with more wanted asks for the next; anything else ends the
                // transfer, short packets early.
                if data.len() == channel.max_packet() && channel.size() > 0 && channel.packets() > 0
                {
                    channel.due = Some(now + TRANSACTION_DELAY);
                } else {
                    state.rx.push_back(PKTSTS_COMPLETE | n as u32);
                    channel.hcint |= XFRC;
                }
            }
        }
    }
}

impl Registers for OtgRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        if offset >= FIFO {
            // Whichever window it is, it is the one RX FIFO.
            return self.state.rx.pop_front().unwrap_or(0);
        }
        match offset {
            GINTSTS => self.gintsts(),
            GRSTCTL => AHBIDL | CSRST_DONE,
            GRXSTSR => self.state.rx.front().copied().unwrap_or(0),
            GRXSTSP => {
                let status = self.state.rx.pop_front().unwrap_or(0);
                self.interrupt();
                status
            }
            GNPTXSTS => GNPTXSTS_VALUE,
            GSNPSID => GSNPSID_VALUE,
            GHWCFG1 | GHWCFG4 => 0,
            GHWCFG2 => GHWCFG2_VALUE,
            GHWCFG3 => GHWCFG3_VALUE,
            HFNUM => self.hfnum(),
            HPTXSTS => HPTXSTS_VALUE,
            HAINT => self.haint(),
            HPRT => self.hprt_read(),
            _ => match Self::channel_register(offset) {
                Some((n, register)) => self.channel_read(n, register, offset),
                None => self.stored.read(offset),
            },
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        if offset >= FIFO {
            let window = ((offset - FIFO) / FIFO_STRIDE) as usize;
            self.stage(window, value, mask);
        } else {
            match offset {
                // Read only, or flags nothing here ever sets.
                GOTGINT | GNPTXSTS | GSNPSID | GHWCFG1 | GHWCFG2 | GHWCFG3 | GHWCFG4 | HFNUM
                | HPTXSTS | HAINT | GRXSTSR | GRXSTSP => {}
                GRSTCTL => {
                    // The reset and the flushes are over as soon as they are asked for.
                    let bits = value & mask;
                    if bits & CSRST != 0 {
                        self.soft_reset();
                    }
                    if bits & TXFFLSH != 0 {
                        self.flush_tx(bits >> TXFNUM_SHIFT & TXFNUM_MASK);
                    }
                    if bits & RXFFLSH != 0 {
                        self.state.rx.clear();
                    }
                }
                GINTSTS => {
                    let clear = value & mask;
                    self.state.latched &= !clear;
                    if clear & SOF != 0 {
                        self.state.sof_seen = self.frame();
                    }
                }
                HPRT => self.hprt_write(value & mask, mask),
                _ => match Self::channel_register(offset) {
                    Some((n, register)) => self.channel_write(n, register, offset, value, mask),
                    None => self.stored.write(offset, value, mask),
                },
            }
        }
        self.sync_port();
        self.schedule();
        self.interrupt();
    }

    fn reset(&mut self, cause: ResetCause) {
        self.reset_values(cause);
    }

    fn tick(&mut self) {
        self.sync_port();
        let now = self.clock.now();
        let state = &mut self.state;
        if state.connect_at.is_some_and(|at| at <= now) {
            state.connect_at = None;
            state.hprt |= PCSTS | PCDET;
        }
        if state.enable_at.is_some_and(|at| at <= now) {
            state.enable_at = None;
            state.hprt |= PENA | PENCHNG;
        }
        for n in 0..CHANNELS {
            if self.state.channels[n].due.is_some_and(|at| at <= now) {
                self.transact(n);
            }
        }
        self.schedule();
        self.interrupt();
    }
}

pub type Otg = WordAccess<OtgRegisters>;

impl Otg {
    pub fn new(base: u32, clock: Clock, irqs: Irqs, port: UsbPort) -> Otg {
        let seen_plugs = port.plugs();
        let mut registers = OtgRegisters {
            stored: StubRegisters::with_reset_values(base, Svd::stm32h743()),
            clock,
            irqs,
            port,
            seen_plugs,
            state: State::default(),
        };
        registers.reset_values(ResetCause::PowerOn);
        WordAccess::wrap(registers)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use super::*;
    use crate::memory::MemoryMap;
    use crate::usb::UsbDevice;
    use crate::usb_keyboard::{HidKeyboard, KeyReport};

    const GINTSTS_ADDR: u32 = OTG_BASE + GINTSTS;
    const GINTMSK_ADDR: u32 = OTG_BASE + GINTMSK;
    const GRSTCTL_ADDR: u32 = OTG_BASE + GRSTCTL;
    const GAHBCFG_ADDR: u32 = OTG_BASE + GAHBCFG;
    const HPRT_ADDR: u32 = OTG_BASE + HPRT;
    const HAINT_ADDR: u32 = OTG_BASE + HAINT;
    const HAINTMSK_ADDR: u32 = OTG_BASE + HAINTMSK;
    const FIFO0: u32 = OTG_BASE + FIFO;

    fn hcchar(n: u32) -> u32 {
        OTG_BASE + CHANNEL0 + CHANNEL_STRIDE * n + CH_HCCHAR
    }
    fn hcint(n: u32) -> u32 {
        OTG_BASE + CHANNEL0 + CHANNEL_STRIDE * n + CH_HCINT
    }
    fn hcintmsk(n: u32) -> u32 {
        OTG_BASE + CHANNEL0 + CHANNEL_STRIDE * n + CH_HCINTMSK
    }
    fn hctsiz(n: u32) -> u32 {
        OTG_BASE + CHANNEL0 + CHANNEL_STRIDE * n + CH_HCTSIZ
    }

    /// `hctsiz` for a transfer of `size` bytes in `packets` packets.
    fn tsiz(dpid: u32, packets: u32, size: u32) -> u32 {
        dpid << 29 | packets << 19 | size
    }

    /// `hcchar` for a channel that talks to `device`'s endpoint `endpoint`, with the packet size
    /// 8 that the keyboard has, and the channel enabled.
    fn chan(device: u32, endpoint: u32, input: bool) -> u32 {
        CHENA | device << 22 | u32::from(input) << 15 | endpoint << 11 | 8
    }

    const DATA0: u32 = 0;
    const DATA1: u32 = 2;
    const SETUP: u32 = 3;
    const POWERED_LOW_SPEED: u32 = PPWR | PLSTS_LOW | PSPD_LOW;

    #[derive(Default)]
    struct Log {
        setups: Vec<[u8; 8]>,
        outs: Vec<(u8, Vec<u8>)>,
        ins: Vec<(u8, usize)>,
        resets: u32,
    }

    /// A device that answers what it is told to, and writes down what it was asked.
    struct Script {
        log: Rc<RefCell<Log>>,
        address: u8,
        replies: VecDeque<In>,
        out_reply: Out,
    }

    fn script(address: u8, replies: Vec<In>) -> (Script, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let device = Script {
            log: log.clone(),
            address,
            replies: replies.into(),
            out_reply: Out::Ack,
        };
        (device, log)
    }

    impl UsbDevice for Script {
        fn address(&self) -> u8 {
            self.address
        }
        fn reset(&mut self) {
            self.log.borrow_mut().resets += 1;
        }
        fn setup(&mut self, packet: [u8; 8]) {
            self.log.borrow_mut().setups.push(packet);
        }
        fn out(&mut self, endpoint: u8, data: &[u8]) -> Out {
            self.log.borrow_mut().outs.push((endpoint, data.to_vec()));
            self.out_reply
        }
        fn in_token(&mut self, endpoint: u8, max: usize) -> In {
            self.log.borrow_mut().ins.push((endpoint, max));
            self.replies.pop_front().unwrap_or(In::Nak)
        }
    }

    struct Rig {
        m: MemoryMap,
        irqs: Irqs,
        clock: Clock,
        port: UsbPort,
    }

    fn rig() -> Rig {
        let irqs = Irqs::new();
        let mut m = MemoryMap::new();
        let clock = m.clock();
        let port = m.usb_port();
        m.map_peripheral(
            OTG_BASE,
            OTG_SIZE,
            Box::new(Otg::new(
                OTG_BASE,
                clock.clone(),
                irqs.clone(),
                port.clone(),
            )),
        )
        .unwrap();
        Rig {
            m,
            irqs,
            clock,
            port,
        }
    }

    impl Rig {
        fn read(&mut self, addr: u32) -> u32 {
            self.m.read32(addr)
        }

        fn write(&mut self, addr: u32, value: u32) {
            self.m.write32(addr, value);
        }

        // The machine runs on in steps of a microsecond, as the CPU does, so that what a tick
        // asks for later happens later.
        fn advance_cycles(&mut self, cycles: u64) {
            let mut left = cycles;
            while left > 0 {
                let step = left.min(CYCLES_PER_US);
                self.clock.set(self.clock.now() + step);
                left -= step;
                while self.clock.take_due() {
                    self.m.tick();
                }
            }
        }

        fn advance_us(&mut self, us: u64) {
            self.advance_cycles(us * CYCLES_PER_US);
        }

        // Whether the line was raised since the last call.
        fn raised(&self) -> bool {
            self.irqs.take() == Some([0, 0, 0, 1 << (OTG_IRQ - 96), 0, 0, 0, 0])
        }

        fn interrupts_on(&mut self, mask: u32) {
            self.write(GINTMSK_ADDR, mask);
            self.write(GAHBCFG_ADDR, 1);
        }

        /// Powers the port, the way the firmware does, and lets the device connect.
        fn power(&mut self) {
            self.write(HPRT_ADDR, PPWR);
            self.advance_us(200);
        }

        /// A port that has been powered, reset, and left enabled with its change flags clear.
        fn enabled_port(&mut self) {
            self.power();
            self.write(HPRT_ADDR, PPWR | PRST);
            self.write(HPRT_ADDR, PPWR);
            self.advance_us(10);
            self.write(HPRT_ADDR, PPWR | PCDET | PENCHNG);
            assert_eq!(self.read(HPRT_ADDR), PCSTS | PENA | POWERED_LOW_SPEED);
        }

        /// An enabled port with `device` on it, and the channel interrupts the firmware turns on.
        fn with(device: impl UsbDevice + 'static) -> Rig {
            let mut rig = rig();
            rig.port.attach(device);
            rig.enabled_port();
            rig.write(HAINTMSK_ADDR, 0xFFF);
            for n in 0..2 {
                rig.write(hcintmsk(n), 0x7B9);
            }
            rig
        }

        /// A SETUP stage on channel `n`: the transfer is set up and the two words written.
        fn setup(&mut self, n: u32, device: u32, packet: [u8; 8]) {
            self.write(hcint(n), 0xFFFF_FFFF);
            self.write(hctsiz(n), tsiz(SETUP, 1, 8));
            self.write(hcchar(n), chan(device, 0, false));
            for word in packet.chunks(4) {
                self.write(
                    FIFO0 + 0x1000 * n,
                    u32::from_le_bytes(word.try_into().unwrap()),
                );
            }
        }

        /// An IN transfer of `size` bytes in `packets` packets, to `endpoint`.
        fn start_in(
            &mut self,
            n: u32,
            device: u32,
            endpoint: u32,
            dpid: u32,
            packets: u32,
            size: u32,
        ) {
            self.write(hcint(n), 0xFFFF_FFFF);
            self.write(hctsiz(n), tsiz(dpid, packets, size));
            self.write(hcchar(n), chan(device, endpoint, true));
        }

        /// Pops everything in the RX FIFO as the firmware does: a status word, and for a data
        /// packet its words. Returns (status word, data bytes) for each.
        fn drain_rx(&mut self) -> Vec<(u32, Vec<u8>)> {
            let mut packets = Vec::new();
            while self.read(GINTSTS_ADDR) & RXFLVL != 0 {
                let status = self.read(OTG_BASE + GRXSTSP);
                let bytes = (status >> 4 & 0x7FF) as usize;
                let mut data = Vec::new();
                for _ in 0..bytes.div_ceil(4) {
                    data.extend(self.read(FIFO0).to_le_bytes());
                }
                data.truncate(bytes);
                packets.push((status, data));
            }
            packets
        }
    }

    // Identity and reset.

    #[test]
    fn the_core_id_is_a_synopsys_one() {
        let mut r = rig();
        let id = r.read(OTG_BASE + GSNPSID);
        assert!([0x4F54, 0x5531, 0x5532].contains(&(id >> 16)), "{id:#x}");
        // The firmware takes the AHBIDL path only for ids up to 0x4209.
        assert!(id & 0xFFFF <= 0x4209, "{id:#x}");
    }

    #[test]
    fn the_hardware_configuration_says_slave_mode_fs_phy_and_twelve_channels() {
        let mut r = rig();
        let cfg2 = r.read(OTG_BASE + GHWCFG2);
        assert_eq!(cfg2 >> 3 & 3, 0, "OTG_ARCH: slave only");
        assert_eq!(cfg2 >> 8 & 3, 1, "FSPHYTYPE: dedicated");
        assert_eq!(cfg2 >> 14 & 15, 11, "NUMHSTCHNL: 12 channels");
        assert_eq!(cfg2 >> 10 & 15, 5, "NUMDEVEPS: 6 endpoints");
        assert_eq!(cfg2 >> 18 & 1, 1, "PERIOSUPPORT");
        assert_eq!(cfg2 >> 19 & 1, 1, "DYNFIFOSIZING");
        assert_eq!((cfg2 >> 22 & 3, cfg2 >> 24 & 3), (2, 2), "queues of 8");
        assert_eq!(
            r.read(OTG_BASE + GHWCFG3) >> 16,
            0x400,
            "FIFO of 1024 words"
        );
    }

    #[test]
    fn the_identity_registers_are_read_only() {
        let mut r = rig();
        for offset in [GSNPSID, GHWCFG1, GHWCFG2, GHWCFG3, GHWCFG4] {
            let before = r.read(OTG_BASE + offset);
            r.write(OTG_BASE + offset, !before);
            assert_eq!(r.read(OTG_BASE + offset), before, "{offset:#x}");
        }
    }

    #[test]
    fn the_registers_reset_to_the_values_the_board_boots_from() {
        let mut r = rig();
        assert_eq!(r.read(OTG_BASE + GUSBCFG), 0x1440);
        assert_eq!(r.read(OTG_BASE + HCFG), 0x200);
        assert_eq!(r.read(OTG_BASE + HFIR), 0xEA60);
    }

    #[test]
    fn a_core_reset_and_the_flushes_complete_at_once() {
        let mut r = rig();
        let idle = r.read(OTG_BASE + GRSTCTL);
        assert_eq!(idle & AHBIDL, AHBIDL);
        for bits in [CSRST, TXFFLSH | 0x400, RXFFLSH] {
            r.write(OTG_BASE + GRSTCTL, bits);
            assert_eq!(r.read(OTG_BASE + GRSTCTL), idle, "{bits:#x}");
        }
    }

    #[test]
    fn the_transmit_queue_has_room() {
        let mut r = rig();
        assert_eq!(r.read(OTG_BASE + GNPTXSTS), 0x0008_0200);
        assert_eq!(r.read(OTG_BASE + HPTXSTS), 0x0008_0100);
    }

    #[test]
    fn the_core_is_in_host_mode_with_its_transmit_fifos_empty() {
        let mut r = rig();
        assert_eq!(r.read(GINTSTS_ADDR), CMOD | NPTXFE | PTXFE);
    }

    #[test]
    fn the_plain_registers_remember_what_was_written() {
        let mut r = rig();
        r.write(OTG_BASE + HCFG, 6);
        assert_eq!(r.read(OTG_BASE + HCFG), 6);
        r.write(OTG_BASE + 0x5C, 0x0400_0400); // GDFIFOCFG
        assert_eq!(r.read(OTG_BASE + 0x5C), 0x0400_0400);
    }

    #[test]
    fn a_system_reset_puts_the_registers_back() {
        let mut r = rig();
        r.write(OTG_BASE + GUSBCFG, 0xFFFF);
        r.write(GINTMSK_ADDR, 0x23000014);
        r.write(HPRT_ADDR, PPWR);
        r.m.reset_peripherals(ResetCause::Software);
        assert_eq!(r.read(OTG_BASE + GUSBCFG), 0x1440);
        assert_eq!(r.read(GINTMSK_ADDR), 0);
        assert_eq!(r.read(HPRT_ADDR), 0);
    }

    // Interrupts.

    #[test]
    fn the_current_mode_bit_is_not_an_interrupt() {
        let mut r = rig();
        r.interrupts_on(CMOD);
        assert!(r.irqs.take().is_none());
    }

    #[test]
    fn a_connect_raises_the_line_only_when_enabled_and_unmasked() {
        // Global interrupt off.
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.write(GINTMSK_ADDR, HPRTINT);
        r.power();
        assert_eq!(r.read(GINTSTS_ADDR) & HPRTINT, HPRTINT);
        assert!(r.irqs.take().is_none());
        // Masked.
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.write(GAHBCFG_ADDR, 1);
        r.power();
        assert!(r.irqs.take().is_none());
        // Both on.
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.interrupts_on(HPRTINT);
        r.power();
        assert!(r.raised());
    }

    #[test]
    fn unmasking_or_enabling_a_pending_condition_raises_the_line() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        assert!(r.irqs.take().is_none());
        r.write(GINTMSK_ADDR, HPRTINT);
        assert!(r.irqs.take().is_none());
        r.write(GAHBCFG_ADDR, 1);
        assert!(r.raised());
        // And the other way round.
        r.write(GAHBCFG_ADDR, 0);
        r.write(GINTMSK_ADDR, 0);
        r.irqs.take();
        r.write(GAHBCFG_ADDR, 1);
        assert!(r.irqs.take().is_none());
        r.write(GINTMSK_ADDR, HPRTINT);
        assert!(r.raised());
    }

    #[test]
    fn clearing_the_condition_silences_the_line() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.interrupts_on(HPRTINT);
        r.power();
        assert!(r.raised());
        r.write(HPRT_ADDR, PPWR | PCDET);
        assert_eq!(r.read(GINTSTS_ADDR) & HPRTINT, 0);
        r.write(GINTMSK_ADDR, HPRTINT | RXFLVL);
        assert!(r.irqs.take().is_none());
    }

    #[test]
    fn the_status_bits_the_core_computes_do_not_clear_by_writing() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.write(GINTSTS_ADDR, 0xFFFF_FFFF);
        assert_eq!(r.read(GINTSTS_ADDR), CMOD | NPTXFE | PTXFE | HPRTINT);
    }

    #[test]
    fn clearing_one_latched_flag_leaves_the_other() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.port.detach();
        r.advance_cycles(CYCLES_PER_FRAME);
        assert_eq!(r.read(GINTSTS_ADDR) & (SOF | DISCINT), SOF | DISCINT);
        r.write(GINTSTS_ADDR, SOF);
        assert_eq!(r.read(GINTSTS_ADDR) & (SOF | DISCINT), DISCINT);
    }

    #[test]
    fn the_otg_interrupt_flags_are_never_set() {
        let mut r = rig();
        r.write(OTG_BASE + GOTGINT, 0xFFFF_FFFF);
        assert_eq!(r.read(OTG_BASE + GOTGINT), 0);
    }

    #[test]
    fn a_system_reset_clears_the_start_of_frame_flag() {
        let mut r = rig();
        r.clock.set(5 * CYCLES_PER_FRAME);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, SOF);
        r.m.reset_peripherals(ResetCause::Software);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, 0);
    }

    // The port.

    #[test]
    fn a_device_connects_200_microseconds_after_the_port_is_powered() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.advance_us(1_000);
        assert_eq!(r.read(HPRT_ADDR), 0, "no power, no connect");
        r.write(HPRT_ADDR, PPWR);
        r.advance_cycles(200 * CYCLES_PER_US - 1);
        assert_eq!(r.read(HPRT_ADDR), PPWR);
        r.advance_cycles(1);
        assert_eq!(r.read(HPRT_ADDR), PCSTS | PCDET | POWERED_LOW_SPEED);
    }

    #[test]
    fn a_device_plugged_into_a_powered_port_connects_after_the_same_delay() {
        let mut r = rig();
        r.write(HPRT_ADDR, PPWR);
        r.advance_us(1_000);
        r.port.attach(script(1, vec![]).0);
        r.advance_cycles(1);
        assert_eq!(r.read(HPRT_ADDR), PPWR);
        r.advance_cycles(200 * CYCLES_PER_US);
        assert_eq!(r.read(HPRT_ADDR), PCSTS | PCDET | POWERED_LOW_SPEED);
    }

    #[test]
    fn with_no_device_the_port_stays_quiet() {
        let mut r = rig();
        r.interrupts_on(HPRTINT | RXFLVL | HCINT | DISCINT);
        r.irqs.take();
        r.power();
        r.write(HPRT_ADDR, PPWR | PRST);
        r.write(HPRT_ADDR, PPWR);
        r.advance_us(10_000);
        assert_eq!(r.read(HPRT_ADDR), PPWR);
        assert!(r.irqs.take().is_none());
    }

    #[test]
    fn the_change_flags_clear_when_one_is_written_and_the_other_bits_stay() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.write(HPRT_ADDR, PPWR | PCDET);
        assert_eq!(r.read(HPRT_ADDR), PCSTS | POWERED_LOW_SPEED);
    }

    #[test]
    fn the_port_is_enabled_two_microseconds_after_the_reset_ends() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.write(HPRT_ADDR, PPWR | PRST);
        r.advance_us(52_000);
        assert_eq!(r.read(HPRT_ADDR) & PENA, 0, "not while the reset is held");
        r.write(HPRT_ADDR, PPWR);
        r.advance_cycles(2 * CYCLES_PER_US - 1);
        assert_eq!(r.read(HPRT_ADDR) & (PENA | PENCHNG), 0);
        r.advance_cycles(1);
        assert_eq!(
            r.read(HPRT_ADDR),
            PCSTS | PCDET | PENA | PENCHNG | POWERED_LOW_SPEED
        );
        // The firmware clears PENCHNG, leaving PENA and PPWR alone, and ends up with what the
        // board reads.
        r.write(HPRT_ADDR, PPWR | PENCHNG | PCDET);
        assert_eq!(r.read(HPRT_ADDR), 0x41805);
    }

    #[test]
    fn a_device_pulled_out_as_the_reset_ends_is_not_enabled() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.write(HPRT_ADDR, PPWR | PRST);
        r.write(HPRT_ADDR, PPWR);
        r.port.detach();
        r.advance_us(10);
        assert_eq!(r.read(HPRT_ADDR) & PENA, 0);
    }

    #[test]
    fn a_reset_that_starts_again_before_the_port_is_enabled_holds_it_off() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.write(HPRT_ADDR, PPWR | PRST);
        r.write(HPRT_ADDR, PPWR);
        r.write(HPRT_ADDR, PPWR | PRST);
        r.advance_us(10);
        assert_eq!(r.read(HPRT_ADDR) & PENA, 0);
    }

    #[test]
    fn writing_pena_disables_the_port() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.write(HPRT_ADDR, PPWR | PENA);
        assert_eq!(r.read(HPRT_ADDR), PCSTS | POWERED_LOW_SPEED);
    }

    #[test]
    fn a_reset_disables_the_port_and_resets_the_device() {
        let (device, log) = script(1, vec![]);
        let mut r = rig();
        r.port.attach(device);
        r.enabled_port();
        let before = log.borrow().resets;
        r.write(HPRT_ADDR, PPWR | PRST);
        assert_eq!(r.read(HPRT_ADDR) & PENA, 0);
        assert_eq!(log.borrow().resets, before + 1);
        r.write(HPRT_ADDR, PPWR);
        r.advance_us(10);
        assert_eq!(r.read(HPRT_ADDR) & PENA, PENA);
        assert_eq!(
            log.borrow().resets,
            before + 1,
            "only the start of a reset resets it"
        );
    }

    #[test]
    fn removing_the_power_drops_the_connection() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.write(HPRT_ADDR, 0);
        assert_eq!(r.read(HPRT_ADDR) & (PCSTS | PENA), 0);
        r.advance_us(1_000);
        assert_eq!(r.read(HPRT_ADDR) & PCSTS, 0);
    }

    #[test]
    fn unplugging_a_device_sets_the_disconnect_flags() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.interrupts_on(HPRTINT | DISCINT);
        r.irqs.take();
        r.port.detach();
        r.advance_cycles(1);
        assert_eq!(r.read(HPRT_ADDR), PENCHNG | PPWR);
        assert_eq!(r.read(GINTSTS_ADDR) & DISCINT, DISCINT);
        assert!(r.raised());
        r.write(GINTSTS_ADDR, DISCINT);
        assert_eq!(r.read(GINTSTS_ADDR) & DISCINT, 0);
    }

    #[test]
    fn unplugging_a_device_that_was_not_enabled_leaves_no_enable_change() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.power();
        r.port.detach();
        r.advance_cycles(1);
        assert_eq!(r.read(HPRT_ADDR), PCDET | PPWR);
        assert_eq!(r.read(GINTSTS_ADDR) & DISCINT, DISCINT);
    }

    #[test]
    fn plugging_a_device_in_again_is_a_new_connect() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.port.detach();
        r.advance_us(1);
        r.write(HPRT_ADDR, PPWR | PENCHNG);
        r.port.attach(script(1, vec![]).0);
        r.advance_us(201);
        assert_eq!(r.read(HPRT_ADDR), PCSTS | PCDET | POWERED_LOW_SPEED);
    }

    #[test]
    fn plugging_a_device_over_another_disconnects_and_reconnects() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.port.attach(script(1, vec![]).0);
        r.advance_cycles(1);
        assert_eq!(r.read(HPRT_ADDR), PENCHNG | PPWR);
        assert_eq!(r.read(GINTSTS_ADDR) & DISCINT, DISCINT);
        r.advance_us(200);
        assert_eq!(
            r.read(HPRT_ADDR),
            PCSTS | PCDET | PENCHNG | POWERED_LOW_SPEED
        );
    }

    #[test]
    fn a_system_reset_disconnects_and_the_device_comes_back_when_the_port_is_powered() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.m.reset_peripherals(ResetCause::PowerOn);
        r.advance_us(1_000);
        assert_eq!(r.read(HPRT_ADDR), 0);
        r.power();
        assert_eq!(r.read(HPRT_ADDR), PCSTS | PCDET | POWERED_LOW_SPEED);
    }

    // The frame counter and the start of frame.

    #[test]
    fn the_frame_number_counts_milliseconds() {
        let mut r = rig();
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 0);
        r.advance_cycles(3 * CYCLES_PER_FRAME + 5);
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 3);
        r.advance_cycles(CYCLES_PER_FRAME);
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 4);
    }

    #[test]
    fn the_frame_number_counts_to_0x3fff_and_starts_again() {
        let mut r = rig();
        r.clock.set(300 * CYCLES_PER_FRAME);
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 300);
        r.clock.set(0x3FFF * CYCLES_PER_FRAME);
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 0x3FFF);
        r.clock.set(0x4000 * CYCLES_PER_FRAME);
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 0);
        r.clock.set(0x4003 * CYCLES_PER_FRAME);
        assert_eq!(r.read(OTG_BASE + HFNUM) & 0xFFFF, 3);
    }

    #[test]
    fn the_frame_time_remaining_counts_down_in_the_frame_interval() {
        let mut r = rig();
        r.write(OTG_BASE + HFIR, 6_000);
        assert_eq!(r.read(OTG_BASE + HFNUM) >> 16, 6_000);
        r.advance_cycles(CYCLES_PER_FRAME / 2);
        assert_eq!(r.read(OTG_BASE + HFNUM) >> 16, 3_000);
        r.advance_cycles(CYCLES_PER_FRAME / 2 - 1);
        assert_eq!(r.read(OTG_BASE + HFNUM) >> 16, 0);
        r.advance_cycles(1);
        assert_eq!(r.read(OTG_BASE + HFNUM) >> 16, 6_000);
    }

    #[test]
    fn the_start_of_frame_flag_is_set_each_millisecond_and_clears_by_writing() {
        let mut r = rig();
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, 0);
        r.advance_cycles(CYCLES_PER_FRAME - 1);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, 0);
        r.advance_cycles(1);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, SOF);
        r.write(GINTSTS_ADDR, SOF);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, 0);
        r.advance_cycles(CYCLES_PER_FRAME - 1);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, 0);
        r.advance_cycles(1);
        assert_eq!(r.read(GINTSTS_ADDR) & SOF, SOF);
    }

    #[test]
    fn an_unmasked_start_of_frame_raises_the_line_every_millisecond() {
        let mut r = rig();
        r.interrupts_on(SOF);
        r.irqs.take();
        for frame in 1..=5 {
            r.advance_cycles(CYCLES_PER_FRAME);
            assert!(r.raised(), "frame {frame}");
            assert_eq!(r.read(GINTSTS_ADDR) & SOF, SOF);
            r.write(GINTSTS_ADDR, SOF);
        }
    }

    #[test]
    fn the_wake_is_asked_for_again_after_each_one_comes_due() {
        let mut r = rig();
        r.interrupts_on(SOF);
        let mut wakes = 0;
        for _ in 0..4 {
            r.clock.set(r.clock.now() + CYCLES_PER_FRAME);
            if r.clock.take_due() {
                wakes += 1;
                r.m.tick();
            }
        }
        assert_eq!(wakes, 4);
    }

    #[test]
    fn a_masked_start_of_frame_costs_no_wakes() {
        let mut r = rig();
        r.interrupts_on(SOF);
        r.write(GINTMSK_ADDR, 0);
        // The wake that was already asked for comes due once, and is not asked for again.
        r.advance_cycles(CYCLES_PER_FRAME);
        r.clock.set(r.clock.now() + 100 * CYCLES_PER_FRAME);
        assert!(!r.clock.take_due());
    }

    #[test]
    fn a_quiet_core_with_nothing_attached_never_asks_to_be_woken() {
        let mut r = rig();
        r.interrupts_on(HPRTINT | RXFLVL | HCINT | DISCINT);
        r.write(HPRT_ADDR, PPWR);
        r.write(OTG_BASE + HCFG, 6);
        r.clock.set(100 * CYCLES_PER_FRAME);
        assert!(!r.clock.take_due());
    }

    // Transfers.

    #[test]
    fn a_setup_stage_goes_to_the_device_once_both_words_are_in_the_fifo() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hcint(0), 0xFFFF_FFFF);
        r.write(hctsiz(0), tsiz(SETUP, 1, 8));
        r.write(hcchar(0), chan(0, 0, false));
        r.write(FIFO0, 0x0100_0680);
        r.advance_us(1_000);
        assert!(log.borrow().setups.is_empty(), "one word is not a packet");
        r.write(FIFO0, 0x0008_0000);
        r.advance_cycles(TRANSACTION_DELAY - 1);
        assert!(log.borrow().setups.is_empty(), "the transaction takes time");
        r.advance_cycles(1);
        assert_eq!(log.borrow().setups, [[0x80, 6, 0, 1, 0, 0, 8, 0]]);
    }

    #[test]
    fn a_setup_stage_completes_with_xfrc_and_ack_and_leaves_data1_next() {
        let (device, _) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.interrupts_on(HCINT);
        r.irqs.take();
        r.setup(0, 0, [0x80, 6, 0, 1, 0, 0, 8, 0]);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA1, 0, 0));
        assert_eq!(r.read(HAINT_ADDR), 1);
        assert_eq!(r.read(GINTSTS_ADDR) & HCINT, HCINT);
        assert!(r.raised());
        // The channel is still enabled until the firmware halts it.
        assert_eq!(r.read(hcchar(0)) & CHENA, CHENA);
    }

    #[test]
    fn the_channel_interrupt_needs_its_masks() {
        let (device, _) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hcintmsk(0), 0);
        r.interrupts_on(HCINT);
        r.irqs.take();
        r.setup(0, 0, [0; 8]);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.read(HAINT_ADDR), 0, "HCINTMSK");
        assert_eq!(r.read(GINTSTS_ADDR) & HCINT, 0);
        assert!(r.irqs.take().is_none());
        r.write(hcintmsk(0), 0x4D9);
        assert_eq!(r.read(HAINT_ADDR), 1);
        r.write(HAINTMSK_ADDR, 0);
        assert_eq!(r.read(HAINT_ADDR), 1, "HAINT is not masked by HAINTMSK");
        assert_eq!(r.read(GINTSTS_ADDR) & HCINT, 0, "HAINTMSK");
    }

    #[test]
    fn the_channel_flags_clear_by_writing_ones() {
        let (device, _) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.setup(0, 0, [0; 8]);
        r.advance_us(1_000);
        r.write(hcint(0), ACK);
        assert_eq!(r.read(hcint(0)), XFRC);
        r.write(hcint(0), XFRC);
        assert_eq!(r.read(hcint(0)), 0);
        assert_eq!(r.read(HAINT_ADDR), 0);
    }

    #[test]
    fn an_in_transfer_fills_the_rx_fifo_with_status_words_and_data() {
        let descriptor: Vec<u8> = (0..18).collect();
        let (device, log) = script(
            0,
            vec![
                In::Data(descriptor[..8].to_vec()),
                In::Data(descriptor[8..16].to_vec()),
                In::Data(descriptor[16..].to_vec()),
            ],
        );
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 3, 18);
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins, [(0, 8), (0, 8), (0, 8)]);

        let packets = r.drain_rx();
        let statuses: Vec<u32> = packets.iter().map(|p| p.0).collect();
        // Data received (2), the byte count, the toggle of the packet, the channel.
        assert_eq!(
            statuses,
            [
                2 << 17 | DATA1 << 15 | 8 << 4,
                2 << 17 | 8 << 4,
                2 << 17 | DATA1 << 15 | 2 << 4,
                3 << 17,
            ]
        );
        let data: Vec<u8> = packets.iter().flat_map(|p| p.1.clone()).collect();
        assert_eq!(data, descriptor);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA0, 0, 0));
    }

    #[test]
    fn the_rx_status_names_the_channel() {
        let (device, _) = script(0, vec![In::Data(vec![1, 2, 3])]);
        let mut r = Rig::with(device);
        r.start_in(1, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        let packets = r.drain_rx();
        assert_eq!(packets[0].0 & 0xF, 1);
        assert_eq!(packets[0].0 >> 4 & 0x7FF, 3);
        assert_eq!(packets[0].1, [1, 2, 3]);
        assert_eq!(packets[1].0, 3 << 17 | 1);
    }

    #[test]
    fn the_rx_status_can_be_peeked_before_it_is_popped() {
        let (device, _) = script(0, vec![In::Data(vec![9; 4])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        let peeked = r.read(OTG_BASE + GRXSTSR);
        assert_eq!(peeked, 2 << 17 | 4 << 4);
        assert_eq!(r.read(OTG_BASE + GRXSTSR), peeked);
        assert_eq!(r.read(OTG_BASE + GRXSTSP), peeked);
        assert_ne!(r.read(OTG_BASE + GRXSTSR), peeked);
    }

    #[test]
    fn an_empty_rx_fifo_reads_zero() {
        let mut r = rig();
        assert_eq!(r.read(OTG_BASE + GRXSTSR), 0);
        assert_eq!(r.read(OTG_BASE + GRXSTSP), 0);
        assert_eq!(r.read(FIFO0), 0);
    }

    #[test]
    fn the_data_is_read_from_any_fifo_window() {
        let (device, _) = script(0, vec![In::Data(vec![1, 2, 3, 4, 5, 6, 7, 8])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        r.read(OTG_BASE + GRXSTSP);
        assert_eq!(r.read(FIFO0 + 0x3000), 0x0403_0201);
        assert_eq!(r.read(FIFO0 + 0x1000), 0x0807_0605);
    }

    #[test]
    fn the_rx_level_flag_follows_the_fifo_and_raises_the_line() {
        let (device, _) = script(0, vec![In::Data(vec![1])]);
        let mut r = Rig::with(device);
        r.interrupts_on(RXFLVL);
        r.irqs.take();
        r.start_in(0, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert!(r.raised());
        assert_eq!(r.read(GINTSTS_ADDR) & RXFLVL, RXFLVL);
        r.drain_rx();
        assert_eq!(r.read(GINTSTS_ADDR) & RXFLVL, 0);
    }

    #[test]
    fn popping_a_status_with_more_waiting_raises_the_line_again() {
        let (device, _) = script(0, vec![In::Data(vec![1])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        r.interrupts_on(RXFLVL);
        r.irqs.take();
        r.read(OTG_BASE + GRXSTSP);
        // The data and then the transfer-complete status are still there.
        assert!(r.raised());
    }

    #[test]
    fn flushing_the_rx_fifo_empties_it() {
        let (device, _) = script(0, vec![In::Data(vec![1])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        r.write(OTG_BASE + GRSTCTL, RXFFLSH);
        assert_eq!(r.read(GINTSTS_ADDR) & RXFLVL, 0);
    }

    #[test]
    fn a_short_packet_ends_an_in_transfer_early() {
        let (device, log) = script(0, vec![In::Data(vec![1, 2, 3, 4])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 32, 255);
        r.advance_us(10_000);
        assert_eq!(log.borrow().ins.len(), 1);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA0, 31, 251));
    }

    #[test]
    fn a_full_packet_asks_for_the_next_one_a_transaction_later() {
        let (device, log) = script(0, vec![In::Data(vec![0; 8]), In::Data(vec![0; 8])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 3, 24);
        r.advance_cycles(TRANSACTION_DELAY);
        assert_eq!(log.borrow().ins.len(), 1);
        assert_eq!(r.read(hcint(0)) & XFRC, 0);
        r.advance_cycles(TRANSACTION_DELAY);
        assert_eq!(log.borrow().ins.len(), 2);
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA1, 1, 8));
        // The device has nothing for the third, and says so.
        r.advance_cycles(TRANSACTION_DELAY);
        assert_eq!(r.read(hcint(0)) & NAK, NAK);
    }

    #[test]
    fn a_zero_length_in_packet_is_a_status_stage() {
        let (device, _) = script(0, vec![In::Data(vec![])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 1, 0);
        r.advance_us(1_000);
        let packets = r.drain_rx();
        assert_eq!(packets[0], (2 << 17 | DATA1 << 15, vec![]));
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA0, 0, 0));
    }

    #[test]
    fn the_transfer_goes_to_the_endpoint_and_packet_size_the_channel_names() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 8));
        r.write(hcchar(0), CHENA | 1 << 15 | 3 << 11 | 64);
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins, [(3, 64)]);
    }

    #[test]
    fn a_nak_sets_nak_and_leaves_the_transfer_to_be_tried_again() {
        let (device, _) = script(1, vec![]);
        let mut r = Rig::with(device);
        r.interrupts_on(HCINT);
        r.irqs.take();
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(1)), NAK);
        assert_eq!(r.read(hctsiz(1)), tsiz(DATA0, 1, 8));
        assert_eq!(r.read(GINTSTS_ADDR) & RXFLVL, 0);
        assert_eq!(r.read(HAINT_ADDR), 2);
        assert!(r.raised());
    }

    #[test]
    fn a_stall_sets_stall() {
        let (device, _) = script(0, vec![In::Stall]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), STALL);
    }

    #[test]
    fn a_device_that_stalls_the_out_packet_is_heard() {
        let (mut device, _) = script(0, vec![]);
        device.out_reply = Out::Stall;
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA1, 1, 0));
        r.write(hcchar(0), chan(0, 0, false));
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), STALL);
    }

    #[test]
    fn a_device_that_naks_the_out_packet_is_heard() {
        let (mut device, _) = script(0, vec![]);
        device.out_reply = Out::Nak;
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA1, 1, 0));
        r.write(hcchar(0), chan(0, 0, false));
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), NAK);
    }

    #[test]
    fn no_answer_is_a_transaction_error() {
        // The wrong address.
        let (device, log) = script(1, vec![In::Data(vec![1])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), TXERR);
        assert!(log.borrow().ins.is_empty());
        // No device.
        let mut r = rig();
        r.start_in(0, 1, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), TXERR);
        // A port that is not enabled.
        let (device, log) = script(0, vec![]);
        let mut r = rig();
        r.port.attach(device);
        r.power();
        r.start_in(0, 0, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), TXERR);
        assert!(log.borrow().ins.is_empty());
    }

    #[test]
    fn the_data_toggle_is_left_in_hctsiz_for_the_next_poll() {
        let (device, _) = script(
            1,
            vec![
                In::Data(vec![1; 8]),
                In::Data(vec![2; 8]),
                In::Data(vec![3; 8]),
            ],
        );
        let mut r = Rig::with(device);
        let mut dpid = DATA0;
        for (poll, byte) in [1, 2, 3].into_iter().enumerate() {
            r.start_in(1, 1, 1, dpid, 1, 8);
            r.advance_us(1_000);
            let packets = r.drain_rx();
            // The packet is stamped with the toggle the firmware asked for.
            assert_eq!(packets[0].0 >> 15 & 3, dpid, "poll {poll}");
            assert_eq!(packets[0].1, [byte; 8]);
            // The firmware reads the toggle back out of HCTSIZ for the poll after.
            dpid = r.read(hctsiz(1)) >> 29;
            assert_eq!(
                dpid,
                if poll % 2 == 0 { DATA1 } else { DATA0 },
                "poll {poll}"
            );
        }
    }

    #[test]
    fn an_out_transfer_toggles_too() {
        let (device, _) = script(0, vec![]);
        let mut r = Rig::with(device);
        for (dpid, next) in [(DATA0, DATA1), (DATA1, DATA0)] {
            r.write(hctsiz(0), tsiz(dpid, 1, 0));
            r.write(hcchar(0), chan(0, 0, false));
            r.advance_us(1_000);
            assert_eq!(r.read(hctsiz(0)), tsiz(next, 0, 0));
        }
    }

    #[test]
    fn a_zero_length_out_packet_needs_no_fifo_data() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hcint(0), 0xFFFF_FFFF);
        r.write(hctsiz(0), tsiz(DATA1, 1, 0));
        r.write(hcchar(0), chan(0, 0, false));
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(0, vec![])]);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
    }

    #[test]
    fn an_out_transfer_sends_a_packet_when_it_has_one_and_ends_with_the_last() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 2, 12));
        r.write(hcchar(0), chan(0, 0, false));
        r.write(FIFO0, 0x0403_0201);
        r.advance_us(1_000);
        assert!(log.borrow().outs.is_empty());
        r.write(FIFO0, 0x0807_0605);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(0, vec![1, 2, 3, 4, 5, 6, 7, 8])]);
        assert_eq!(r.read(hcint(0)) & XFRC, 0, "a packet is left");
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA1, 1, 4));
        r.write(FIFO0, 0x0C0B_0A09);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs[1], (0, vec![9, 10, 11, 12]));
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.read(hctsiz(0)), tsiz(DATA0, 0, 0));
    }

    #[test]
    fn a_short_last_out_packet_drops_the_padding_of_its_last_word() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 1));
        r.write(hcchar(0), chan(0, 0, false));
        r.write(FIFO0, 0xFFFF_FF01);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(0, vec![1])]);
    }

    #[test]
    fn each_channel_has_its_own_fifo_window() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 4));
        r.write(hcchar(0), chan(0, 1, false));
        r.write(hctsiz(1), tsiz(DATA0, 1, 4));
        r.write(hcchar(1), chan(0, 2, false));
        r.write(FIFO0 + 0x1000, 0x2222_2222);
        r.write(FIFO0, 0x1111_1111);
        r.advance_us(1_000);
        let outs = log.borrow().outs.clone();
        assert!(outs.contains(&(1, vec![0x11; 4])), "{outs:?}");
        assert!(outs.contains(&(2, vec![0x22; 4])), "{outs:?}");
    }

    #[test]
    fn a_narrow_write_to_the_fifo_adds_only_the_bytes_written() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 7));
        r.write(hcchar(0), chan(0, 1, false));
        // Bytes and a halfword, wherever in the word they land, in the order they were written.
        r.m.write8(FIFO0, 1);
        r.m.write8(FIFO0 + 2, 2);
        r.m.write16(FIFO0 + 2, 0x0403);
        r.advance_us(1_000);
        assert!(
            log.borrow().outs.is_empty(),
            "four bytes are not a packet of 7"
        );
        r.m.write16(FIFO0, 0x0605);
        r.m.write8(FIFO0 + 1, 7);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(1, vec![1, 2, 3, 4, 5, 6, 7])]);
    }

    #[test]
    fn data_written_to_a_channel_that_is_not_sending_is_dropped() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(FIFO0, 0x1111_1111);
        // An IN channel does not stage it either, or make another transaction of it.
        r.start_in(0, 0, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        r.write(FIFO0, 0x2222_2222);
        r.advance_us(1_000);
        assert!(log.borrow().outs.is_empty());
        assert_eq!(log.borrow().ins.len(), 1, "no second transaction");
        // And a completed transfer does not send it again.
        r.write(hctsiz(1), tsiz(DATA0, 1, 4));
        r.write(hcchar(1), chan(0, 1, false));
        r.write(FIFO0 + 0x1000, 0x3333_3333);
        r.advance_us(1_000);
        r.write(FIFO0 + 0x1000, 0x4444_4444);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(1, vec![0x33; 4])]);
    }

    #[test]
    fn a_halt_request_sets_chh_and_clears_the_channel_enable() {
        let (device, _) = script(1, vec![]);
        let mut r = Rig::with(device);
        r.interrupts_on(HCINT);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        r.write(hcint(1), NAK);
        r.write(hcintmsk(1), 0x7BB);
        r.irqs.take();
        let char = r.read(hcchar(1));
        assert_eq!(char & CHENA, CHENA);
        r.write(hcchar(1), char | CHDIS);
        assert_eq!(r.read(hcint(1)), CHH);
        assert_eq!(r.read(hcchar(1)) & (CHENA | CHDIS), 0);
        assert!(r.raised());
    }

    #[test]
    fn a_halt_cancels_a_transfer_that_has_not_happened_yet() {
        let (device, log) = script(1, vec![In::Data(vec![1])]);
        let mut r = Rig::with(device);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        let char = r.read(hcchar(1));
        r.write(hcchar(1), char | CHDIS);
        r.advance_us(1_000);
        assert!(log.borrow().ins.is_empty());
        assert_eq!(r.read(hcint(1)), CHH);
    }

    #[test]
    fn writing_hcchar_again_with_the_enable_bit_starts_another_transfer() {
        let (device, log) = script(1, vec![In::Nak, In::Data(vec![7])]);
        let mut r = Rig::with(device);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(1)), NAK);
        r.write(hcchar(1), chan(1, 1, true));
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins.len(), 2);
        assert_eq!(r.read(hcint(1)), NAK | XFRC | ACK);
    }

    #[test]
    fn a_channel_past_the_last_one_is_only_stored() {
        let mut r = rig();
        let past = OTG_BASE + CHANNEL0 + CHANNEL_STRIDE * CHANNELS as u32;
        r.write(past, 0xFFFF_FFFF);
        assert_eq!(r.read(past), 0xFFFF_FFFF);
        assert_eq!(r.read(hcchar(CHANNELS as u32 - 1)), 0);
    }

    #[test]
    fn a_device_address_can_be_any_seven_bits() {
        let (device, log) = script(100, vec![]);
        let mut r = Rig::with(device);
        r.start_in(0, 100, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins.len(), 1);
    }

    #[test]
    fn restarting_a_transfer_forgets_the_data_written_for_the_last_try() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 8));
        r.write(hcchar(0), chan(0, 1, false));
        r.write(FIFO0, 1);
        r.write(hcchar(0), chan(0, 1, false));
        r.write(FIFO0, 2);
        r.advance_us(1_000);
        assert!(log.borrow().outs.is_empty(), "half a packet is not one");
    }

    #[test]
    fn an_out_transfer_of_whole_packets_ends_with_the_zero_length_packet_it_counted() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 2, 8));
        r.write(hcchar(0), chan(0, 1, false));
        r.write(FIFO0, 0x0403_0201);
        r.write(FIFO0, 0x0807_0605);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs.len(), 2);
        assert_eq!(log.borrow().outs[1], (1, vec![]));
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
    }

    #[test]
    fn an_in_transfer_ends_when_the_size_or_the_packet_count_runs_out() {
        let full = vec![0; 8];
        let (device, log) = script(0, vec![In::Data(full.clone()), In::Data(full.clone())]);
        let mut r = Rig::with(device);
        // The size is used up with a packet to spare.
        r.start_in(0, 0, 1, DATA0, 2, 8);
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins.len(), 1);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        // The packets are used up with size to spare.
        r.start_in(0, 0, 1, DATA0, 1, 16);
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins.len(), 2);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
    }

    #[test]
    fn the_other_channel_registers_are_stored() {
        let mut r = rig();
        let hcsplt = OTG_BASE + CHANNEL0 + CHANNEL_STRIDE * 2 + 4;
        r.write(hcsplt, 0x8000_0001);
        assert_eq!(r.read(hcsplt), 0x8000_0001);
    }

    #[test]
    fn a_detach_clears_the_queues_and_cancels_what_is_in_flight() {
        let (device, log) = script(1, vec![In::Data(vec![1]), In::Data(vec![2])]);
        let mut r = Rig::with(device);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(GINTSTS_ADDR) & RXFLVL, RXFLVL);
        r.start_in(0, 1, 1, DATA0, 1, 8);
        r.port.detach();
        r.advance_us(1_000);
        assert_eq!(r.read(GINTSTS_ADDR) & RXFLVL, 0);
        assert_eq!(log.borrow().ins.len(), 1);
        assert_eq!(r.read(hcint(0)), 0, "the transfer never happened");
    }

    #[test]
    fn a_detach_drops_data_waiting_to_be_sent() {
        let (device, _) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 8));
        r.write(hcchar(0), chan(0, 1, false));
        r.write(FIFO0, 1);
        r.port.detach();
        r.advance_us(1_000);
        let (again, log) = script(0, vec![]);
        r.port.attach(again);
        r.advance_us(1_000);
        r.write(HPRT_ADDR, PPWR | PRST);
        r.write(HPRT_ADDR, PPWR);
        r.advance_us(10);
        // The channel is still enabled, and the one word it had is gone: one more is not a packet.
        r.write(FIFO0, 2);
        r.advance_us(1_000);
        assert!(log.borrow().outs.is_empty());
    }

    // The device and the core, when either starts again.

    #[test]
    fn a_system_reset_resets_the_device() {
        let (device, log) = script(1, vec![]);
        let mut r = Rig::with(device);
        let before = log.borrow().resets;
        r.m.reset_peripherals(ResetCause::Software);
        assert_eq!(log.borrow().resets, before + 1);
    }

    #[test]
    fn removing_the_power_resets_the_device() {
        let (device, log) = script(1, vec![]);
        let mut r = Rig::with(device);
        let before = log.borrow().resets;
        r.write(HPRT_ADDR, 0);
        assert_eq!(log.borrow().resets, before + 1);
    }

    #[test]
    fn a_device_that_never_connected_is_not_reset_by_the_power_going() {
        let (device, log) = script(1, vec![]);
        let mut r = rig();
        r.port.attach(device);
        r.write(HPRT_ADDR, PPWR);
        r.write(HPRT_ADDR, 0);
        assert_eq!(log.borrow().resets, 0);
    }

    #[test]
    fn a_system_reset_makes_the_keyboard_unpolled_and_unsent() {
        let keys = KeyReport::new();
        let mut r = Rig::with(HidKeyboard::new(keys.clone()));
        r.setup(0, 0, [0x00, 5, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        r.start_in(0, 0, 0, DATA1, 1, 0);
        r.advance_us(1_000);
        r.setup(0, 1, [0x00, 9, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert!(keys.polled() && keys.was_sent());
        r.m.reset_peripherals(ResetCause::Software);
        assert!(!keys.polled() && !keys.was_sent());
    }

    #[test]
    fn removing_the_power_makes_the_keyboard_unpolled() {
        let keys = KeyReport::new();
        let mut r = Rig::with(HidKeyboard::new(keys.clone()));
        r.setup(0, 0, [0x00, 5, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        r.start_in(0, 0, 0, DATA1, 1, 0);
        r.advance_us(1_000);
        r.setup(0, 1, [0x00, 9, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert!(keys.polled());
        r.write(HPRT_ADDR, 0);
        assert!(!keys.polled() && !keys.was_sent());
    }

    #[test]
    fn a_core_soft_reset_idles_the_channels_and_empties_the_queues() {
        let (device, log) = script(1, vec![In::Data(vec![1, 2, 3, 4]), In::Data(vec![5])]);
        let mut r = Rig::with(device);
        r.interrupts_on(HCINT);
        // A transfer that has run and left data in the RX FIFO, and one still waiting.
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        r.start_in(0, 1, 1, DATA0, 1, 8);
        assert_eq!(r.read(GINTSTS_ADDR) & (RXFLVL | HCINT), RXFLVL | HCINT);
        r.write(GRSTCTL_ADDR, CSRST);
        for n in 0..2 {
            assert_eq!(r.read(hcchar(n)), 0, "channel {n}");
            assert_eq!(r.read(hcint(n)), 0, "channel {n}");
            assert_eq!(r.read(hcintmsk(n)), 0, "channel {n}");
            assert_eq!(r.read(hctsiz(n)), 0, "channel {n}");
        }
        assert_eq!(r.read(HAINT_ADDR), 0);
        assert_eq!(r.read(GINTSTS_ADDR) & (RXFLVL | HCINT), 0);
        r.advance_us(1_000);
        assert_eq!(log.borrow().ins.len(), 1, "the waiting transfer is gone");
    }

    #[test]
    fn a_core_soft_reset_puts_the_registers_back_except_the_speed_bits() {
        let mut r = rig();
        r.write(OTG_BASE + GUSBCFG, 0xFFFF);
        r.write(OTG_BASE + HCFG, 0x2);
        r.write(OTG_BASE + HFIR, 1234);
        r.interrupts_on(HPRTINT);
        r.write(HAINTMSK_ADDR, 0xFFF);
        r.write(GRSTCTL_ADDR, CSRST);
        assert_eq!(r.read(OTG_BASE + GUSBCFG), 0x1440);
        assert_eq!(r.read(OTG_BASE + HCFG), 0x200 | 0x2);
        assert_eq!(r.read(OTG_BASE + HFIR), 0xEA60);
        assert_eq!(r.read(GINTMSK_ADDR), 0);
        assert_eq!(r.read(GAHBCFG_ADDR), 0);
        assert_eq!(r.read(HAINTMSK_ADDR), 0);
    }

    #[test]
    fn a_core_soft_reset_leaves_the_power_of_the_port() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.write(HPRT_ADDR, PPWR);
        r.write(GRSTCTL_ADDR, CSRST);
        assert_eq!(r.read(HPRT_ADDR) & PPWR, PPWR);
        r.advance_us(200);
        assert_eq!(r.read(HPRT_ADDR) & PCSTS, PCSTS);
    }

    #[test]
    fn a_core_soft_reset_clears_the_latched_flags_and_leaves_the_port() {
        let mut r = rig();
        r.port.attach(script(1, vec![]).0);
        r.enabled_port();
        r.port.detach();
        r.advance_us(1);
        assert_eq!(r.read(GINTSTS_ADDR) & DISCINT, DISCINT);
        r.write(GRSTCTL_ADDR, CSRST);
        assert_eq!(r.read(GINTSTS_ADDR) & DISCINT, 0);
        let (device, _) = script(1, vec![]);
        let mut r = Rig::with(device);
        let hprt = r.read(HPRT_ADDR);
        r.write(GRSTCTL_ADDR, CSRST);
        assert_eq!(r.read(HPRT_ADDR), hprt, "the device stays connected");
    }

    #[test]
    fn a_tx_flush_drops_the_data_staged_for_sending() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 8));
        r.write(hcchar(0), chan(0, 1, false));
        r.write(FIFO0, 1);
        r.write(GRSTCTL_ADDR, TXFFLSH);
        // The word that was in the FIFO is gone, so one more is half a packet.
        r.write(FIFO0, 2);
        r.advance_us(1_000);
        assert!(log.borrow().outs.is_empty());
        r.write(FIFO0, 3);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(1, vec![2, 0, 0, 0, 3, 0, 0, 0])]);
    }

    #[test]
    fn a_tx_flush_cancels_a_packet_that_was_ready_to_go() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA0, 1, 4));
        r.write(hcchar(0), chan(0, 1, false));
        r.write(FIFO0, 1);
        r.write(GRSTCTL_ADDR, TXFFLSH);
        r.advance_us(1_000);
        assert!(log.borrow().outs.is_empty());
    }

    // A channel for an interrupt endpoint, which sends through the periodic FIFO.
    fn interrupt_out(device: u32, endpoint: u32) -> u32 {
        chan(device, endpoint, false) | 3 << 18
    }

    #[test]
    fn a_tx_flush_empties_the_fifo_it_names_and_the_one_for_all() {
        for (txfnum, flushed) in [(0, [true, false]), (1, [false, true]), (0x10, [true, true])] {
            let (device, log) = script(0, vec![]);
            let mut r = Rig::with(device);
            // Channel 0 is a control one (non-periodic), channel 1 an interrupt one (periodic).
            r.write(hctsiz(0), tsiz(DATA0, 1, 4));
            r.write(hcchar(0), chan(0, 0, false));
            r.write(hctsiz(1), tsiz(DATA0, 1, 4));
            r.write(hcchar(1), interrupt_out(0, 1));
            r.write(FIFO0, 1);
            r.write(FIFO0 + 0x1000, 2);
            r.write(GRSTCTL_ADDR, TXFFLSH | txfnum << 6);
            r.advance_us(1_000);
            let sent: Vec<u8> = log
                .borrow()
                .outs
                .iter()
                .map(|(endpoint, _)| *endpoint)
                .collect();
            let expected: Vec<u8> = [(0, flushed[0]), (1, flushed[1])]
                .into_iter()
                .filter(|&(_, flushed)| !flushed)
                .map(|(endpoint, _)| endpoint)
                .collect();
            assert_eq!(sent, expected, "TXFNUM {txfnum:#x}");
        }
    }

    #[test]
    fn a_tx_flush_does_not_cancel_a_zero_length_out_with_nothing_in_the_fifo() {
        let (device, log) = script(0, vec![]);
        let mut r = Rig::with(device);
        r.write(hctsiz(0), tsiz(DATA1, 1, 0));
        r.write(hcchar(0), chan(0, 0, false));
        r.write(GRSTCTL_ADDR, TXFFLSH | 0x10 << 6);
        r.advance_us(1_000);
        assert_eq!(log.borrow().outs, [(0, vec![])]);
    }

    #[test]
    fn a_nak_on_a_control_in_is_tried_again_when_the_firmware_starts_the_channel() {
        let (device, log) = script(0, vec![In::Nak, In::Data(vec![7, 8])]);
        let mut r = Rig::with(device);
        r.start_in(0, 0, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), NAK);
        assert_eq!(log.borrow().ins.len(), 1, "no retry of its own");
        // The halt, and the start again with the same transfer.
        r.write(hcchar(0), chan(0, 0, true) | CHDIS);
        assert_eq!(r.read(hcint(0)), NAK | CHH);
        r.start_in(0, 0, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        assert_eq!(r.drain_rx()[0].1, [7, 8]);
    }

    // The keyboard, through the registers.

    #[test]
    fn the_keyboard_enumerates_and_reports_through_the_channels() {
        let keys = KeyReport::new();
        let mut r = Rig::with(HidKeyboard::new(keys.clone()));

        // GET_DESCRIPTOR (device, 8 bytes), the data stage IN, and the status stage OUT.
        r.setup(0, 0, [0x80, 6, 0, 1, 0, 0, 8, 0]);
        r.advance_us(1_000);
        r.start_in(0, 0, 0, DATA1, 1, 8);
        r.advance_us(1_000);
        let packets = r.drain_rx();
        assert_eq!(packets[0].1, [0x12, 1, 0x10, 1, 0, 0, 0, 8]);
        r.write(hcint(0), 0xFFFF_FFFF);
        r.write(hctsiz(0), tsiz(DATA1, 1, 0));
        r.write(hcchar(0), chan(0, 0, false));
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);

        // SET_ADDRESS 1, and its status stage, which is when the address takes.
        r.setup(0, 0, [0x00, 5, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        r.start_in(0, 0, 0, DATA1, 1, 0);
        r.advance_us(1_000);
        r.drain_rx();
        // The device now answers at address 1 and not at 0.
        r.setup(0, 1, [0x00, 9, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), XFRC | ACK);
        r.setup(0, 0, [0x00, 9, 1, 0, 0, 0, 0, 0]);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(0)), TXERR);
        r.start_in(0, 1, 0, DATA1, 1, 0);
        r.advance_us(1_000);
        r.drain_rx();

        // Polling endpoint 1: NAK, then a report.
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.read(hcint(1)), NAK);
        keys.set([0, 0, 4, 0, 0, 0, 0, 0]);
        r.start_in(1, 1, 1, DATA0, 1, 8);
        r.advance_us(1_000);
        assert_eq!(r.drain_rx()[0].1, [0, 0, 4, 0, 0, 0, 0, 0]);
    }
}
