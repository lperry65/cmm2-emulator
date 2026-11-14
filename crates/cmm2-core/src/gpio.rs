//! The GPIO ports. Each port has its own reset values, and the input data register is worked
//! out from the pin configuration and the levels the real board has on its pins.

use crate::ds3231::Ds3231Slot;
use crate::peripheral::ResetCause;
use crate::registers::{Registers, WordAccess};
use crate::sd_slot::SdSlot;

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

// The ports and pins of the SD socket.
const PORT_C: usize = 2;
const PORT_D: usize = 3;
const PORT_H: usize = 7;
const SD_SCK: u32 = 10;
const SD_MISO: u32 = 11;
const SD_MOSI: u32 = 12;
const SD_CS: u32 = 2;
const SD_DETECT: u32 = 3;
const SD_PROTECT: u32 = 12;

// The DS3231's wires, on port D. The board pulls both up to 3V3 with 10K resistors.
const RTC_SDA: u32 = 4;
const RTC_SCL: u32 = 5;

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
// with a USB keyboard attached), so something outside the chip drives them. D3 is the SD
// socket's card-detect switch, so it comes from the SD slot instead and is not in this table.
// H12 is the write-protect switch, which comes from the SD slot as well: it is low (not
// protected) unless the slot says otherwise.
const HELD_LOW: [u16; GPIO_PORTS] = [0, 0, 0, 0, 0, 1 << 9, 0, 0, 0, 0, 0];

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
    sd: SdSlot,
    rtc: Ds3231Slot,
}

impl GpioRegisters {
    fn new(port: usize, sd: SdSlot, rtc: Ds3231Slot) -> GpioRegisters {
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
            sd,
            rtc,
        }
    }

    // The level of pin `pin` when nothing on the chip drives it. A pin with no pull has no
    // defined level; it reads low here.
    fn input_level(&self, pin: u32) -> bool {
        self.held_low >> pin & 1 == 0
            && !self.board_pulls_low(pin)
            && (self.pupdr >> (2 * pin) & 3 == 0b01 || self.board_pulls_up(pin))
    }

    // The board's own pull-ups, which hold a pin high whatever PUPDR says: the DS3231's wires.
    fn board_pulls_up(&self, pin: u32) -> bool {
        self.port == PORT_D && matches!(pin, RTC_SDA | RTC_SCL)
    }

    // The SD card holds its data-out wire low for a 0 bit, the card-detect switch closes when a
    // card is in, and the write-protect switch is open (the pin high, with its pull-up) only when
    // the card is protected. The DS3231 holds SDA low to acknowledge a byte and for a 0 bit it
    // sends; it never drives SCL.
    fn board_pulls_low(&self, pin: u32) -> bool {
        match (self.port, pin) {
            (PORT_C, SD_MISO) => !self.sd.miso(),
            (PORT_D, SD_DETECT) => self.sd.present(),
            (PORT_D, RTC_SDA) => self.rtc.pulls_sda_low(),
            (PORT_H, SD_PROTECT) => !self.sd.write_protected(),
            _ => false,
        }
    }

    // The level a chip on the board sees on one of its inputs: what this port drives, or
    // `released` when the pin is not an output.
    fn wire_input(&self, pin: u32, released: bool) -> bool {
        if self.moder >> (2 * pin) & 3 == 0b01 {
            self.odr >> pin & 1 != 0
        } else {
            released
        }
    }

    // Tells the chips on the board the levels this port now drives onto their wires. Chip
    // select is pulled up on the board, and the clock idles low in SPI mode 0; the I2C bus
    // idles high, both wires pulled up.
    fn drive_wires(&self) {
        match self.port {
            PORT_C => self.sd.set_clock(
                self.wire_input(SD_SCK, false),
                self.wire_input(SD_MOSI, true),
            ),
            PORT_D => {
                self.sd.set_chip_select(self.wire_input(SD_CS, true));
                self.rtc.set_lines(
                    self.wire_input(RTC_SDA, true),
                    self.wire_input(RTC_SCL, true),
                );
            }
            _ => {}
        }
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
            MODER => {
                store(&mut self.moder, value, mask);
                self.drive_wires();
            }
            OTYPER => store(&mut self.otyper, value, mask),
            OSPEEDR => store(&mut self.ospeedr, value, mask),
            PUPDR => store(&mut self.pupdr, value, mask),
            ODR => {
                store(&mut self.odr, value, mask);
                self.odr &= 0xFFFF;
                self.drive_wires();
            }
            BSRR => {
                let value = value & mask;
                self.odr = (self.odr & !(value >> 16) | value) & 0xFFFF;
                self.drive_wires();
            }
            LCKR => store(&mut self.lckr, value, mask),
            AFRL => store(&mut self.afrl, value, mask),
            AFRH => store(&mut self.afrh, value, mask),
            _ => {}
        }
    }

    // A system reset puts the port back to its reset values.
    fn reset(&mut self, _cause: ResetCause) {
        *self = GpioRegisters::new(self.port, self.sd.clone(), self.rtc.clone());
        if self.port == PORT_D {
            self.rtc.release_bus();
        }
        self.drive_wires();
    }
}

