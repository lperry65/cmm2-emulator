//! The machine as the firmware sees it: which model sits where. The models that exist are
//! mapped here; everything else in the peripheral space is a register stub.

use crate::flash::{FLASH_REGS_BASE, FLASH_REGS_SIZE, FlashController};
use crate::gpio::{GPIO_BASE, GPIO_PORT_SIZE, GPIO_PORTS, Gpio};
use crate::irq::Irqs;
use crate::memory::MemoryMap;
use crate::pwr::{PWR_BASE, PWR_SIZE, Pwr};
use crate::rcc::{RCC_BASE, RCC_SIZE, Rcc};
use crate::rtc::{RTC_BASE, RTC_SIZE, Rtc};
use crate::timer::{TIM5_BASE, TIM16_BASE, TIMER_SIZE, Timer};
use crate::usart::{USART_SIZE, USART1_BASE, Usart};

pub fn cmm2_memory(irqs: &Irqs) -> MemoryMap {
    let mut memory = MemoryMap::new();
    // The layout is fixed, so none of this can overlap.
    memory
        .map_peripheral(PWR_BASE, PWR_SIZE, Box::new(Pwr::new()))
        .expect("PWR window is clear of RAM");
    memory
        .map_peripheral(RCC_BASE, RCC_SIZE, Box::new(Rcc::new()))
        .expect("RCC window is clear of RAM");
    memory
        .map_peripheral(RTC_BASE, RTC_SIZE, Box::new(Rtc::new(memory.clock())))
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
                Box::new(Gpio::new(port)),
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
        assert_eq!(m.read32(0x5800_400C), 0xE7);
    }

    #[test]
    fn the_rtc_counts_with_the_machine_clock() {
        let mut m = cmm2_memory(&Irqs::new());
        assert_eq!(m.read32(0x5800_4000), 0); // RTC_TR
        m.clock().set(480_000_000);
        assert_eq!(m.read32(0x5800_4000), 1);
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
    fn ram_is_there() {
        let mut m = cmm2_memory(&Irqs::new());
        m.write32(0x2400_0000, 5);
        assert_eq!(m.read32(0x2400_0000), 5);
    }
}
