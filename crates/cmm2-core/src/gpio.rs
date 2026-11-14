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
mod tests;
