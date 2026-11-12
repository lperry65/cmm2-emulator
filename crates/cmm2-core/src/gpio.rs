//! The GPIO ports. Each port has its own reset values, and the input data register is worked
//! out from the pin configuration and the levels the real board has on its pins.

use crate::peripheral::ResetCause;
use crate::registers::{Registers, WordAccess};

pub const GPIO_BASE: u32 = 0x5802_0000;
pub const GPIO_PORT_SIZE: u32 = 0x400;
/// GPIOA to GPIOK.
pub const GPIO_PORTS: usize = 11;

const MODER: u32 = 0x00;
const OTYPER: u32 = 0x04;
const OSPEEDR: u32 = 0x08;
const PUPDR: u32 = 0x0C;
const IDR: u32 = 0x10;
const ODR: u32 = 0x14;
const BSRR: u32 = 0x18;
const LCKR: u32 = 0x1C;
const AFRL: u32 = 0x20;
const AFRH: u32 = 0x24;

// Reset values per port (MODER, OSPEEDR, PUPDR), A to K. They follow RM0433. The SVD gives
// GPIOA's values to every port, which is wrong for B to K.
const RESETS: [[u32; 3]; GPIO_PORTS] = [
    [0xABFF_FFFF, 0x0C00_0000, 0x6400_0000], // A
    [0xFFFF_FEBF, 0x0000_00C0, 0x0000_0100], // B
    [0xFFFF_FFFF, 0, 0],                     // C
    [0xFFFF_FFFF, 0, 0],                     // D
    [0xFFFF_FFFF, 0, 0],                     // E
    [0xFFFF_FFFF, 0, 0],                     // F
    [0xFFFF_FFFF, 0, 0],                     // G
    [0xFFFF_FFFF, 0, 0],                     // H
    [0xFFFF_FFFF, 0, 0],                     // I
    [0xFFFF_FFFF, 0, 0],                     // J
    [0xFFFF_FFFF, 0, 0],                     // K
];

// Pins the board itself holds low, A to K. They read low even with a pull-up on. The real
// board read D3, F9 and H12 low with their pull-ups enabled (2026-11-12, at the idle prompt,
// with a USB keyboard attached), so something outside the chip drives them.
const HELD_LOW: [u16; GPIO_PORTS] = [0, 0, 0, 1 << 3, 0, 1 << 9, 0, 1 << 12, 0, 0, 0];

// Pins in alternate-function mode that read high, A to K: the levels the real board read at
// the idle prompt (same occasion, same keyboard). They stand in for the peripherals that
// drive these pins, which are not modelled yet.
const AF_HIGH: [u16; GPIO_PORTS] = [
    0x2600, 0, 0, 0x8000, 0xAA80, 0x0C00, 0x8131, 0x00E0, 0, 0, 0,
];

#[derive(Debug)]
pub struct GpioRegisters {
    port: usize,
    moder: u32,
    otyper: u32,
    ospeedr: u32,
    pupdr: u32,
    odr: u32,
    lckr: u32,
    afrl: u32,
    afrh: u32,
    held_low: u16,
    af_high: u16,
}

impl GpioRegisters {
    fn new(port: usize) -> GpioRegisters {
        let [moder, ospeedr, pupdr] = RESETS[port];
        GpioRegisters {
            port,
            moder,
            otyper: 0,
            ospeedr,
            pupdr,
            odr: 0,
            lckr: 0,
            afrl: 0,
            afrh: 0,
            held_low: HELD_LOW[port],
            af_high: AF_HIGH[port],
        }
    }

    // The level of pin `pin` when nothing on the chip drives it. A pin with no pull has no
    // defined level; it reads low here.
    fn input_level(&self, pin: u32) -> bool {
        self.held_low >> pin & 1 == 0 && self.pupdr >> (2 * pin) & 3 == 0b01
    }

    fn pin_level(&self, pin: u32) -> bool {
        let driven = self.odr >> pin & 1 != 0;
        match self.moder >> (2 * pin) & 3 {
            0b00 => self.input_level(pin),
            0b01 if self.otyper >> pin & 1 == 0 => driven,
            0b01 => driven && self.input_level(pin),
            0b10 => self.af_high >> pin & 1 != 0,
            _ => false,
        }
    }

    fn idr(&self) -> u32 {
        (0..16).fold(0, |idr, pin| idr | u32::from(self.pin_level(pin)) << pin)
    }
}

fn store(register: &mut u32, value: u32, mask: u32) {
    *register = *register & !mask | value & mask;
}

