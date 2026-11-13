use std::cell::RefCell;
use std::rc::Rc;

use cmm2_core::{
    Console, Cpu, FLASH_BASE, FLASH_SIZE, Firmware, Irqs, PeripheralAccess, Stop, Svd, cmm2_memory,
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
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

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
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

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
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

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
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

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

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_passes_the_flash_erase_wait() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let stop = cpu.run(3_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");

    // The wait for the flash interrupt handler was at 0x08094DD6.
    let pc = cpu.pc();
    assert!(!(0x0809_4DD6..=0x0809_4DE4).contains(&pc), "{pc:#010x}");

    const FLASH_CR2: u32 = 0x5200_210C;
    const SER_START: u32 = 1 << 2 | 1 << 7;
    let starts: Vec<u32> = trace
        .borrow()
        .iter()
        .filter(|a| a.write && a.addr == FLASH_CR2 && a.value & SER_START == SER_START)
        .map(|a| a.value)
        .collect();
    // The first one erases sector 7 of bank 2.
    assert_eq!(starts.first().map(|v| v >> 8 & 7), Some(7), "{starts:x?}");
    assert!(starts.len() >= 3, "{starts:x?}");

    // Measured: three erases (sector 7 of bank 2, then sector 1 twice), and the firmware has
    // programmed the first word of both sectors by the end of the run.
    assert_ne!(cpu.read32(0x081E_0000), 0xFFFF_FFFF);
    assert_ne!(cpu.read32(0x0812_0000), 0xFFFF_FFFF);
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_passes_the_pb13_loop() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let stop = cpu.run(3_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");

    // The firmware reads GPIOB_IDR bit 13, with a pull-up on. While it read low the firmware
    // waited 300 ms in the delay function at 0x0809ADE0 and tried again. It is the same delay
    // that the later SDRAM start-up uses, so the pc says nothing: check what the firmware read.
    let idr: Vec<u32> = trace
        .borrow()
        .iter()
        .filter(|a| !a.write && a.addr == 0x5802_0410)
        .map(|a| a.value)
        .collect();
    assert!(!idr.is_empty());
    assert_ne!(idr.last().unwrap() & 1 << 13, 0, "{idr:x?}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_passes_the_usart1_waits() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let console = memory.console();
    let trace = Rc::new(RefCell::new(Vec::<PeripheralAccess>::new()));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        sink.borrow_mut().push(*access)
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let stop = cpu.run(10_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");

    // The wait at 0x08009D2A used to read USART1_ISR 36 million times in a 400 million
    // instruction run. Measured: the firmware reads it 4 times in the whole run.
    let isr_reads = trace
        .borrow()
        .iter()
        .filter(|a| !a.write && a.addr == 0x4001_101C)
        .count();
    assert!((1..100).contains(&isr_reads), "{isr_reads}");

    // The firmware queues its text and sets TCIE, and the USART1 interrupt handler sends it.
    // Measured: the text starts at about 196.5 million instructions and the first prompt is
    // complete by 197.6 million.
    let stop = cpu.run(190_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    let output = String::from_utf8_lossy(&console.take_output()).into_owned();
    assert!(output.contains("Keyboard type"), "{output:?}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_receives_a_key_from_the_console() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let stop = cpu.run(200_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    let before = String::from_utf8_lossy(&console.take_output()).into_owned();
    assert!(before.contains("Keyboard type"), "{before:?}");

    console.push_input(b"1");
    let stop = cpu.run(20_000_000);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    let after = String::from_utf8_lossy(&console.take_output()).into_owned();
    // Measured: the key is echoed and the next setup prompt follows.
    assert!(after.contains("Enter time (24hr clock)"), "{after:?}");
}

/// Runs in slices until the console output so far contains `text`, or `cap` instructions have
/// run. Returns the output so far.
fn run_to_text(cpu: &mut Cpu, console: &Console, text: &str, cap: u64) -> String {
    let limit = cpu.instruction_count() + cap;
    let mut output = String::new();
    while !output.contains(text) && cpu.instruction_count() < limit {
        let stop = cpu.run(1_000_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    assert!(output.contains(text), "no {text:?} in {output:?}");
    output
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_reboots_after_the_date_prompt() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let mut output = run_to_text(&mut cpu, &console, "Keyboard type", 300_000_000);
    println!("keyboard prompt at {}", cpu.instruction_count());
    console.push_input(b"1");
    output += &run_to_text(&mut cpu, &console, "HH:MM:SS", 20_000_000);
    println!("time prompt at {}", cpu.instruction_count());
    console.push_input(b"12:00:00\r");
    output += &run_to_text(&mut cpu, &console, "DD/MM/YY", 20_000_000);
    println!("date prompt at {}", cpu.instruction_count());
    console.push_input(b"12/11/26\r");
    assert_eq!(cpu.resets(), 0);
    while cpu.resets() == 0 {
        assert!(cpu.instruction_count() < 250_000_000, "no reset");
        let stop = cpu.run(1_000_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    println!("reset at {}", cpu.instruction_count());
    let before_reboot = output.len();
    output += &run_to_text(&mut cpu, &console, "\r\n> ", 500_000_000);
    println!("second prompt at {}", cpu.instruction_count());

    assert_eq!(cpu.resets(), 1);
    assert_eq!(output.matches("MMBasic Version").count(), 2, "{output:?}");
    // Measured: the second banner is the board's own, 480MHz, where the first said 504MHz,
    // and no setup questions follow it.
    let second = &output[before_reboot..];
    assert!(second.contains("480MHz Colour Maximite 2 G2"), "{second:?}");
    assert!(!second.contains("Initial setup"), "{second:?}");
    assert!(second.ends_with("\r\n> "), "{second:?}");
}

/// Boots, answers the setup questions and runs through the reset to the second boot's prompt.
/// Returns the instruction count at the prompt.
fn boot_to_second_prompt(cpu: &mut Cpu, console: &Console) -> u64 {
    run_to_text(cpu, console, "Keyboard type", 300_000_000);
    console.push_input(b"1");
    run_to_text(cpu, console, "HH:MM:SS", 20_000_000);
    console.push_input(b"12:00:00\r");
    run_to_text(cpu, console, "DD/MM/YY", 20_000_000);
    console.push_input(b"12/11/26\r");
    while cpu.resets() == 0 {
        assert!(cpu.instruction_count() < 250_000_000, "no reset");
        let stop = cpu.run(1_000_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        console.take_output();
    }
    run_to_text(cpu, console, "\r\n> ", 500_000_000);
    cpu.instruction_count()
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_leaves_the_rtc_second_wait() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let console = memory.console();
    let trace = Rc::new(RefCell::new(0u64));
    let sink = trace.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        if !access.write && access.addr == 0x5800_4000 {
            *sink.borrow_mut() += 1;
        }
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let prompt = boot_to_second_prompt(&mut cpu, &console);
    // The firmware used to wait in a loop that read RTC_TR about 143,000 times and, with a
    // clock that did not count, never left it. Measured: it reads RTC_TR 3 times in the
    // whole run to the second prompt, and the second prompt is at 612 million instructions.
    let reads = *trace.borrow();
    println!("second prompt at {prompt}, RTC_TR reads so far: {reads}");
    assert!(reads < 1_000, "{reads}");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_a_command_at_the_prompt() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    boot_to_second_prompt(&mut cpu, &console);
    // Without the LTDC line interrupt the main thread waited at 0x0809A508 for it and never ran
    // the line. Measured: the first answer is back 17 million instructions after the keys and
    // the later ones 7 to 8 million, each the line echoed, the answer and a new prompt.
    let mut answer = |keys: &[u8]| {
        console.push_input(keys);
        let output = run_to_text(&mut cpu, &console, "\r\n> ", 50_000_000);
        let echo = String::from_utf8_lossy(&keys[..keys.len() - 1]).into_owned();
        let answer = output
            .strip_prefix(&format!("{echo}\r\n"))
            .and_then(|rest| rest.strip_suffix("\r\n> "))
            .unwrap_or_else(|| panic!("{output:?}"))
            .to_string();
        println!("{echo} -> {answer:?} at {}", cpu.instruction_count());
        answer
    };
    assert_eq!(answer(b"PRINT 1+2\r"), " 3");
    // The clock was set to 12:00:00 on 12/11/26 in the setup, and under a second has run since.
    let time = answer(b"PRINT TIME$\r");
    assert!(time.starts_with("12:00:0"), "{time:?}");
    assert_eq!(answer(b"PRINT DATE$\r"), "12-11-2026");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_keeps_its_saved_options_in_the_persistent_state() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    let first = boot_to_second_prompt(&mut cpu, &console);
    let saved = cpu.persistent();
    assert_ne!(cpu.read32(0x081E_0000), 0xFFFF_FFFF);

    // A new machine, started from what the first one saved, is a board that was set up before:
    // no setup questions and no reset on the way to the prompt.
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    cpu.load_persistent(&saved).unwrap();
    let output = run_to_text(&mut cpu, &console, "\r\n> ", 500_000_000);
    let second = cpu.instruction_count();
    println!("first run to the prompt {first}, second {second}");

    assert_eq!(cpu.resets(), 0);
    assert!(output.contains("MMBasic Version"), "{output:?}");
    assert!(!output.contains("Initial setup"), "{output:?}");
    // Measured: 412 million instructions to the prompt, where the first run took 612 million.
    assert!(second < first, "{second} against {first}");
    console.push_input(b"PRINT 1+2\r");
    let answer = run_to_text(&mut cpu, &console, "\r\n> ", 50_000_000);
    assert_eq!(answer, "PRINT 1+2\r\n 3\r\n> ");
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_draws_a_frame_at_the_prompt() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    boot_to_second_prompt(&mut cpu, &console);
    let frame = cpu.memory().frame().unwrap();
    assert_eq!((frame.width, frame.height), (800, 600));

    // The prompt is text: more than one colour, and some of it in the top left, where the
    // banner is. Look at it with the PPM the test leaves behind.
    let background = frame.pixel(799, 599);
    let colours: std::collections::HashSet<[u8; 3]> = (0..frame.height)
        .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
        .map(|(x, y)| frame.pixel(x, y))
        .collect();
    println!("{} colours, background {background:?}", colours.len());
    assert!(colours.len() > 1);
    let top_left_text = (0..100)
        .flat_map(|y| (0..400).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) != background)
        .count();
    assert!(top_left_text > 100, "{top_left_text}");

    if let Ok(path) = std::env::var("CMM2_SCREENSHOT") {
        std::fs::write(path, frame.to_ppm()).unwrap();
    }
}
