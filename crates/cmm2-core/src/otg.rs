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
use crate::usb::UsbPort;

mod channel;
mod port;
mod regs;
use channel::Channel;
use regs::{
    AHBIDL, CMOD, CSRST, CSRST_DONE, CSRST_KEEPS, FIFO, FIFO_STRIDE, GAHBCFG, GAHBCFG_GINT,
    GHWCFG1, GHWCFG2, GHWCFG2_VALUE, GHWCFG3, GHWCFG3_VALUE, GHWCFG4, GINTMSK, GINTSTS, GNPTXSTS,
    GNPTXSTS_VALUE, GOTGINT, GRSTCTL, GRXSTSP, GRXSTSR, GSNPSID, GSNPSID_VALUE, GUSBCFG,
    GUSBCFG_RESET, HAINT, HAINTMSK, HCFG, HCFG_RESET, HCINT, HFIR, HFNUM, HPRT, HPRT_W1C, HPRTINT,
    HPTXSTS, HPTXSTS_VALUE, NPTXFE, PCDET, PCSTS, PENA, PENCHNG, PTXFE, RXFFLSH, RXFLVL, SOF,
    TXFFLSH, TXFNUM_ALL, TXFNUM_MASK, TXFNUM_SHIFT,
};

pub const OTG_BASE: u32 = 0x4008_0000;
/// The registers reach 0x680 and the FIFO windows 0x1000 + 0x1000 per channel.
pub const OTG_SIZE: u32 = 0x2_0000;

/// `OTG_FS_IRQn`.
const OTG_IRQ: u16 = 101;
const CHANNELS: usize = 12;

/// The frame number counts to 0x3FFF and starts again at 0, as RM0433 57.14.24 (p. 2653) gives
/// HFNUM.FRNUM ("cleared to 0 when it reaches 0x3FFF"; the register resets to 0x3FFF, so 0x3FFF
/// is a value it holds, and the model's period is 0x4000 frames).
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

    // A core soft reset (RM0433 57.14.5, p. 2629): the interrupts and all the CSR bits are cleared
    // except those the manual lists (kept below), the state machines go idle and the FIFOs are
    // flushed. So the channels, the RX queue, the flags the core latched and the port's control
    // and change bits (HPRT's) are cleared too: the port loses its power, as `sync_port` finds,
    // and the firmware has to power it again, which it does after the reset, since it does the
    // reset before it powers the port. What shows the line, PCSTS, is not a bit of the core's.
    // The model has no GATEHCLK or STPPCLK, which are kept like the registers they are in.
    fn soft_reset(&mut self) {
        let kept = CSRST_KEEPS.map(|(offset, mask)| (offset, mask, self.stored.read(offset)));
        self.stored.reset(ResetCause::Software);
        self.stored.write(GUSBCFG, GUSBCFG_RESET, !0);
        self.stored.write(HCFG, HCFG_RESET, !0);
        for (offset, mask, value) in kept {
            self.stored.write(offset, value, mask);
        }
        let state = &mut self.state;
        state.rx.clear();
        state.latched = 0;
        state.channels = Default::default();
        state.hprt &= PCSTS;
        state.enable_at = None;
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
mod tests;