pub type Gpio = WordAccess<GpioRegisters>;

impl Gpio {
    /// `port` is 0 for GPIOA. `sd` is the socket whose wires pass through the ports that carry
    /// them (C and D) and whose switches are read from D3. `rtc` is the DS3231, on PD4 and PD5.
    pub fn new(port: usize, sd: SdSlot, rtc: Ds3231Slot) -> Gpio {
        WordAccess::wrap(GpioRegisters::new(port, sd, rtc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svd::Svd;
    use crate::{FixedClock, HostTime, MemoryMap, ResetCause};

    const GPIOA: u32 = GPIO_BASE;
    const GPIOB: u32 = GPIO_BASE + GPIO_PORT_SIZE;
    const GPIOC: u32 = GPIO_BASE + 2 * GPIO_PORT_SIZE;
    const GPIOD: u32 = GPIO_BASE + 3 * GPIO_PORT_SIZE;
    const GPIOF: u32 = GPIO_BASE + 5 * GPIO_PORT_SIZE;
    const GPIOG: u32 = GPIO_BASE + 6 * GPIO_PORT_SIZE;
    const GPIOH: u32 = GPIO_BASE + 7 * GPIO_PORT_SIZE;
    const GPIOK: u32 = GPIO_BASE + 10 * GPIO_PORT_SIZE;

    fn machine_with(sd: &SdSlot) -> MemoryMap {
        machine_on(sd, &Ds3231Slot::default())
    }

    fn machine_on(sd: &SdSlot, rtc: &Ds3231Slot) -> MemoryMap {
        let mut m = MemoryMap::new();
        for i in 0..GPIO_PORTS {
            m.map_peripheral(
                GPIO_BASE + i as u32 * GPIO_PORT_SIZE,
                GPIO_PORT_SIZE,
                Box::new(Gpio::new(i, sd.clone(), rtc.clone())),
            )
            .unwrap();
        }
        m
    }

    // A slot with a card in it, a 4-block image of 0x42.
    fn card_in() -> SdSlot {
        let sd = SdSlot::new();
        sd.insert(vec![0x42; 4 * 512]).unwrap();
        sd
    }

    // The board as the firmware found it: with a card in the socket.
    fn machine() -> MemoryMap {
        machine_with(&card_in())
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
    fn card_detect_reads_high_with_no_card_and_low_with_one() {
        let sd = SdSlot::new();
        let mut m = machine_with(&sd);
        configure(&mut m, GPIOD, 3, 0, 1);
        assert_eq!(pin(&mut m, GPIOD, 3), 1);
        sd.insert(vec![0; 512]).unwrap();
        assert_eq!(pin(&mut m, GPIOD, 3), 0);
        configure(&mut m, GPIOD, 3, 0, 0);
        assert_eq!(pin(&mut m, GPIOD, 3), 0);
        sd.eject();
        configure(&mut m, GPIOD, 3, 0, 1);
        assert_eq!(pin(&mut m, GPIOD, 3), 1);
    }

    #[test]
    fn write_protect_reads_high_when_the_slot_says_protected() {
        let sd = card_in();
        sd.set_write_protected(true);
        let mut m = machine_with(&sd);
        configure(&mut m, GPIOH, 12, 0, 1);
        assert_eq!(pin(&mut m, GPIOH, 12), 1);
        // The pull-up is what lifts it, as for the card-detect switch.
        configure(&mut m, GPIOH, 12, 0, 0);
        assert_eq!(pin(&mut m, GPIOH, 12), 0);
        sd.set_write_protected(false);
        configure(&mut m, GPIOH, 12, 0, 1);
        assert_eq!(pin(&mut m, GPIOH, 12), 0);
    }

    #[test]
    fn write_protect_reads_low_with_or_without_a_card() {
        for sd in [SdSlot::new(), card_in()] {
            let mut m = machine_with(&sd);
            configure(&mut m, GPIOH, 12, 0, 1);
            assert_eq!(pin(&mut m, GPIOH, 12), 0);
        }
    }

    #[test]
    fn the_card_detect_pin_is_only_d3() {
        let mut m = machine();
        configure(&mut m, GPIOD, 4, 0, 1);
        assert_eq!(pin(&mut m, GPIOD, 4), 1);
        configure(&mut m, GPIOC, 3, 0, 1);
        assert_eq!(pin(&mut m, GPIOC, 3), 1);
    }

    // The pins as the firmware sets them up for the bit-bang: PD2, PC10 and PC12 outputs,
    // PC11 an input with a pull-up. Chip select is high.
    fn socket_wired(sd: &SdSlot) -> MemoryMap {
        let mut m = machine_with(sd);
        m.write32(GPIOD + BSRR, 1 << SD_CS);
        configure(&mut m, GPIOD, SD_CS, 1, 0);
        configure(&mut m, GPIOC, SD_SCK, 1, 0);
        configure(&mut m, GPIOC, SD_MOSI, 1, 0);
        configure(&mut m, GPIOC, SD_MISO, 0, 1);
        m
    }

    // A byte the way the firmware's loop sends it: out through BSRR, the clock up, MISO read
    // while it is high, the clock down.
    fn transfer(m: &mut MemoryMap, byte: u8) -> u8 {
        let mut read = 0;
        for bit in (0..8).rev() {
            let mosi = if byte >> bit & 1 != 0 {
                1 << SD_MOSI
            } else {
                1 << (SD_MOSI + 16)
            };
            m.write32(GPIOC + BSRR, mosi);
            m.write32(GPIOC + BSRR, 1 << SD_SCK);
            read = read << 1 | pin(m, GPIOC, SD_MISO) as u8;
            m.write32(GPIOC + BSRR, 1 << (SD_SCK + 16));
        }
        read
    }

    fn cmd0_reply(m: &mut MemoryMap) -> Vec<u8> {
        for byte in [0x40, 0, 0, 0, 0, 0x95] {
            transfer(m, byte);
        }
        (0..3).map(|_| transfer(m, 0xFF)).collect()
    }

    #[test]
    fn cmd0_clocked_out_of_the_pins_is_answered_on_miso() {
        let mut m = socket_wired(&card_in());
        m.write32(GPIOD + BSRR, 1 << (SD_CS + 16));
        assert_eq!(cmd0_reply(&mut m), [0xFF, 0x01, 0xFF]);
    }

    #[test]
    fn nothing_answers_with_chip_select_high() {
        let mut m = socket_wired(&card_in());
        assert_eq!(cmd0_reply(&mut m), [0xFF; 3]);
    }

    #[test]
    fn nothing_answers_with_no_card() {
        let mut m = socket_wired(&SdSlot::new());
        m.write32(GPIOD + BSRR, 1 << (SD_CS + 16));
        assert_eq!(cmd0_reply(&mut m), [0xFF; 3]);
    }

    #[test]
    fn taking_the_clock_pin_off_its_output_does_not_clock_the_card() {
        let mut m = socket_wired(&card_in());
        m.write32(GPIOD + BSRR, 1 << (SD_CS + 16));
        configure(&mut m, GPIOC, SD_SCK, 0, 0);
        configure(&mut m, GPIOC, SD_SCK, 1, 0);
        assert_eq!(cmd0_reply(&mut m), [0xFF, 0x01, 0xFF]);
    }

    #[test]
    fn chip_select_goes_through_odr_as_well_as_bsrr() {
        let mut m = socket_wired(&card_in());
        m.write32(GPIOD + ODR, 0);
        assert_eq!(cmd0_reply(&mut m), [0xFF, 0x01, 0xFF]);
        m.write32(GPIOD + ODR, 1 << SD_CS);
        assert_eq!(cmd0_reply(&mut m), [0xFF; 3]);
    }

    #[test]
    fn the_wires_only_count_when_the_pins_are_outputs() {
        // Chip select low in ODR but PD2 an input: the board pulls it high, so no card hears.
        let mut m = socket_wired(&card_in());
        m.write32(GPIOD + MODER, !(3 << (2 * SD_CS)));
        m.write32(GPIOD + ODR, 0);
        assert_eq!(cmd0_reply(&mut m), [0xFF; 3]);
        // Switching it to an output with ODR low selects the card.
        configure(&mut m, GPIOD, SD_CS, 1, 0);
        assert_eq!(cmd0_reply(&mut m), [0xFF, 0x01, 0xFF]);
    }

    // A reset puts PD2 back to a released pin, which the board pulls high.
    #[test]
    fn a_reset_deselects_the_card() {
        let mut m = socket_wired(&card_in());
        m.write32(GPIOD + BSRR, 1 << (SD_CS + 16));
        m.reset_peripherals(ResetCause::Software);
        configure(&mut m, GPIOC, SD_SCK, 1, 0);
        configure(&mut m, GPIOC, SD_MOSI, 1, 0);
        configure(&mut m, GPIOC, SD_MISO, 0, 1);
        assert_eq!(cmd0_reply(&mut m), [0xFF; 3]);
    }

    #[test]
    fn miso_is_the_pull_up_with_no_card() {
        let mut m = socket_wired(&SdSlot::new());
        assert_eq!(pin(&mut m, GPIOC, SD_MISO), 1);
    }

    // The DS3231 on PD4 (SDA) and PD5 (SCL), clocked the way the firmware's bit-bang does it:
    // push-pull outputs written through BSRR, SDA switched to an input to read the answer.
    const SDA: u32 = 4;
    const SCL: u32 = 5;
    const ADDRESS: u8 = 0x68;

    // A chip on a fixed clock (Thursday 2026-12-24 13:45:56) and the board's port D wired to it,
    // with both pins outputs and high, as the firmware leaves the bus.
    fn clocked() -> (MemoryMap, Ds3231Slot) {
        let rtc = Ds3231Slot::new(Box::new(FixedClock::new(HostTime::new(
            2026, 12, 24, 13, 45, 56,
        ))));
        let mut m = machine_on(&SdSlot::new(), &rtc);
        m.write32(GPIOD + BSRR, 1 << SDA | 1 << SCL);
        configure(&mut m, GPIOD, SDA, 1, 0);
        configure(&mut m, GPIOD, SCL, 1, 0);
        (m, rtc)
    }

    fn drive(m: &mut MemoryMap, pin: u32, high: bool) {
        let shift = if high { pin } else { pin + 16 };
        m.write32(GPIOD + BSRR, 1 << shift);
    }

    fn i2c_start(m: &mut MemoryMap) {
        drive(m, SDA, true);
        drive(m, SCL, true);
        drive(m, SDA, false);
        drive(m, SCL, false);
    }

    fn i2c_stop(m: &mut MemoryMap) {
        drive(m, SDA, false);
        drive(m, SCL, true);
        drive(m, SDA, true);
    }

    // True if the chip acknowledged. SDA is an input for the ninth pulse.
    fn i2c_send(m: &mut MemoryMap, byte: u8) -> bool {
        for bit in (0..8).rev() {
            drive(m, SDA, byte >> bit & 1 != 0);
            drive(m, SCL, true);
            drive(m, SCL, false);
        }
        configure(m, GPIOD, SDA, 0, 0);
        drive(m, SCL, true);
        let acknowledged = pin(m, GPIOD, SDA) == 0;
        drive(m, SCL, false);
        configure(m, GPIOD, SDA, 1, 0);
        acknowledged
    }

    fn i2c_receive(m: &mut MemoryMap, acknowledge: bool) -> u8 {
        configure(m, GPIOD, SDA, 0, 0);
        let mut byte = 0;
        for _ in 0..8 {
            drive(m, SCL, true);
            byte = byte << 1 | pin(m, GPIOD, SDA) as u8;
            drive(m, SCL, false);
        }
        drive(m, SDA, !acknowledge);
        configure(m, GPIOD, SDA, 1, 0);
        drive(m, SCL, true);
        drive(m, SCL, false);
        byte
    }

    // The firmware's read of `count` registers from `register`.
    fn i2c_read(m: &mut MemoryMap, register: u8, count: usize) -> Vec<u8> {
        i2c_start(m);
        assert!(i2c_send(m, ADDRESS << 1));
        assert!(i2c_send(m, register));
        i2c_stop(m);
        i2c_start(m);
        assert!(i2c_send(m, ADDRESS << 1 | 1));
        let bytes = (0..count).map(|i| i2c_receive(m, i + 1 < count)).collect();
        i2c_stop(m);
        bytes
    }

    #[test]
    fn the_bus_idles_high_whatever_the_pull_registers_say() {
        let (mut m, _) = clocked();
        for pull in [0, 1, 2] {
            configure(&mut m, GPIOD, SDA, 0, pull);
            configure(&mut m, GPIOD, SCL, 0, pull);
            assert_eq!(pin(&mut m, GPIOD, SDA), 1, "SDA pull {pull}");
            assert_eq!(pin(&mut m, GPIOD, SCL), 1, "SCL pull {pull}");
        }
    }

    #[test]
    fn sda_reads_low_only_while_the_chip_acknowledges() {
        let (mut m, _) = clocked();
        i2c_start(&mut m);
        for bit in (0..8).rev() {
            drive(&mut m, SDA, ADDRESS << 1 >> bit & 1 != 0);
            drive(&mut m, SCL, true);
            drive(&mut m, SCL, false);
        }
        configure(&mut m, GPIOD, SDA, 0, 0);
        assert_eq!(pin(&mut m, GPIOD, SDA), 0);
        drive(&mut m, SCL, true);
        assert_eq!(pin(&mut m, GPIOD, SDA), 0);
        drive(&mut m, SCL, false);
        assert_eq!(pin(&mut m, GPIOD, SDA), 1);
    }

    #[test]
    fn a_read_clocked_through_the_pins_gives_the_hosts_time() {
        let (mut m, _) = clocked();
        assert_eq!(
            i2c_read(&mut m, 0, 7),
            [0x56, 0x45, 0x13, 0x04, 0x24, 0x12, 0x26]
        );
    }

    #[test]
    fn a_write_clocked_through_the_pins_sets_the_time() {
        let (mut m, rtc) = clocked();
        i2c_start(&mut m);
        for byte in [ADDRESS << 1, 0, 0x30, 0x20, 0x10] {
            assert!(i2c_send(&mut m, byte));
        }
        i2c_stop(&mut m);
        assert_eq!(i2c_read(&mut m, 0, 3), [0x30, 0x20, 0x10]);
        assert_eq!(rtc.offset_millis(), Some(-(3 * 3600 + 25 * 60 + 26) * 1000));
    }

    #[test]
    fn another_address_is_not_acknowledged_through_the_pins() {
        let (mut m, _) = clocked();
        i2c_start(&mut m);
        assert!(!i2c_send(&mut m, 0x69 << 1));
    }

    #[test]
    fn the_pins_of_other_ports_do_not_reach_the_chip() {
        // The same wiggling on port C's pins 4 and 5 does not wake it, and port C's pin 4
        // has no pull-up from the board.
        let (mut m, _) = clocked();
        m.write32(GPIOC + BSRR, 1 << 4 | 1 << 5);
        configure(&mut m, GPIOC, 4, 1, 0);
        configure(&mut m, GPIOC, 5, 1, 0);
        m.write32(GPIOC + BSRR, 1 << 20);
        m.write32(GPIOC + BSRR, 1 << 21);
        configure(&mut m, GPIOC, 4, 0, 0);
        assert_eq!(pin(&mut m, GPIOC, 4), 0);
        assert_eq!(i2c_read(&mut m, 0, 1), [0x56]);
    }

    #[test]
    fn a_released_scl_is_a_high_clock() {
        // With SCL an input the pull-up holds it high, so SDA falling is a START.
        let (mut m, _) = clocked();
        drive(&mut m, SDA, true);
        drive(&mut m, SCL, true);
        configure(&mut m, GPIOD, SCL, 0, 0);
        drive(&mut m, SDA, false);
        // Taking the clock back as an output, low, is its falling edge.
        drive(&mut m, SCL, false);
        configure(&mut m, GPIOD, SCL, 1, 0);
        assert!(i2c_send(&mut m, ADDRESS << 1));
    }

    #[test]
    fn a_reset_releases_the_bus() {
        let (mut m, _) = clocked();
        i2c_start(&mut m);
        // Eight clocks of the address, and the chip is now acknowledging.
        for bit in (0..8).rev() {
            drive(&mut m, SDA, ADDRESS << 1 >> bit & 1 != 0);
            drive(&mut m, SCL, true);
            drive(&mut m, SCL, false);
        }
        configure(&mut m, GPIOD, SDA, 0, 0);
        assert_eq!(pin(&mut m, GPIOD, SDA), 0);
        m.reset_peripherals(ResetCause::Software);
        configure(&mut m, GPIOD, SDA, 0, 0);
        assert_eq!(pin(&mut m, GPIOD, SDA), 1);
        // And the chip answers a new transaction after it.
        configure(&mut m, GPIOD, SDA, 1, 0);
        configure(&mut m, GPIOD, SCL, 1, 0);
        assert_eq!(i2c_read(&mut m, 0, 1), [0x56]);
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
