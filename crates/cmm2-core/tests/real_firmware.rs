use std::cell::RefCell;
use std::rc::Rc;

use cmm2_core::{
    Cpu, FLASH_BASE, FLASH_SIZE, Firmware, Irqs, PeripheralAccess, Stop, Svd, cmm2_memory,
};

fn firmware() -> Firmware {
    let path = std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
    Firmware::load(path).unwrap()
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_vector_table() {
    let vectors = firmware().vector_table();
    assert_eq!(vectors.initial_sp, 0x2001_FFF0);
    assert_eq!(vectors.reset_handler, 0x080A_DDD9);
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_its_first_instructions() {
    let firmware = firmware();
    let mut memory = cmm2_memory();
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, Irqs::new()).unwrap();

    assert_eq!(cpu.run(100_000), Stop::InstructionLimit);
    assert!(cpu.instruction_count() >= 100_000);
    let pc = cpu.pc();
    let in_flash = (FLASH_BASE..FLASH_BASE + FLASH_SIZE as u32).contains(&pc);
    assert!(
        in_flash || pc < 0x1_0000,
        "pc {pc:#010x} is not in flash or ITCM"
    );

    let svd = Svd::stm32h743();
    let names: Vec<&str> = trace
        .borrow()
        .iter()
        .filter_map(|access| svd.lookup(access.addr).map(|l| l.name))
        .collect();
    assert!(names.contains(&"RCC_CR"));
    assert!(names.contains(&"RCC_PLL1DIVR"));
    let first_rcc = names.iter().find(|name| name.starts_with("RCC_"));
    assert_eq!(first_rcc, Some(&"RCC_CR"));
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_passes_the_pwr_voltage_waits() {
    let firmware = firmware();
    let mut memory = cmm2_memory();
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, Irqs::new()).unwrap();

    let stop = cpu.run(3_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");

    let svd = Svd::stm32h743();
    let reads = |register: &str| {
        trace
            .borrow()
            .iter()
            .filter(|a| !a.write && svd.lookup(a.addr).is_some_and(|l| l.name == register))
            .count()
    };
    assert!(reads("PWR_CSR1") < 100, "{}", reads("PWR_CSR1"));
    assert!(reads("PWR_D3CR") < 100, "{}", reads("PWR_D3CR"));
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_passes_the_rcc_clock_waits() {
    let firmware = firmware();
    let mut memory = cmm2_memory();
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, Irqs::new()).unwrap();

    let stop = cpu.run(3_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");

    let svd = Svd::stm32h743();
    let cr_reads = trace
        .borrow()
        .iter()
        .filter(|a| !a.write && svd.lookup(a.addr).is_some_and(|l| l.name == "RCC_CR"))
        .count();
    assert!(cr_reads < 200, "{cr_reads}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_passes_the_rtc_waits() {
    let firmware = firmware();
    let mut memory = cmm2_memory();
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, Irqs::new()).unwrap();

    let stop = cpu.run(3_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");

    let svd = Svd::stm32h743();
    let isr_reads = trace
        .borrow()
        .iter()
        .filter(|a| !a.write && svd.lookup(a.addr).is_some_and(|l| l.name == "RTC_RTC_ISR"))
        .count();
    assert!(isr_reads < 200, "{isr_reads}");
}