impl Registers for GpioRegisters {
    fn read(&mut self, offset: u32) -> u32 {
        match offset {
            MODER => self.moder,
            OTYPER => self.otyper,
            OSPEEDR => self.ospeedr,
            PUPDR => self.pupdr,
            IDR => self.idr(),
            ODR => self.odr,
            LCKR => self.lckr,
            AFRL => self.afrl,
            AFRH => self.afrh,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u32, value: u32, mask: u32) {
        match offset {
            MODER => store(&mut self.moder, value, mask),
            OTYPER => store(&mut self.otyper, value, mask),
            OSPEEDR => store(&mut self.ospeedr, value, mask),
            PUPDR => store(&mut self.pupdr, value, mask),
            ODR => {
                store(&mut self.odr, value, mask);
                self.odr &= 0xFFFF;
            }
            BSRR => {
                let value = value & mask;
                self.odr = (self.odr & !(value >> 16) | value) & 0xFFFF;
            }
            LCKR => store(&mut self.lckr, value, mask),
            AFRL => store(&mut self.afrl, value, mask),
            AFRH => store(&mut self.afrh, value, mask),
            _ => {}
        }
    }

    // A system reset puts the port back to its reset values.
    fn reset(&mut self, _cause: ResetCause) {
        *self = GpioRegisters::new(self.port);
    }
}

pub type Gpio = WordAccess<GpioRegisters>;

impl Gpio {
    /// `port` is 0 for GPIOA.
    pub fn new(port: usize) -> Gpio {
        WordAccess::wrap(GpioRegisters::new(port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svd::Svd;
    use crate::{MemoryMap, ResetCause};

    const GPIOA: u32 = GPIO_BASE;
    const GPIOB: u32 = GPIO_BASE + GPIO_PORT_SIZE;
    const GPIOC: u32 = GPIO_BASE + 2 * GPIO_PORT_SIZE;
    const GPIOD: u32 = GPIO_BASE + 3 * GPIO_PORT_SIZE;
    const GPIOF: u32 = GPIO_BASE + 5 * GPIO_PORT_SIZE;
    const GPIOG: u32 = GPIO_BASE + 6 * GPIO_PORT_SIZE;
    const GPIOH: u32 = GPIO_BASE + 7 * GPIO_PORT_SIZE;
    const GPIOK: u32 = GPIO_BASE + 10 * GPIO_PORT_SIZE;

    fn machine() -> MemoryMap {
        let mut m = MemoryMap::new();
        for i in 0..GPIO_PORTS {
            m.map_peripheral(
                GPIO_BASE + i as u32 * GPIO_PORT_SIZE,
                GPIO_PORT_SIZE,
                Box::new(Gpio::new(i)),
            )
            .unwrap();
        }
        m
    }

    // Sets pin `pin` of the port at `base` to `mode` (0 input, 1 output, 2 AF, 3 analog)
    // and `pull` (0 none, 1 up, 2 down), leaving the other pins alone.
    fn configure(m: &mut MemoryMap, base: u32, pin: u32, mode: u32, pull: u32) {
        let mut field = |reg: u32, value: u32| {
            let old = m.read32(base + reg);
            m.write32(base + reg, old & !(3 << (2 * pin)) | value << (2 * pin));
        };
        field(MODER, mode);
        field(PUPDR, pull);
    }

    fn pin(m: &mut MemoryMap, base: u32, pin: u32) -> u32 {
        m.read32(base + IDR) >> pin & 1
    }

    #[test]
    fn reset_values_follow_the_reference_manual() {
        let mut m = machine();
        assert_eq!(m.read32(GPIOA + MODER), 0xABFF_FFFF);
        assert_eq!(m.read32(GPIOB + MODER), 0xFFFF_FEBF);
        assert_eq!(m.read32(GPIOC + MODER), 0xFFFF_FFFF);
        assert_eq!(m.read32(GPIOK + MODER), 0xFFFF_FFFF);
        assert_eq!(m.read32(GPIOA + PUPDR), 0x6400_0000);
        assert_eq!(m.read32(GPIOB + PUPDR), 0x0000_0100);
        assert_eq!(m.read32(GPIOC + PUPDR), 0);
        assert_eq!(m.read32(GPIOA + OSPEEDR), 0x0C00_0000);
        assert_eq!(m.read32(GPIOB + OSPEEDR), 0xC0);
        assert_eq!(m.read32(GPIOK + OSPEEDR), 0);
        for i in 0..GPIO_PORTS as u32 {
            let base = GPIO_BASE + i * GPIO_PORT_SIZE;
            for reg in [OTYPER, ODR, BSRR, LCKR, AFRL, AFRH] {
                assert_eq!(m.read32(base + reg), 0, "port {i} register {reg:#x}");
            }
        }
    }

    // The SVD gives every port the reset values of GPIOA.
    #[test]
    fn reset_values_differ_from_the_svd_for_the_other_ports() {
        let svd = Svd::stm32h743();
        let svd_moder = svd.lookup(GPIOB + MODER).unwrap().reset_value;
        assert_eq!(svd_moder, 0xABFF_FFFF);
        assert_ne!(machine().read32(GPIOB + MODER), svd_moder);
    }

    #[test]
    fn pb13_as_the_firmware_sets_it_up_reads_high() {
        let mut m = machine();
        m.write32(GPIOB + PUPDR, 0x0410_0000);
        m.write32(GPIOB + MODER, 0xA0BA_BABE);
        assert_eq!(pin(&mut m, GPIOB, 13), 1);
    }

    #[test]
    fn pb13_reads_low_as_analog_or_without_a_pull_up() {
        let mut m = machine();
        configure(&mut m, GPIOB, 13, 3, 1);
        assert_eq!(pin(&mut m, GPIOB, 13), 0);
        configure(&mut m, GPIOB, 13, 0, 0);
        assert_eq!(pin(&mut m, GPIOB, 13), 0);
        configure(&mut m, GPIOB, 13, 0, 2);
        assert_eq!(pin(&mut m, GPIOB, 13), 0);
    }

    #[test]
    fn pins_the_board_holds_low_stay_low_with_a_pull_up() {
        let mut m = machine();
        for (base, p) in [(GPIOF, 9), (GPIOD, 3), (GPIOH, 12)] {
            configure(&mut m, base, p, 0, 1);
            assert_eq!(pin(&mut m, base, p), 0, "pull-up {base:#x} {p}");
            configure(&mut m, base, p, 0, 0);
            assert_eq!(pin(&mut m, base, p), 0, "no pull {base:#x} {p}");
        }
        configure(&mut m, GPIOC, 11, 0, 1);
        assert_eq!(pin(&mut m, GPIOC, 11), 1);
        configure(&mut m, GPIOG, 3, 0, 2);
        assert_eq!(pin(&mut m, GPIOG, 3), 0);
    }

    #[test]
    fn push_pull_outputs_read_their_odr_bit() {
        let mut m = machine();
        configure(&mut m, GPIOC, 5, 1, 0);
        assert_eq!(pin(&mut m, GPIOC, 5), 0);
        m.write32(GPIOC + ODR, 1 << 5);
        assert_eq!(pin(&mut m, GPIOC, 5), 1);
        assert_eq!(m.read32(GPIOC + ODR), 1 << 5);
    }

    #[test]
    fn odr_keeps_only_the_low_sixteen_bits() {
        let mut m = machine();
        m.write32(GPIOC + ODR, 0xFFFF_1234);
        assert_eq!(m.read32(GPIOC + ODR), 0x1234);
    }

    #[test]
    fn bsrr_sets_and_resets_odr_bits() {
        let mut m = machine();
        configure(&mut m, GPIOC, 5, 1, 0);
        m.write32(GPIOC + ODR, 1 << 2);
        m.write32(GPIOC + BSRR, 1 << 5);
        assert_eq!(m.read32(GPIOC + ODR), 1 << 5 | 1 << 2);
        assert_eq!(pin(&mut m, GPIOC, 5), 1);
        m.write32(GPIOC + BSRR, 1 << (16 + 5));
        assert_eq!(m.read32(GPIOC + ODR), 1 << 2);
        assert_eq!(pin(&mut m, GPIOC, 5), 0);
    }

    #[test]
    fn bsrr_set_wins_over_reset_of_the_same_pin() {
        let mut m = machine();
        m.write32(GPIOC + BSRR, 1 << 5 | 1 << 21);
        assert_eq!(m.read32(GPIOC + ODR), 1 << 5);
        m.write32(GPIOC + BSRR, 1 << 5 | 1 << 21);
        assert_eq!(m.read32(GPIOC + ODR), 1 << 5);
    }

    #[test]
    fn bsrr_reads_zero() {
        let mut m = machine();
        m.write32(GPIOC + BSRR, 0xFFFF);
        assert_eq!(m.read32(GPIOC + BSRR), 0);
    }

    #[test]
    fn idr_upper_half_reads_zero() {
        let mut m = machine();
        m.write32(GPIOC + MODER, 0x5555_5555);
        m.write32(GPIOC + ODR, 0xFFFF);
        assert_eq!(m.read32(GPIOC + IDR), 0xFFFF);
    }

    #[test]
    fn open_drain_outputs_read_low_or_the_released_level() {
        let mut m = machine();
        m.write32(GPIOC + OTYPER, 1 << 5 | 1 << 6);
        configure(&mut m, GPIOC, 5, 1, 1);
        configure(&mut m, GPIOC, 6, 1, 0);
        m.write32(GPIOC + ODR, 0);
        assert_eq!(pin(&mut m, GPIOC, 5), 0);
        assert_eq!(pin(&mut m, GPIOC, 6), 0);
        m.write32(GPIOC + ODR, 1 << 5 | 1 << 6);
        assert_eq!(pin(&mut m, GPIOC, 5), 1);
        assert_eq!(pin(&mut m, GPIOC, 6), 0);
    }

    #[test]
    fn alternate_function_pins_read_the_boards_level() {
        let mut m = machine();
        configure(&mut m, GPIOA, 10, 2, 0);
        configure(&mut m, GPIOA, 11, 2, 0);
        assert_eq!(pin(&mut m, GPIOA, 10), 1);
        assert_eq!(pin(&mut m, GPIOA, 11), 0);
        configure(&mut m, GPIOA, 10, 0, 0);
        assert_eq!(pin(&mut m, GPIOA, 10), 0);
    }

    #[test]
    fn plain_registers_store_writes() {
        let mut m = machine();
        for reg in [OTYPER, OSPEEDR, LCKR, AFRL, AFRH] {
            m.write32(GPIOC + reg, 0xA5A5_5A5A);
            assert_eq!(m.read32(GPIOC + reg), 0xA5A5_5A5A, "{reg:#x}");
            m.write8(GPIOC + reg + 1, 0x11);
            assert_eq!(m.read32(GPIOC + reg), 0xA5A5_115A, "{reg:#x}");
            m.write16(GPIOC + reg + 2, 0x2233);
            assert_eq!(m.read32(GPIOC + reg), 0x2233_115A, "{reg:#x}");
        }
    }

    #[test]
    fn idr_writes_are_ignored() {
        let mut m = machine();
        m.write32(GPIOC + MODER, 0);
        m.write32(GPIOC + IDR, 0xFFFF);
        assert_eq!(m.read32(GPIOC + IDR), 0);
    }

    // MODER, PUPDR and ODR as the real board had them at the prompt, and the IDR it read.
    #[test]
    fn the_boards_registers_give_the_boards_idr() {
        const BOARD: [[u32; 4]; 9] = [
            [0xEAAB_FFFF, 0x2414_0000, 0, 0x0000_2600],
            [0xFFFF_FFFB, 0, 0, 0],
            [0xFD1F_FFFE, 0x0040_0000, 0x0000_1000, 0x0000_1800],
            [0xAFEA_E51A, 0x0000_0540, 0x0000_0034, 0x0000_8034],
            [0xAAAA_ABFA, 0, 0, 0x0000_AA80],
            [0xAAA3_FAAA, 0x0004_0000, 0, 0x0000_0C00],
            [0xBEAE_EA2A, 0x0000_0080, 0, 0x0000_8131],
            [0xB8EA_ABAF, 0x0100_0000, 0, 0x0000_00E0],
            [0xFF6B_AAEA, 0, 0, 0],
        ];
        let mut m = machine();
        for (i, [moder, pupdr, odr, idr]) in BOARD.into_iter().enumerate() {
            let base = GPIO_BASE + i as u32 * GPIO_PORT_SIZE;
            m.write32(base + MODER, moder);
            m.write32(base + PUPDR, pupdr);
            m.write32(base + ODR, odr);
            assert_eq!(
                m.read32(base + IDR),
                idr,
                "port {}",
                (b'A' + i as u8) as char
            );
        }
    }

    #[test]
    fn reset_puts_the_registers_back_to_the_table() {
        let mut m = machine();
        m.write32(GPIOB + MODER, 0x1234_5678);
        m.write32(GPIOB + ODR, 0xFF);
        m.write32(GPIOB + AFRL, 0x1111_1111);
        m.write32(GPIOA + MODER, 0);
        m.reset_peripherals(ResetCause::Software);
        assert_eq!(m.read32(GPIOB + MODER), 0xFFFF_FEBF);
        assert_eq!(m.read32(GPIOB + ODR), 0);
        assert_eq!(m.read32(GPIOB + AFRL), 0);
        assert_eq!(m.read32(GPIOA + MODER), 0xABFF_FFFF);
        assert_eq!(m.read32(GPIOB + PUPDR), 0x0000_0100);
    }
}
