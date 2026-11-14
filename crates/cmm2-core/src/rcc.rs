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

// What the PLLs take, from my memory of RM0433 and not yet checked against it. The reference (the
// input divided by DIVM) must be in the range PLL3RGE selects: 1 to 2 MHz, 2 to 4, 4 to 8 or
// 8 to 16. The VCO is of 192 to 836 MHz in the wide range (PLL3VCOSEL clear), which needs a
// reference of 2 MHz or more, or of 150 to 420 MHz in the medium range, which is the one for a
// reference of 1 to 2 MHz. A DIVN3 below the 3 the chip takes gives a VCO of at most 48 MHz, so
// the range of the VCO covers it.
const REFERENCE_HZ: [std::ops::RangeInclusive<u128>; 4] = [
    1_000_000..=2_000_000,
    2_000_000..=4_000_000,
    4_000_000..=8_000_000,
    8_000_000..=16_000_000,
];
const WIDE_VCO_HZ: std::ops::RangeInclusive<u128> = 192_000_000..=836_000_000;
const MEDIUM_VCO_HZ: std::ops::RangeInclusive<u128> = 150_000_000..=420_000_000;
/// The least reference the wide VCO range takes.
const WIDE_VCO_REFERENCE_HZ: u128 = 2_000_000;

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
        let reference = &REFERENCE_HZ[(cfgr >> 10 & 3) as usize];
        let least = if medium {
            *reference.start()
        } else {
            *reference.start().max(&WIDE_VCO_REFERENCE_HZ)
        };
        if divm == 0 || hse < least * divm || hse > reference.end() * divm {
            return Vco::OutOfRange;
        }
        let (divr, fracr) = (self.stored.read(PLL3DIVR), self.stored.read(PLL3FRACR));
        let divn = u128::from(divr & 0x1FF) + 1;
        let frac = if cfgr & PLL3FRACEN != 0 {
            u128::from(fracr >> 3 & 0x1FFF)
        } else {
            0
        };
        // HSE / DIVM3 x (DIVN3 + 1 + FRACN3 / 8192), with the 8192 and DIVM3 kept apart.
        let (hz, per) = (u128::from(HSE_HZ) * (divn << 13 | frac), divm << 13);
        let range = if medium { MEDIUM_VCO_HZ } else { WIDE_VCO_HZ };
        if hz < range.start() * per || hz > range.end() * per {
            return Vco::OutOfRange;
        }
        Vco::Hz { hz, per }
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
            self.stored.write(offset, value, mask);
            if PLL3_INPUTS.contains(&offset) {
                self.update_pixel_clock();
            }
        }
    }

    fn reset(&mut self, cause: ResetCause) {
        let bdcr = self.stored.read(BDCR);
        self.stored.reset(cause);
        self.stored.write(BDCR, bdcr, u32::MAX);
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
mod tests {
    use super::*;
    use crate::{MemoryMap, ResetCause};

    const CR: u32 = RCC_BASE;
    const CFGR: u32 = RCC_BASE + 0x10;
    const BDCR: u32 = RCC_BASE + 0x70;
    const CSR: u32 = RCC_BASE + 0x74;
    const PLL1DIVR: u32 = RCC_BASE + 0x30;
    const AHB4ENR: u32 = RCC_BASE + 0xE0;

    // (name, on bit, ready bit) of the RCC_CR pairs that follow.
    const PAIRS: [(&str, u32, u32); 7] = [
        ("HSI", 1 << 0, 1 << 2),
        ("CSI", 1 << 7, 1 << 8),
        ("RC48", 1 << 12, 1 << 13),
        ("HSE", 1 << 16, 1 << 17),
        ("PLL1", 1 << 24, 1 << 25),
        ("PLL2", 1 << 26, 1 << 27),
        ("PLL3", 1 << 28, 1 << 29),
    ];
    // Always set: D1CKRDY, D2CKRDY and HSIDIVF.
    const ALWAYS_READY: u32 = 1 << 14 | 1 << 15 | 1 << 5;
    const ON_BITS: u32 = 0x1501_1001;
    const BOARD_CR: u32 = 0x3F03_F025;

    fn machine() -> MemoryMap {
        let mut m = MemoryMap::new();
        m.map_peripheral(RCC_BASE, RCC_SIZE, Box::new(Rcc::new(Clock::new())))
            .unwrap();
        m
    }

    #[test]
    fn each_ready_bit_follows_its_on_bit_both_ways() {
        for (name, on, ready) in PAIRS {
            let mut m = machine();
            m.write32(CR, 0);
            m.write32(CR, on);
            assert_eq!(m.read32(CR) & ready, ready, "{name} set");
            m.write32(CR, 0);
            assert_eq!(m.read32(CR) & ready, 0, "{name} cleared");
        }
    }

    #[test]
    fn domain_clocks_and_hsidivf_always_read_set() {
        let mut m = machine();
        m.write32(CR, 0);
        assert_eq!(m.read32(CR), ALWAYS_READY);
    }

    #[test]
    fn writes_to_ready_bits_do_nothing() {
        let mut m = machine();
        let on: u32 = PAIRS.iter().map(|p| p.1).sum();
        let ready: u32 = PAIRS.iter().map(|p| p.2).sum();
        m.write32(CR, !on);
        assert_eq!(m.read32(CR) & ready, 0);
    }

    // The reset value 0x83 has HSION, HSIKERON and CSION set.
    #[test]
    fn unwritten_cr_reads_the_reset_value_with_its_ready_bits() {
        let mut m = machine();
        assert_eq!(m.read32(CR), 0x0000_C1A7);
    }

    #[test]
    fn the_board_value_of_cr_comes_from_its_on_bits() {
        let mut m = machine();
        m.write32(CR, ON_BITS);
        assert_eq!(m.read32(CR), BOARD_CR);
    }

    #[test]
    fn cfgr_sws_follows_sw() {
        let mut m = machine();
        for sw in 0..=3 {
            m.write32(CFGR, sw);
            assert_eq!(m.read32(CFGR), sw | sw << 3, "SW {sw}");
        }
    }

    #[test]
    fn cfgr_stores_the_other_fields() {
        let mut m = machine();
        m.write32(CFGR, 0xFFFF_FFC3);
        assert_eq!(m.read32(CFGR), 0xFFFF_FFDB);
    }

    #[test]
    fn cfgr_byte_write_to_the_low_byte() {
        let mut m = machine();
        m.write32(CFGR, 0x0000_0100);
        m.write8(CFGR, 3);
        assert_eq!(m.read32(CFGR), 0x0000_011B);
    }

    #[test]
    fn bdcr_lserdy_follows_lseon_and_the_rest_is_stored() {
        let mut m = machine();
        m.write32(BDCR, 0x0000_8111);
        assert_eq!(m.read32(BDCR), 0x0000_8113);
        m.write32(BDCR, 0x0000_8110);
        assert_eq!(m.read32(BDCR), 0x0000_8110);
        m.write32(BDCR, 0x2);
        assert_eq!(m.read32(BDCR), 0);
    }

    #[test]
    fn csr_lsirdy_follows_lsion() {
        let mut m = machine();
        m.write32(CSR, 1);
        assert_eq!(m.read32(CSR), 3);
        m.write32(CSR, 0);
        assert_eq!(m.read32(CSR), 0);
        m.write32(CSR, 2);
        assert_eq!(m.read32(CSR), 0);
    }

    #[test]
    fn read_modify_write_of_cr_brings_pll1_up_and_down() {
        let (hse_on, hse_rdy) = (1 << 16, 1 << 17);
        let (pll1_on, pll1_rdy) = (1 << 24, 1 << 25);
        let mut m = machine();
        let cr = m.read32(CR);
        m.write32(CR, cr | hse_on);
        assert_eq!(m.read32(CR) & hse_rdy, hse_rdy);
        let cr = m.read32(CR);
        m.write32(CR, cr | pll1_on);
        let cr = m.read32(CR);
        assert_eq!(cr & pll1_rdy, pll1_rdy);
        assert_eq!(cr & hse_rdy, hse_rdy);
        m.write32(CR, cr & !pll1_on);
        let cr = m.read32(CR);
        assert_eq!(cr & pll1_rdy, 0);
        assert_eq!(cr & hse_rdy, hse_rdy);
    }

    #[test]
    fn ordinary_registers_store_writes_and_start_at_their_reset_values() {
        let mut m = machine();
        assert_eq!(m.read32(PLL1DIVR), 0x0101_0280);
        m.write32(PLL1DIVR, 0x0102_03FF);
        assert_eq!(m.read32(PLL1DIVR), 0x0102_03FF);
        m.write16(AHB4ENR, 0x00FF);
        assert_eq!(m.read32(AHB4ENR), 0x00FF);
    }

    #[test]
    fn window_ends_at_rcc_size() {
        let mut m = machine();
        m.write32(RCC_BASE + RCC_SIZE - 4, 5);
        assert_eq!(m.read32(RCC_BASE + RCC_SIZE - 4), 5);
        assert!(m.take_unmapped().is_empty());
    }

    const RSR: u32 = RCC_BASE + 0xD0;
    const POWER_ON_FLAGS: u32 = 0x00E0_0000;
    const SOFTWARE_FLAGS: u32 = 0x0140_0000;

    #[test]
    fn reset_puts_registers_back_but_keeps_bdcr() {
        let mut m = machine();
        m.write32(CR, ON_BITS);
        m.write32(PLL1DIVR, 0x0102_03FF);
        m.write32(BDCR, 0x0000_8111);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(CR), 0x0000_C1A7);
        assert_eq!(m.read32(PLL1DIVR), 0x0101_0280);
        assert_eq!(m.read32(BDCR), 0x0000_8113);
    }

    #[test]
    fn rsr_reads_the_power_on_flags_at_construction() {
        let mut m = machine();
        assert_eq!(m.read32(RSR), POWER_ON_FLAGS);
    }

    #[test]
    fn rsr_says_software_reset_after_one() {
        let mut m = machine();
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(RSR), SOFTWARE_FLAGS);
        m.reset_peripherals(ResetCause::PowerOn);
        assert_eq!(m.read32(RSR), POWER_ON_FLAGS);
    }

    #[test]
    fn rsr_flags_clear_with_rmvf_and_nothing_else_writes_them() {
        let mut m = machine();
        m.write32(RSR, !RMVF);
        assert_eq!(m.read32(RSR), POWER_ON_FLAGS);
        m.write32(RSR, 1 << 16);
        assert_eq!(m.read32(RSR), 0);
        m.reset_peripherals(ResetCause::Software);
        m.write32(RSR, 1 << 16 | 1 << 24);
        assert_eq!(m.read32(RSR), 0);
    }

    // The registers the firmware writes to bring up the board's pixel clock: the crystal as
    // the source and DIVM3 of 1, PLL3 on with its R output enabled and the VCO in the wide
    // range, and the dividers of the video mode.
    fn program_pll3(m: &mut MemoryMap, divr: u32) {
        m.write32(RCC_BASE + 0x28, 0x0010_1012); // PLLCKSELR
        m.write32(RCC_BASE + 0x40, divr); // PLL3DIVR
        m.write32(RCC_BASE + 0x2C, 0x013F_0CDD); // PLLCFGR
        m.write32(CR, ON_BITS);
    }

    fn machine_and_pixel_clock() -> (MemoryMap, PixelClock) {
        let rcc = Rcc::new(Clock::new());
        let pixel_clock = rcc.pixel_clock();
        let mut m = MemoryMap::new();
        m.map_peripheral(RCC_BASE, RCC_SIZE, Box::new(rcc)).unwrap();
        (m, pixel_clock)
    }

    #[test]
    fn there_is_no_pixel_clock_until_pll3_is_set_up() {
        let (_, pixel_clock) = machine_and_pixel_clock();
        assert_eq!(pixel_clock.hz(), 0);
    }

    #[test]
    fn the_pixel_clock_of_800_x_600_is_49_5_mhz() {
        // 8 MHz x 99 = 792 MHz, divided by 16.
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        program_pll3(&mut m, 0x0F0F_1E62);
        assert_eq!(pixel_clock.hz(), 49_500_000);
    }

    #[test]
    fn the_pixel_clock_of_640_x_480_is_31_4_mhz() {
        // 8 MHz x 102 = 816 MHz, divided by 26.
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        program_pll3(&mut m, 0x1919_3265);
        assert_eq!(pixel_clock.hz(), 31_384_615);
    }

    #[test]
    fn the_pixel_clock_follows_each_register_it_depends_on() {
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        program_pll3(&mut m, 0x0F0F_1E62);
        // DIVM3 of 2 halves it, with the reference range for 4 MHz (PLL3RGE of 2).
        m.write32(RCC_BASE + 0x2C, 0x013F_08DD);
        m.write32(RCC_BASE + 0x28, 0x0020_1012);
        assert_eq!(pixel_clock.hz(), 24_750_000);
        m.write32(RCC_BASE + 0x28, 0x0010_1012);
        m.write32(RCC_BASE + 0x2C, 0x013F_0CDD);
        assert_eq!(pixel_clock.hz(), 49_500_000);
        // PLL3 off stops it, on starts it again.
        m.write32(CR, ON_BITS & !(1 << 28));
        assert_eq!(pixel_clock.hz(), 0);
        m.write32(CR, ON_BITS);
        assert_eq!(pixel_clock.hz(), 49_500_000);
        // So does the R output being disabled.
        m.write32(RCC_BASE + 0x2C, 0x003F_0CDD);
        assert_eq!(pixel_clock.hz(), 0);
    }

    #[test]
    fn the_fraction_adds_to_the_multiplier_only_when_it_is_on() {
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        program_pll3(&mut m, 0x0F0F_1E62);
        // FRACN3 of 4096 is a half: 99.5 x 8 MHz = 796 MHz.
        m.write32(RCC_BASE + 0x44, 4096 << 3);
        assert_eq!(pixel_clock.hz(), 49_500_000);
        m.write32(RCC_BASE + 0x2C, 0x013F_0DDD);
        assert_eq!(pixel_clock.hz(), 49_750_000);
    }

    #[test]
    fn a_pll_that_is_not_fed_from_the_crystal_gives_no_pixel_clock() {
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        program_pll3(&mut m, 0x0F0F_1E62);
        for source in [0, 1, 3] {
            m.write32(RCC_BASE + 0x28, 0x0010_1010 | source);
            assert_eq!(pixel_clock.hz(), 0, "source {source}");
        }
        // And a DIVM3 of 0 switches the PLL off.
        m.write32(RCC_BASE + 0x28, 0x0000_1012);
        assert_eq!(pixel_clock.hz(), 0);
    }

    // PLL3 with the crystal as its source, DIVM3 of `divm`, a VCO of `divn` x the reference (the
    // register holds DIVN3 = divn - 1), and R undivided. `rge` is PLL3RGE, bits 11:10 of PLLCFGR
    // (3 is the firmware's: 8 to 16 MHz), and `medium_vco` is bit 9 (PLL3VCOSEL).
    fn pll3_locks_with(divm: u32, divn: u32, rge: u32, medium_vco: bool) -> (bool, u64) {
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        m.write32(RCC_BASE + 0x28, divm << 20 | 0x1012);
        m.write32(RCC_BASE + 0x40, divn - 1);
        m.write32(
            RCC_BASE + 0x2C,
            0x013F_00DD | rge << 10 | u32::from(medium_vco) << 9,
        );
        m.write32(CR, ON_BITS);
        let ready = m.read32(CR) & 1 << 29 != 0;
        (ready, pixel_clock.hz())
    }

    #[test]
    fn the_reference_is_divided_only_after_the_vco_is_worked_out() {
        // 8 MHz / 3 x 300 is 800 MHz, whole; dividing 8 MHz by 3 first loses 200 Hz of it.
        assert_eq!(pll3_locks_with(3, 300, 1, false), (true, 800_000_000));
    }

    #[test]
    fn the_firmwares_own_pll3_settings_lock() {
        // DIVM3 of 1, PLL3RGE of 3 and the wide VCO, with the DIVN3 of every mode of docs/video.md.
        for divn in [99, 102, 104, 103, 89, 101, 81, 93] {
            assert!(pll3_locks_with(1, divn, 3, false).0, "{divn}");
        }
    }

    #[test]
    fn pll3_only_locks_for_a_reference_and_a_vco_in_the_ranges_of_the_chip() {
        // From memory of RM0433, to check against it: PLL3RGE picks the reference range (1 to 2,
        // 2 to 4, 4 to 8, 8 to 16 MHz), the wide VCO (192 to 836 MHz) needs a reference of at
        // least 2 MHz and the medium one (150 to 420 MHz) takes 1 MHz. 8 MHz / DIVM3 x DIVN3 is
        // the VCO.
        for (divm, divn, rge, medium, locks) in [
            (1, 24, 3, false, true),   // 192 MHz
            (1, 23, 3, false, false),  // 184
            (1, 104, 3, false, true),  // 832
            (1, 105, 3, false, false), // 840
            (1, 19, 3, true, true),    // 152 in the medium range
            (1, 18, 3, true, false),   // 144
            (1, 52, 3, true, true),    // 416
            (1, 53, 3, true, false),   // 424
            (1, 80, 3, true, false),   // 640 is for the wide range only
            // The reference against PLL3RGE: 8 MHz, 4, 2, 1 and 0.89 MHz.
            (1, 50, 2, false, true), // 8 MHz is the end of 4 to 8, which counts
            (2, 100, 3, false, false), // 4 MHz is not in 8 to 16
            (2, 100, 2, false, true), // but is in 4 to 8
            (2, 100, 1, false, true), // and in 2 to 4
            (2, 100, 0, false, false), // not in 1 to 2
            (4, 200, 0, false, true), // 2 MHz is in 1 to 2, and the wide VCO takes it
            (8, 200, 0, false, false), // 1 MHz is in 1 to 2 but the wide VCO needs 2
            (8, 200, 0, true, true), // the medium VCO takes it: 200 MHz
            (9, 216, 0, true, false), // 889 kHz is below the least
            (0, 100, 3, false, false), // DIVM3 of 0 is the PLL off
            (1, 1, 3, false, false), // DIVN3 of 0, which is below the least the chip takes
            (1, 512, 3, false, false), // 4096 MHz
        ] {
            let (ready, hz) = pll3_locks_with(divm, divn, rge, medium);
            assert_eq!(ready, locks, "{divm} {divn} {rge} {medium}");
            assert_eq!(hz != 0, locks, "{divm} {divn} {rge} {medium}: {hz}");
        }
    }

    #[test]
    fn every_change_of_the_pixel_clock_is_logged_with_its_cycle_and_asks_for_a_tick() {
        let clock = Clock::new();
        let pixel_clock = PixelClock::new(clock.clone());
        clock.set(500);
        pixel_clock.set(1000);
        assert_eq!(pixel_clock.hz(), 1000);
        assert!(clock.take_due());
        // No change, nothing logged; a stop and a restart at the same rate are two changes.
        clock.set(900);
        pixel_clock.set(1000);
        assert!(!clock.take_due());
        pixel_clock.set(0);
        clock.set(950);
        pixel_clock.set(1000);
        assert_eq!(
            pixel_clock.take_changes(),
            [(500, 1000), (900, 0), (950, 1000)]
        );
        assert!(pixel_clock.take_changes().is_empty());
    }

    #[test]
    fn a_reset_stops_the_pixel_clock() {
        let (mut m, pixel_clock) = machine_and_pixel_clock();
        program_pll3(&mut m, 0x0F0F_1E62);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(pixel_clock.hz(), 0);
    }
}
