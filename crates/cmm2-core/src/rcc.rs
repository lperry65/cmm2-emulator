//! The reset and clock controller. The clocks start instantly: the firmware only waits for
//! the ready flags, so they read as set as soon as what they wait for is switched on.
//!
//! One clock is worked out from the registers: PLL3's R output, which is the LTDC's pixel
//! clock. The firmware sets it for each video mode, so the frame timing follows from it. The
//! other PLL outputs are only stored. PLL3 only locks (PLL3RDY) when the reference and the VCO
//! are in the chip's ranges; otherwise it has no clock and never becomes ready.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::clock::Clock;
use crate::peripheral::{ResetCause, StubRegisters};
use crate::registers::{Registers, WordAccess, follow};
use crate::svd::Svd;

pub const RCC_BASE: u32 = 0x5802_4400;
pub const RCC_SIZE: u32 = 0x400;

// Register offsets and bit positions, as the SVD gives them.
const CR: u32 = 0x00;
const CFGR: u32 = 0x10;
const PLLCKSELR: u32 = 0x28;
const PLLCFGR: u32 = 0x2C;
const PLL3DIVR: u32 = 0x40;
const PLL3FRACR: u32 = 0x44;
const BDCR: u32 = 0x70;
const CSR: u32 = 0x74;
const RSR: u32 = 0xD0;

// RCC_CR: each ready bit follows the on bit next to it in this table.
const READY_FOLLOWS_ON: [(u32, u32); 7] = [
    (1 << 0, 1 << 2),   // HSION, HSIRDY
    (1 << 7, 1 << 8),   // CSION, CSIRDY
    (1 << 12, 1 << 13), // RC48ON, RC48RDY
    (1 << 16, 1 << 17), // HSEON, HSERDY
    (1 << 24, 1 << 25), // PLL1ON, PLL1RDY
    (1 << 26, 1 << 27), // PLL2ON, PLL2RDY
    (1 << 28, 1 << 29), // PLL3ON, PLL3RDY
];
// RCC_CR: the board reads these as set whatever the on bits say.
const HSIDIVF: u32 = 1 << 5;
const PLL3ON: u32 = 1 << 28; // RCC_CR
const PLL3RDY: u32 = 1 << 29; // RCC_CR
const D1CKRDY: u32 = 1 << 14;
const D2CKRDY: u32 = 1 << 15;
const SW: u32 = 0b111; // RCC_CFGR
const SWS: u32 = 0b111 << 3; // RCC_CFGR
const LSEON: u32 = 1 << 0; // RCC_BDCR
const LSERDY: u32 = 1 << 1; // RCC_BDCR
const LSION: u32 = 1 << 0; // RCC_CSR
const LSIRDY: u32 = 1 << 1; // RCC_CSR
// RCC_RSR: the flags the model sets, the bit that clears them all, and every flag bit.
const RMVF: u32 = 1 << 16;
const PINRSTF: u32 = 1 << 22;
const SFTRSTF: u32 = 1 << 24;
const POWER_ON_FLAGS: u32 = 1 << 21 | 1 << 22 | 1 << 23; // BORRSTF, PINRSTF, PORRSTF
const FLAGS: u32 = 0x55FA_0000;

/// The board's crystal. PLL3 is fed from it, and the PLL sources the board does not use (the
/// internal oscillators) are not modelled.
const HSE_HZ: u64 = 8_000_000;
const PLLSRC_HSE: u32 = 2; // RCC_PLLCKSELR, bits 1:0
const PLL3FRACEN: u32 = 1 << 8; // RCC_PLLCFGR
const PLL3VCOSEL: u32 = 1 << 9; // RCC_PLLCFGR
const DIVR3EN: u32 = 1 << 24; // RCC_PLLCFGR
const DIVM3_BITS: u32 = 0x3F << 20; // RCC_PLLCKSELR
const PLLSRC_BITS: u32 = 3; // RCC_PLLCKSELR
const ANY_PLL_ON: u32 = 1 << 24 | 1 << 26 | PLL3ON; // RCC_CR
// RCC_PLLCFGR: DIVR3EN, DIVQ3EN, DIVP3EN, PLL3RGE and PLL3VCOSEL, not PLL3FRACEN.
const PLL3_CONFIG_BITS: u32 = 7 << 22 | 3 << 10 | PLL3VCOSEL;
// The registers that PLL3's R output depends on.
const PLL3_INPUTS: [u32; 5] = [CR, PLLCKSELR, PLLCFGR, PLL3DIVR, PLL3FRACR];

/// The pixel clock in Hz, as the RCC works it out from PLL3's registers, or 0 while PLL3 has no
/// R output. A cheap handle: clones share the value, so the LTDC can see what the firmware
/// programs here. Each change is logged with the core cycle it was made at, for the LTDC to take,
/// so that it sees every one however they come (a stop and a restart at the same rate too) and
/// when. A change also asks the clock for a tick at once, so the LTDC hears of it without the
/// firmware touching it.
#[derive(Clone, Debug)]
pub(crate) struct PixelClock(Rc<PixelClockState>);

