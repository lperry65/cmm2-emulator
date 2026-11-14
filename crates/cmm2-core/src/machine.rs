//! The machine as the firmware sees it: which model sits where. The models that exist are
//! mapped here; everything else in the peripheral space is a register stub.

use crate::flash::{FLASH_REGS_BASE, FLASH_REGS_SIZE, FlashController};
use crate::gpio::{GPIO_BASE, GPIO_PORT_SIZE, GPIO_PORTS, Gpio};
use crate::irq::Irqs;
use crate::ltdc::{LTDC_BASE, LTDC_SIZE, Ltdc};
use crate::memory::MemoryMap;
use crate::otg::{OTG_BASE, OTG_SIZE, Otg};
use crate::pwr::{PWR_BASE, PWR_SIZE, Pwr};
use crate::rcc::{RCC_BASE, RCC_SIZE, Rcc};
use crate::rtc::{RTC_BASE, RTC_SIZE};
use crate::timer::{TIM5_BASE, TIM16_BASE, TIMER_SIZE, Timer};
use crate::usart::{USART_SIZE, USART1_BASE, Usart};

pub fn cmm2_memory(irqs: &Irqs) -> MemoryMap {
    let mut memory = MemoryMap::new();
    // The layout is fixed, so none of this can overlap.
    memory
        .map_peripheral(PWR_BASE, PWR_SIZE, Box::new(Pwr::new()))
        .expect("PWR window is clear of RAM");
    let rcc = Rcc::new(memory.clock());
    let pixel_clock = rcc.pixel_clock();
    memory
        .map_peripheral(RCC_BASE, RCC_SIZE, Box::new(rcc))
        .expect("RCC window is clear of RAM");
    memory
        .map_peripheral(
            LTDC_BASE,
            LTDC_SIZE,
            Box::new(Ltdc::new(
                memory.clock(),
                memory.display(),
                irqs.clone(),
                pixel_clock,
            )),
        )
        .expect("LTDC window is clear of RAM");
    memory
        .map_peripheral(RTC_BASE, RTC_SIZE, Box::new(memory.rtc()))
        .expect("RTC window is clear of RAM");
    memory
        .map_peripheral(
            TIM5_BASE,
            TIMER_SIZE,
            Box::new(Timer::new(TIM5_BASE, 32, memory.clock())),
        )
        .expect("TIM5 window is clear of RAM");
    memory
        .map_peripheral(
            TIM16_BASE,
            TIMER_SIZE,
            Box::new(Timer::new(TIM16_BASE, 16, memory.clock())),
        )
        .expect("TIM16 window is clear of RAM");
    for port in 0..GPIO_PORTS {
        memory
            .map_peripheral(
                GPIO_BASE + port as u32 * GPIO_PORT_SIZE,
                GPIO_PORT_SIZE,
                Box::new(Gpio::new(port, memory.sd_slot(), memory.ds3231())),
            )
            .expect("GPIO window is clear of RAM");
    }
    memory
        .map_peripheral(
            USART1_BASE,
            USART_SIZE,
            Box::new(Usart::new(USART1_BASE, memory.console(), irqs.clone())),
        )
        .expect("USART1 window is clear of RAM");
    memory
        .map_peripheral(
            OTG_BASE,
            OTG_SIZE,
            Box::new(Otg::new(
                OTG_BASE,
                memory.clock(),
                irqs.clone(),
                memory.usb_port(),
            )),
        )
        .expect("OTG window is clear of RAM");
    let flash = FlashController::new(irqs.clone(), memory.flash_ops());
    memory
        .map_peripheral(FLASH_REGS_BASE, FLASH_REGS_SIZE, Box::new(flash))
        .expect("flash controller window is clear of RAM");
    memory
        .map_unmodelled_peripherals()
        .expect("stub windows are clear of RAM");
    memory
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb_keyboard::{HidKeyboard, KeyReport};

    #[test]
    fn pwr_is_served_by_the_model() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5802_4818) & 0x2000, 0x2000); // PWR_D3CR VOSRDY
    }

    #[test]
    fn rcc_is_served_by_the_model() {
        let mut m = cmm2_memory(&Irqs::new());
        m.write32(0x5802_4400, 1 << 16); // RCC_CR: HSEON
        assert_eq!(m.read32(0x5802_4400) & 1 << 17, 1 << 17); // HSERDY
    }

    #[test]
    fn rtc_is_served_by_the_model() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5800_400C), 0x27); // RTC_ISR
        m.write32(0x5800_400C, 0x80); // INIT
        assert_eq!(m.read32(0x5800_400C), 0xC4);
    }

    #[test]
    fn the_rtc_counts_with_the_machine_clock() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5800_4000), 0); // RTC_TR
        m.clock().set(480_000_000);
        assert_eq!(m.read32(0x5800_4000), 1);
    }

    #[test]
    fn the_frame_comes_from_the_ltdc_registers_and_the_ram() {
        let mut m = cmm2_memory(&Irqs::new());
        m.write32(0x5000_1018, 1); // LTDC_GCR: LTDCEN
        m.write32(0x5000_100C, 0x00EF_0017); // LTDC_BPCR
        m.write32(0x5000_1010, 0x040F_026F); // LTDC_AWCR
        m.write32(0x5000_1084, 1); // LTDC_L1CR: LEN
        m.write32(0x5000_1088, 0x040F_00F0); // LTDC_L1WHPCR
        m.write32(0x5000_108C, 0x026F_0018); // LTDC_L1WVPCR
        m.write32(0x5000_1094, 2); // LTDC_L1PFCR: RGB565
        m.write32(0x5000_1098, 0xFF); // LTDC_L1CACR
        m.write32(0x5000_10AC, 0xD000_0000); // LTDC_L1CFBAR
        m.write32(0x5000_10B0, 1600 << 16 | 1607); // LTDC_L1CFBLR
        m.write32(0x5000_10B4, 600); // LTDC_L1CFBLNR
        m.write32(0x5000_1024, 1); // LTDC_SRCR: IMR
        m.write16(0xD000_0000, 0xF800);
        let frame = m.frame().unwrap();
        assert_eq!((frame.width, frame.height), (800, 600));
        assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
    }

    #[test]
    fn the_ltdc_is_served_by_the_model() {
        let irqs = Irqs::new();
        let mut m = cmm2_memory(&irqs);
        // The pixel clock the firmware programs for 800 x 600: 8 MHz x 99 / 16.
        m.write32(0x5802_4428, 0x0010_1012); // RCC_PLLCKSELR
        m.write32(0x5802_4440, 0x0F0F_1E62); // RCC_PLL3DIVR
        m.write32(0x5802_442C, 0x013F_0CDD); // RCC_PLLCFGR
        m.write32(0x5802_4400, 0x1501_1001); // RCC_CR
        m.write32(0x5000_1040, 0x270); // LTDC_LIPCR
        m.write32(0x5000_1014, 0x041F_0270); // LTDC_TWCR
        m.write32(0x5000_1018, 1); // LTDC_GCR: LTDCEN
        m.write32(0x5000_1034, 1); // LTDC_IER: LIE
        m.clock().set(624 * 10_240);
        assert!(m.clock().take_due());
        m.tick();
        assert_eq!(irqs.take(), Some([0, 0, 1 << 24, 0, 0, 0, 0, 0])); // IRQ 88
        assert_eq!(m.read32(0x5000_1038), 1); // LTDC_ISR: LIF
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn the_stub_after_the_rtc_reads_its_reset_values() {
        let mut m = cmm2_memory(&Irqs::new());
        m.write32(0x5800_4400, 0x1234_5678); // just after the RTC window
        assert_eq!(m.read32(0x5800_4400), 0x1234_5678);
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn the_stub_next_to_the_models_still_reads_its_reset_values() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5802_4C00), 0xFFFF_FFFF); // CRC_DR, just after PWR
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn unmodelled_addresses_read_their_reset_values() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x4001_101C), 0xC0); // USART1_ISR
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn usart1_is_served_by_the_model_and_usart2_by_the_stub() {
        let mut m = cmm2_memory(&Irqs::new());
        let console = m.console();
        m.write32(0x4001_1000, 0x9); // USART1_CR1: UE, TE
        assert_eq!(m.read32(0x4001_101C), 0x0020_00C0); // USART1_ISR: TEACK
        m.write32(0x4001_1028, 0x41); // USART1_TDR
        assert_eq!(console.take_output(), b"A");

        m.write32(0x4000_4400, 0x9); // USART2_CR1
        assert_eq!(m.read32(0x4000_441C), 0xC0); // USART2_ISR: reset value only
        m.write32(0x4000_4428, 0x42); // USART2_TDR
        assert!(console.take_output().is_empty());
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn the_otg_is_served_by_the_model_with_the_ports_device() {
        let irqs = Irqs::new();
        let mut m = cmm2_memory(&irqs);
        assert_eq!(m.read32(0x4008_0040) >> 16, 0x4F54); // OTG_GSNPSID
        assert_eq!(m.read32(0x4008_0010), 0xA000_0000); // OTG_GRSTCTL: AHBIDL, CSRST_DONE
        m.usb_port().attach(HidKeyboard::new(KeyReport::new()));
        m.write32(0x4008_0440, 1 << 12); // OTG_HPRT: PPWR
        m.clock().set(480 * 200);
        assert!(m.clock().take_due());
        m.tick();
        assert_eq!(m.read32(0x4008_0440) & 1, 1); // PCSTS
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn flash_controller_is_served_by_the_model() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5200_210C), 0x31); // FLASH_CR2, locked
        m.write32(0x5200_2104, 0x4567_0123); // FLASH_KEYR2
        m.write32(0x5200_2104, 0xCDEF_89AB);
        assert_eq!(m.read32(0x5200_210C), 0x30);
    }

    #[test]
    fn tim16_and_tim5_count_from_the_maps_clock() {
        let mut m = cmm2_memory(&Irqs::new());
        let clock = m.clock();
        m.write32(TIM16_BASE + 0x28, 239); // PSC
        m.write32(TIM16_BASE, 1); // CEN
        m.write32(TIM5_BASE, 1);
        clock.set(480 * 1234);
        assert_eq!(m.read32(TIM16_BASE + 0x24), 1234);
        assert_eq!(m.read32(TIM5_BASE + 0x24), 480 * 1234 / 2);
        assert!(m.take_unmapped().is_empty());
    }

    #[test]
    fn the_other_timers_are_stubs() {
        let mut m = cmm2_memory(&Irqs::new());
        m.write32(0x4000_0024, 77); // TIM2_CNT
        m.clock().set(100_000);
        assert_eq!(m.read32(0x4000_0024), 77);
    }

    #[test]
    fn the_gpio_ports_are_served_by_the_model() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5802_0400), 0xFFFF_FEBF); // GPIOB_MODER, not the SVD's
        assert_eq!(m.read32(0x5802_2800), 0xFFFF_FFFF); // GPIOK_MODER
    }

    #[test]
    fn the_sd_slot_of_the_map_is_the_one_the_gpio_ports_read() {
        let mut m = cmm2_memory(&Irqs::new());
        let sd = m.sd_slot();
        m.write32(0x5802_0C0C, 1 << 6); // GPIOD_PUPDR: pull-up on PD3
        m.write32(0x5802_0C00, !(3 << 6)); // GPIOD_MODER: PD3 an input
        assert_eq!(m.read32(0x5802_0C10) >> 3 & 1, 1); // GPIOD_IDR: no card
        sd.insert(vec![0; 512]).unwrap();
        assert_eq!(m.read32(0x5802_0C10) >> 3 & 1, 0);
    }

    #[test]
    fn ram_is_there() {
        let mut m = cmm2_memory(&Irqs::new());
        m.write32(0x2400_0000, 5);
        assert_eq!(m.read32(0x2400_0000), 5);
    }
}
