//! The host channels: the registers of one, and how it runs a transaction against the device
//! behind the port.

use crate::registers::Registers;
use crate::usb::{In, Out};

use super::regs::{
    ACK, CH_HCCHAR, CH_HCINT, CH_HCINTMSK, CH_HCTSIZ, CHANNEL_STRIDE, CHANNEL0, CHDIS, CHENA, CHH,
    DAD_SHIFT, DATA0, DATA1, DPID_SHIFT, EPDIR_IN, EPNUM_SHIFT, EPTYP_SHIFT, MPSIZ_MASK, NAK, PENA,
    PKTCNT_MASK, PKTCNT_SHIFT, PKTSTS_COMPLETE, PKTSTS_DATA, SETUP_DPID, STALL, TXERR, XFRC,
    XFRSIZ_MASK,
};
use super::{CHANNELS, OtgRegisters, TRANSACTION_DELAY};

/// One host channel: a transfer, in whole, that the firmware sets up and the model carries out a
/// packet at a time.
#[derive(Default)]
pub(super) struct Channel {
    hcchar: u32,
    pub(super) hcint: u32,
    pub(super) hcintmsk: u32,
    hctsiz: u32,
    /// The bytes the firmware has written to the FIFO window for an OUT or SETUP packet that
    /// has not been sent yet.
    pub(super) staged: Vec<u8>,
    /// The cycle the next transaction happens at.
    pub(super) due: Option<u64>,
}

impl Channel {
    fn input(&self) -> bool {
        self.hcchar & EPDIR_IN != 0
    }

    /// Isochronous and interrupt endpoints send through the periodic FIFO.
    pub(super) fn periodic(&self) -> bool {
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

impl OtgRegisters {
    // The channel and register of a channel that `offset` is.
    pub(super) fn channel_register(offset: u32) -> Option<(usize, u32)> {
        let rel = offset.checked_sub(CHANNEL0)?;
        let n = (rel / CHANNEL_STRIDE) as usize;
        (n < CHANNELS).then_some((n, rel % CHANNEL_STRIDE))
    }

    pub(super) fn channel_read(&mut self, n: usize, register: u32, offset: u32) -> u32 {
        let channel = &self.state.channels[n];
        match register {
            CH_HCCHAR => channel.hcchar,
            CH_HCINT => channel.hcint,
            CH_HCINTMSK => channel.hcintmsk,
            CH_HCTSIZ => channel.hctsiz,
            _ => self.stored.read(offset),
        }
    }

    pub(super) fn channel_write(
        &mut self,
        n: usize,
        register: u32,
        offset: u32,
        value: u32,
        mask: u32,
    ) {
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
    // selects, as a byte or halfword write has them. RM0433 only says that the registers are to
    // be accessed by words (57.13, p. 2614) and that an OUT channel's window is written (57.13.1,
    // pp. 2618 and 2619); it does not say what a narrower write does. The FIFO takes what it is
    // given: a narrow write is not widened to a word.
    pub(super) fn stage(&mut self, n: usize, word: u32, mask: u32) {
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
    pub(super) fn transact(&mut self, n: usize) {
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