#[derive(Debug)]
struct PixelClockState {
    clock: Clock,
    hz: Cell<u64>,
    changes: RefCell<Vec<(u64, u64)>>,
}

impl PixelClock {
    /// A clock that is stopped, as PLL3 is until the firmware sets it up.
    pub(crate) fn new(clock: Clock) -> PixelClock {
        PixelClock(Rc::new(PixelClockState {
            clock,
            hz: Cell::new(0),
            changes: RefCell::new(Vec::new()),
        }))
    }

    /// A clock that has been running at `hz` since the start, for a controller that has no RCC
    /// beside it.
    #[cfg(test)]
    pub(crate) fn running_at(clock: Clock, hz: u64) -> PixelClock {
        let pixel_clock = PixelClock::new(clock);
        pixel_clock.0.hz.set(hz);
        pixel_clock
    }

    pub(crate) fn hz(&self) -> u64 {
        self.0.hz.get()
    }

    /// The changes since the last call, oldest first, each as the core cycle and the new rate.
    pub(crate) fn take_changes(&self) -> Vec<(u64, u64)> {
        self.0.changes.take()
    }

    pub(crate) fn set(&self, hz: u64) {
        if hz != self.0.hz.get() {
            let now = self.0.clock.now();
            self.0.hz.set(hz);
            self.0.changes.borrow_mut().push((now, hz));
            self.0.clock.wake_at(now);
        }
    }
}

/// Plain registers with the SVD reset values, except for the status bits that the hardware
/// computes. Those are worked out on every read and ignore what is written to them.
///
/// RCC_RSR holds why the chip last reset, and RMVF clears it. RCC_BDCR is in the backup
/// domain, so a system reset leaves it as it was.
#[derive(Debug)]
pub struct RccRegisters {
    stored: StubRegisters,
    /// PLL3's SH_REG, the fraction the sigma-delta modulator works from: FRACN3 as it was when
    /// PLL3FRACEN last went from 0 to 1, and 0 from a reset (RM0433 8.5.5, p. 347).
    sh_reg: u32,
    rsr: u32,
    pixel_clock: PixelClock,
}

/// What PLL3's VCO comes to, for the source and dividers the firmware has set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Vco {
    /// The source is not the crystal, which is all the model works out: the PLL is taken to lock
    /// when it is switched on, and has no clock.
    NotModelled,
    /// A reference or a VCO outside what the chip accepts, or DIVM3 of 0: the PLL does not lock.
    OutOfRange,
    /// The VCO in Hz is `hz / per`, kept as a fraction so that nothing is rounded before the end.
    Hz { hz: u128, per: u128 },
}

// What the PLLs take, from RM0433 8.5.5 (PLL block diagram and text, pp. 346 to 348) and the
// descriptions of RCC_PLLCFGR (8.7.12, p. 401), RCC_PLL3DIVR (8.7.17, p. 410) and
// RCC_PLL3FRACR (8.7.18, p. 411). The reference (the input divided by DIVM3) must be 1 to
// 16 MHz. The wide VCO (VCOH, PLL3VCOSEL clear) takes a reference of 2 to 16 MHz and makes 192 to
// 960 MHz; the medium one (VCOL, set) takes 1 to 2 MHz and makes 150 to 420 MHz (the
// manual has VCOL chosen for a reference of 2 MHz or less, and VCOH otherwise). PLL3RGE is only
// to be set to the reference's range for optimal performance, so it is not a condition here. DIVN3 is 4 to 512 (the field, 3 to
// 0x1FF); no VCO range can be reached with less, so the range of the VCO covers the lower limit. The manual does not say what a PLL does outside
// these ranges (it does not guarantee the output frequency): the model does not lock it.
const REFERENCE_HZ: std::ops::RangeInclusive<u128> = 1_000_000..=16_000_000;
const WIDE_REFERENCE_HZ: std::ops::RangeInclusive<u128> = 2_000_000..=16_000_000;
const MEDIUM_REFERENCE_HZ: std::ops::RangeInclusive<u128> = 1_000_000..=2_000_000;
const WIDE_VCO_HZ: std::ops::RangeInclusive<u128> = 192_000_000..=960_000_000;
const MEDIUM_VCO_HZ: std::ops::RangeInclusive<u128> = 150_000_000..=420_000_000;

impl RccRegisters {
    fn pll3_vco(&mut self) -> Vco {
        let (cksel, cfgr) = (self.stored.read(PLLCKSELR), self.stored.read(PLLCFGR));
        if cksel & 3 != PLLSRC_HSE {
            return Vco::NotModelled;
        }
        let divm = u128::from(cksel >> 20 & 0x3F);
        // DIVM3 of 0 switches the PLL off. The reference is HSE / DIVM3, so its range is a range
        // on HSE, compared with the range times DIVM3 instead of dividing.
        let hse = u128::from(HSE_HZ);
        let medium = cfgr & PLL3VCOSEL != 0;
        let reference = if medium {
            MEDIUM_REFERENCE_HZ
        } else {
            WIDE_REFERENCE_HZ
        };
        if divm == 0
            || hse < reference.start().max(REFERENCE_HZ.start()) * divm
            || hse > reference.end().min(REFERENCE_HZ.end()) * divm
        {
            return Vco::OutOfRange;
        }
        let divn = u128::from(self.stored.read(PLL3DIVR) & 0x1FF) + 1;
        let frac = u128::from(self.sh_reg);
        // HSE / DIVM3 x (DIVN3 + FRACN3 / 8192), with the 8192 and DIVM3 kept apart. The field
        // holds DIVN3 less 1. The fraction is the one latched in SH_REG.
        let (hz, per) = (u128::from(HSE_HZ) * (divn << 13 | frac), divm << 13);
        let range = if medium { MEDIUM_VCO_HZ } else { WIDE_VCO_HZ };
        if hz < range.start() * per || hz > range.end() * per {
            return Vco::OutOfRange;
        }
        Vco::Hz { hz, per }
    }

    /// The bits of the register at `offset` that a write cannot change now (RM0433: DIVM3, and
    /// PLLSRC while any PLL is on, p. 347 and 398; PLL3RGE, PLL3VCOSEL, DIVR3EN, DIVQ3EN and
    /// DIVP3EN with PLL3 on, p. 401; DIVN3 and the three dividers, p. 409 and 410). FRACN3 and
    /// PLL3FRACEN can be written at any time (p. 411).
    fn locked_while_pll3_on(&mut self, offset: u32) -> u32 {
        let cr = self.stored.read(CR);
        let pll3_on = cr & PLL3ON != 0;
        match offset {
            PLLCKSELR => {
                (if pll3_on { DIVM3_BITS } else { 0 })
                    | (if cr & ANY_PLL_ON != 0 { PLLSRC_BITS } else { 0 })
            }
            PLLCFGR if pll3_on => PLL3_CONFIG_BITS,
            PLL3DIVR if pll3_on => u32::MAX,
            _ => 0,
        }
    }

    /// PLL3's R output: the VCO divided by DIVR3.
    fn pll3_r_hz(&mut self) -> u64 {
        let (cr, cfgr) = (self.stored.read(CR), self.stored.read(PLLCFGR));
        let Vco::Hz { hz, per } = self.pll3_vco() else {
            return 0;
        };
        if cr & PLL3ON == 0 || cfgr & DIVR3EN == 0 {
            return 0;
        }
        let divr = u128::from(self.stored.read(PLL3DIVR) >> 24 & 0x7F) + 1;
        (hz / (per * divr)) as u64
    }

    fn update_pixel_clock(&mut self) {
        let hz = self.pll3_r_hz();
        self.pixel_clock.set(hz);
    }
}

impl Registers for RccRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        let value = self.stored.read(offset);
        match offset {
            CR => {
                let ready = READY_FOLLOWS_ON
                    .iter()
                    .fold(value | HSIDIVF | D1CKRDY | D2CKRDY, |v, &(on, ready)| {
                        follow(v, on, ready)
                    });
                // A PLL set to what it cannot lock on does not become ready.
                if self.pll3_vco() == Vco::OutOfRange {
                    ready & !PLL3RDY
                } else {
                    ready
                }
            }
            CFGR => value & !SWS | (value & SW) << 3,
            BDCR => follow(value, LSEON, LSERDY),
            CSR => follow(value, LSION, LSIRDY),
            RSR => self.rsr,
            _ => value,
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        if offset == RSR {
            if value & mask & RMVF != 0 {
                self.rsr &= !FLAGS;
            }
        } else {
            let was_latching = self.stored.read(PLLCFGR) & PLL3FRACEN != 0;
            let locked = self.locked_while_pll3_on(offset);
            self.stored.write(offset, value, mask & !locked);
            if self.stored.read(PLLCFGR) & PLL3FRACEN != 0 && !was_latching {
                self.sh_reg = self.stored.read(PLL3FRACR) >> 3 & 0x1FFF;
            }
            if PLL3_INPUTS.contains(&offset) {
                self.update_pixel_clock();
            }
        }
    }

    fn reset(&mut self, cause: ResetCause) {
        let bdcr = self.stored.read(BDCR);
        self.stored.reset(cause);
        self.stored.write(BDCR, bdcr, u32::MAX);
        self.sh_reg = 0;
        // A software reset sets the pin flag too, as on the chip.
        self.rsr = match cause {
            ResetCause::PowerOn => POWER_ON_FLAGS,
            ResetCause::Software => SFTRSTF | PINRSTF,
        };
        self.update_pixel_clock();
    }
}

pub type Rcc = WordAccess<RccRegisters>;

impl Rcc {
    pub fn new(clock: Clock) -> Rcc {
        WordAccess::wrap(RccRegisters {
            stored: StubRegisters::with_reset_values(RCC_BASE, Svd::stm32h743()),
            sh_reg: 0,
            rsr: POWER_ON_FLAGS,
            pixel_clock: PixelClock::new(clock),
        })
    }

    /// The pixel clock, for the LTDC to run its frame by.
    pub(crate) fn pixel_clock(&self) -> PixelClock {
        self.inner().pixel_clock.clone()
    }
}

#[cfg(test)]
mod tests;
