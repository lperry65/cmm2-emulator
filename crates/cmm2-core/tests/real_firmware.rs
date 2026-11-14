use std::cell::RefCell;
use std::rc::Rc;

use cmm2_core::{
    Console, Cpu, FLASH_BASE, FLASH_SIZE, Firmware, HidKeyboard, Irqs, KeyReport, PeripheralAccess,
    Stop, Svd, UsbPort, cmm2_memory,
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

// The text the firmware prints for one typed line: what lies between the echo of the line and
// the next prompt.
fn answer_to(cpu: &mut Cpu, console: &Console, line: &str) -> String {
    let (output, _) = type_keys(cpu, console, &format!("{line}\r"));
    output
        .strip_prefix(&format!("{line}\r\n"))
        .and_then(|rest| rest.strip_suffix("\r\n> "))
        .unwrap_or_else(|| panic!("{output:?}"))
        .to_string()
}

// MMBasic's floating point runs on the core's FPU. A plain FOR loop used to stop at the first
// VRINTM, which the core did not execute, and the rest of its maths library uses VRINTA, VRINTP
// and VFNMA. The answers are the ones Python gives, to the ten digits MMBasic prints.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_does_floating_point_at_the_prompt() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);

    let cases = [
        ("FOR i=1 TO 3:PRINT i:NEXT", " 1\r\n 2\r\n 3"),
        ("PRINT 1/3", " 0.3333333333"),
        ("PRINT SQR(2)", " 1.414213562"),
        ("PRINT SIN(1)", " 0.8414709848"),
        ("PRINT INT(2.7)", " 2"),
        ("PRINT 2^0.5", " 1.414213562"),
        ("PRINT 10/4", " 2.5"),
        ("PRINT 1E10", " 1e+10"),
        ("DIM a(10):FOR i=0 TO 10:a(i)=i*1.5:NEXT:PRINT a(10)", " 15"),
        (
            "PRINT COS(1),TAN(1),ATN(1),LOG(10),EXP(1)",
            " 0.5403023059\t 1.557407725\t 0.7853981634\t 2.302585093\t 2.718281828",
        ),
        ("PRINT STR$(1/3)", "0.3333333333"),
        ("PRINT VAL(\"3.14159\")", " 3.14159"),
        (
            "PRINT INT(-2.5),FIX(-2.5),CINT(2.5),CINT(3.5),CINT(-2.5)",
            "-3\t-2\t 3\t 4\t-3",
        ),
        // These need the whole of a double rounded down: 1E15+0.5 has 51 significant bits.
        (
            "PRINT INT(1E15+0.5),INT(-1E15-0.5),FIX(123456789012.9)",
            " 1000000000000000\t-1000000000000001\t 123456789012",
        ),
        (
            "x=0:FOR i=1 TO 200:x=x+SQR(i)*SIN(i)/(1+LOG(i+1)):NEXT:PRINT x",
            "-1.494066728",
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(answer_to(&mut cpu, &console, line), expected, "{line}");
    }
}

// The FPU used to do its arithmetic in a decimal type with a narrow exponent: 1E300*10 never
// came back and 1E-130/10 printed 0. These are what the board prints (7.00.00b5, at the prompt),
// subnormals kept and an overflow giving INF.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_does_floating_point_over_the_whole_range_of_a_double() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    // The first error after the boot prints no message, so one goes before the one asserted.
    answer_to(&mut cpu, &console, "PRINT INT(1E300)");

    let cases = [
        ("PRINT 1E300*10", " 1e+301"),
        ("PRINT -1E300*10", "-1e+301"),
        ("PRINT 1E-120/10", " 1e-121"),
        ("PRINT 1E-300/1E10", " 1e-310"),
        ("PRINT 1E-320", " 1.001980198e-320"),
        ("PRINT 1E308*10/10", " INF"),
        ("PRINT 0.1+0.2", " 0.3"),
        ("PRINT 1/3*3", " 1"),
        ("PRINT 1E15+0.3", " 1e+15"),
        ("PRINT 2^0.5*2^0.5", " 2"),
        ("PRINT SQR(1E300)", " 1e+150"),
        ("PRINT EXP(800)", " INF"),
        ("PRINT EXP(-800)", " 0"),
        ("PRINT LOG(1E-300)", "-690.7755279"),
        // An error is the console's cursor and colour codes, then its message.
        (
            "PRINT INT(1E300)",
            "\u{1b}[?25h\u{1b}[37m\u{1b}[mNumber too large\r\n",
        ),
        ("PRINT STR$(1E300*1E10)", "INF"),
    ];
    for (line, expected) in cases {
        assert_eq!(answer_to(&mut cpu, &console, line), expected, "{line}");
    }
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

// A 64 MB FAT32 card image made with mkfs.vfat. The test is ignored and already needs the
// firmware, and it needs this tool as well (dosfstools): without it the test fails and says so,
// and does not pass having tested nothing. The file lives in the temp directory under the
// test's own name.
fn fat32_image(name: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("cmm2-core-{}-{name}.img", std::process::id()));
    std::fs::File::create(&path)
        .and_then(|file| file.set_len(64 << 20))
        .unwrap();
    let made = std::process::Command::new("mkfs.vfat")
        .args(["-F", "32", "-n", "CMM2"])
        .arg(&path)
        .output();
    let image = match made {
        Ok(output) if output.status.success() => Ok(std::fs::read(&path).unwrap()),
        Ok(output) => Err(format!(
            "mkfs.vfat: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
        Err(err) => Err(format!(
            "this test needs mkfs.vfat (the dosfstools package) to make a card image: {err}"
        )),
    };
    std::fs::remove_file(&path).unwrap();
    image.unwrap_or_else(|message| panic!("{message}"))
}

// A little-endian number of up to four bytes.
fn le(bytes: &[u8]) -> usize {
    bytes.iter().rev().fold(0, |n, &b| n << 8 | b as usize)
}

// The contents of the file `name` (8.3 as stored: eight characters of name and three of
// extension, blank padded) in the root directory of a FAT32 image. The directory and the
// file have to fit in their first cluster.
fn fat32_file(image: &[u8], name: &[u8; 11]) -> Option<Vec<u8>> {
    let bytes_per_sector = le(&image[11..13]);
    let cluster_size = image[13] as usize * bytes_per_sector;
    let fat_sectors = le(&image[36..40]);
    let data = (le(&image[14..16]) + image[16] as usize * fat_sectors) * bytes_per_sector;
    let cluster = |n: usize| data + (n - 2) * cluster_size;
    let root = &image[cluster(le(&image[44..48]))..][..cluster_size];
    let entry = root.chunks(32).find(|entry| entry[..11] == name[..])?;
    let first = le(&entry[20..22]) << 16 | le(&entry[26..28]);
    Some(image[cluster(first)..][..le(&entry[28..32])].to_vec())
}

/// Types `keys` and runs to the next prompt. Returns everything the firmware printed on the
/// way, the echo of the keys included, and the instructions it took.
fn type_keys(cpu: &mut Cpu, console: &Console, keys: &str) -> (String, u64) {
    let start = cpu.instruction_count();
    console.push_input(keys.as_bytes());
    let mut output = String::new();
    while !output.ends_with("\r\n> ") {
        assert!(
            cpu.instruction_count() - start < 2_000_000_000,
            "{output:?}"
        );
        let stop = cpu.run(10_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    let spent = cpu.instruction_count() - start;
    println!("{keys:?} -> {output:?} in {spent} instructions");
    (output, spent)
}

// The CMM2 has no SAVE or LOAD for programs: AUTOSAVE takes a program typed at the console
// into a file (F1, as ESC O P, ends it and loads the file too), and RUN and LIST take a file
// name. The first command that reaches the card also initialises it, which costs about 5
// million instructions (the bit-bang runs slowly until the card is up); the later ones are
// quick.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_lists_saves_and_runs_a_program_on_a_card() {
    let image = fat32_image("saves");
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let sd = memory.sd_slot();
    sd.insert(image).unwrap();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    let mut say = |keys: &str| type_keys(&mut cpu, &console, keys).0;

    let files = say("FILES\r");
    assert!(files.contains("0 directories, 0 files"), "{files:?}");
    assert!(!files.contains("not found"), "{files:?}");
    say("NEW\r");
    let saved = say("AUTOSAVE \"T.BAS\"\rPRINT \"FROM THE CARD\"\r\x1bOP");
    assert!(saved.contains("Saved to SDcard"), "{saved:?}");
    let files = say("FILES\r");
    assert!(files.contains("T.BAS"), "{files:?}");

    // The program in memory is the one just saved. After NEW it is gone, and comes back
    // from the card.
    assert!(say("LIST\r").contains("PRINT \"FROM THE CARD\""));
    say("NEW\r");
    assert!(!say("LIST\r").contains("FROM THE CARD"));
    assert!(say("LIST \"T.BAS\"\r").contains("PRINT \"FROM THE CARD\""));
    let ran = say("RUN \"T.BAS\"\r");
    assert!(ran.contains("FROM THE CARD\r\n"), "{ran:?}");

    let image = sd.eject().unwrap();
    let file = fat32_file(&image, b"T       BAS").expect("no T.BAS on the card");
    assert!(String::from_utf8_lossy(&file).contains("PRINT \"FROM THE CARD\""));
}

// A card with its write-protect switch on is read, and the firmware refuses a write to it, with
// its own message; the image is left as it was and the card is not marked as written to.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_refuses_to_write_to_a_write_protected_card() {
    let image = fat32_image("protected");
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let sd = memory.sd_slot();
    sd.insert(image.clone()).unwrap();
    sd.set_write_protected(true);
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    let mut say = |keys: &str| type_keys(&mut cpu, &console, keys).0;

    let files = say("FILES\r");
    assert!(files.contains("0 directories, 0 files"), "{files:?}");
    let refused = say("OPEN \"W.TXT\" FOR OUTPUT AS #1\r");
    assert!(refused.contains("write protected"), "{refused:?}");
    assert!(!sd.modified());
    assert_eq!(sd.eject().unwrap(), image);
}

// With no card the socket reports it empty, and the firmware says so without a card to ask.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_says_the_card_is_not_present_with_no_card() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);

    // The first error the firmware prints after a boot comes out empty (not understood yet),
    // so the second FILES is the one that says it.
    type_keys(&mut cpu, &console, "FILES\r");
    let (files, _) = type_keys(&mut cpu, &console, "FILES\r");
    assert!(files.contains("SD card not present"), "{files:?}");
}

/// Runs in slices until `done` says so, or `cap` instructions have run. Returns everything the
/// console printed.
fn run_until(
    cpu: &mut Cpu,
    console: &Console,
    cap: u64,
    mut done: impl FnMut(&str) -> bool,
) -> String {
    let limit = cpu.instruction_count() + cap;
    let mut output = String::new();
    while !done(&output) && cpu.instruction_count() < limit {
        let stop = cpu.run(10_000);
        assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
        output.push_str(&String::from_utf8_lossy(&console.take_output()));
    }
    assert!(done(&output), "{output:?}");
    output
}

const KEY_A: u8 = 0x04;
const LEFT_SHIFT: u8 = 0x02;

/// What `boot_with_usb_keyboard` hands back: the machine with the keyboard enumerated and polled.
struct UsbBoot {
    cpu: Cpu,
    console: Console,
    keys: KeyReport,
    usb: UsbPort,
    /// What the firmware printed from the second prompt to the connect message.
    connect_output: String,
}

// Boots with a USB keyboard plugged in, and runs until the firmware has enumerated it and
// started to poll it. The USB task starts a second or so after the prompt, and the keyboard is
// usable about a second after that (docs/usb-host.md section 3), so there is no point in
// setting a key before the first poll.
fn boot_with_usb_keyboard() -> UsbBoot {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let console = memory.console();
    let keys = KeyReport::new();
    let usb = memory.usb_port();
    usb.attach(HidKeyboard::new(keys.clone()));
    // The firmware polls with a write of HCCHAR for channel 1 that has the enable bit set.
    let polls = Rc::new(RefCell::new(0u32));
    let sink = polls.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        if access.write && access.addr == 0x4008_0520 && access.value & 1 << 31 != 0 {
            *sink.borrow_mut() += 1;
        }
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();

    let prompt = boot_to_second_prompt(&mut cpu, &console);
    println!(
        "prompt at {prompt} instructions, cycle {}",
        cpu.cycle_count()
    );
    let connect_output = run_until(&mut cpu, &console, 200_000_000, |text| {
        text.contains("USB Keyboard Connected on channel 1\r\n")
    });
    println!(
        "connected at {} instructions, cycle {}",
        cpu.instruction_count(),
        cpu.cycle_count()
    );
    run_until(&mut cpu, &console, 400_000_000, |_| *polls.borrow() > 0);
    println!(
        "first poll at {} instructions, cycle {}",
        cpu.instruction_count(),
        cpu.cycle_count()
    );
    UsbBoot {
        cpu,
        console,
        keys,
        usb,
        connect_output,
    }
}

// The firmware's USB stack enumerates the keyboard on the OTG model, and prints the message it
// prints for a keyboard on the real board.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_enumerates_a_usb_keyboard() {
    let boot = boot_with_usb_keyboard();
    // Measured: the first boot's questions end with the UK keyboard, so the message says UK.
    assert_eq!(
        boot.connect_output, "UK USB Keyboard Connected on channel 1\r\n",
        "{:?}",
        boot.connect_output
    );
}

// Runs `instructions` and returns what the console printed meanwhile.
fn run_and_collect(boot: &mut UsbBoot, instructions: u64) -> String {
    let stop = boot.cpu.run(instructions);
    assert!(!matches!(stop, Stop::Fault(_)), "{stop:?}");
    String::from_utf8_lossy(&boot.console.take_output()).into_owned()
}

// Types `line`, then runs on, and sets each report after the number of instructions that go with
// it. Returns the output up to the next prompt.
fn run_line_with_keys(boot: &mut UsbBoot, line: &str, events: &[(u64, [u8; 8])]) -> String {
    let start = boot.cpu.instruction_count();
    boot.console.push_input(line.as_bytes());
    let mut output = String::new();
    for &(instructions, report) in events {
        output += &run_and_collect(boot, instructions);
        boot.keys.set(report);
    }
    while !output.ends_with("\r\n> ") {
        assert!(
            boot.cpu.instruction_count() - start < 500_000_000,
            "{output:?}"
        );
        output += &run_and_collect(boot, 10_000);
    }
    output
}

// Keys set in the report reach the firmware through the polling: KEYDOWN() counts them and names
// the first, and a release brings the count back to 0.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_reads_the_held_keys_of_a_usb_keyboard() {
    let mut boot = boot_with_usb_keyboard();

    // 'a' goes down while the first PAUSE runs and up during the second: the first reading is
    // taken while it is down, and the second after it has been let go. 100 ms is about 22 million
    // instructions, and the key is down for 38 million, short of the firmware's repeat delay.
    let output = run_line_with_keys(
        &mut boot,
        "PAUSE 100:a=KEYDOWN(0):b=KEYDOWN(1):PAUSE 200:c=KEYDOWN(0):PRINT STR$(a)+\",\"+STR$(b)+\",\"+STR$(c)\r",
        &[
            (2_000_000, [0, 0, KEY_A, 0, 0, 0, 0, 0]),
            (38_000_000, [0; 8]),
        ],
    );
    // 97 is "a": the firmware reports the character, not the usage.
    assert!(output.ends_with("\r\n1,97,0\r\n> "), "{output:?}");

    // Two keys and a modifier: the count, and KEYDOWN(7), which has the modifier bits.
    let output = run_line_with_keys(
        &mut boot,
        "PAUSE 100:a=KEYDOWN(0):b=KEYDOWN(7):PAUSE 200:PRINT STR$(a)+\",\"+STR$(b)\r",
        &[
            (2_000_000, [LEFT_SHIFT, 0, KEY_A, KEY_A + 1, 0, 0, 0, 0]),
            (38_000_000, [0; 8]),
        ],
    );
    assert!(output.ends_with("\r\n2,8\r\n> "), "{output:?}");
}

// INKEY$ returns the character that a key down sends, and at the prompt it is typed into the
// console, shifted when the modifier byte says so. The device sends a report only when it has
// changed, so a press shorter than the poll is not seen.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_types_the_keys_of_a_usb_keyboard() {
    let mut boot = boot_with_usb_keyboard();

    // The line's own end of line is still in the input when it starts, so it empties the input
    // first. 20 ms is about 4.4 million instructions.
    let output = run_line_with_keys(
        &mut boot,
        "PAUSE 20:DO WHILE INKEY$<>\"\":LOOP:DO:a$=INKEY$:LOOP UNTIL a$<>\"\":PRINT \"got \"+a$\r",
        &[
            (8_000_000, [0, 0, KEY_A, 0, 0, 0, 0, 0]),
            (5_000_000, [0; 8]),
        ],
    );
    assert!(output.ends_with("\r\ngot a\r\n> "), "{output:?}");

    // The release has not been polled yet, and a report that is set and reset between two polls
    // is never seen: give the firmware time to see it.
    run_and_collect(&mut boot, 5_000_000);

    // Shift and 'a' at the idle prompt type "A".
    boot.keys.set([LEFT_SHIFT, 0, KEY_A, 0, 0, 0, 0, 0]);
    let mut typed = run_and_collect(&mut boot, 5_000_000);
    boot.keys.set([0; 8]);
    typed += &run_and_collect(&mut boot, 5_000_000);
    assert_eq!(typed, "A");
}

// Pulling the keyboard out is noticed, and plugging one in again is a fresh enumeration.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_sees_a_usb_keyboard_come_and_go() {
    let mut boot = boot_with_usb_keyboard();

    boot.usb.detach();
    run_until(&mut boot.cpu, &boot.console, 100_000_000, |text| {
        text.contains("USB Keyboard Disconnected\r\n")
    });
    boot.usb.attach(HidKeyboard::new(boot.keys.clone()));
    run_until(&mut boot.cpu, &boot.console, 400_000_000, |text| {
        text.contains("UK USB Keyboard Connected on channel 1\r\n")
    });
}

// The video modes. MODE sets the resolution and the colour depth, and the firmware programs
// the LTDC and PLL3 for each (see docs/video.md). The shapes are drawn with RGB() colours and
// the picture is the LTDC's frame, so these check the model of the formats, the blending and
// the layers against what the firmware does to them.

/// Boots to the second prompt and returns the machine.
fn booted() -> (Cpu, Console) {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    (cpu, console)
}

/// Types each line of BASIC in turn, to its prompt. CLS clears the screen after the echo, so
/// the prompt is not always at the start of a line.
fn say_all(cpu: &mut Cpu, console: &Console, lines: &[&str]) {
    for line in lines {
        console.push_input(format!("{line}\r").as_bytes());
        run_until(cpu, console, 2_000_000_000, |output| output.ends_with("> "));
    }
}

/// Writes the frame as `<name>.ppm` in the directory CMM2_SCREENSHOTS names, if it is set.
fn screenshot(frame: &cmm2_core::Frame, name: &str) {
    if let Ok(dir) = std::env::var("CMM2_SCREENSHOTS") {
        std::fs::write(format!("{dir}/{name}.ppm"), frame.to_ppm()).unwrap();
    }
}

// What the firmware's colour table makes of the 8-bit colours: three bits of red and green and
// two of blue, each to its level (the top one is 0xE0, not 0xFF).
const LEVELS_3_BITS: [u8; 8] = [0x00, 0x24, 0x48, 0x68, 0x91, 0xB0, 0xD0, 0xE0];
const LEVELS_2_BITS: [u8; 4] = [0x00, 0x54, 0xA8, 0xE0];

fn at_8_bits([r, g, b]: [u8; 3]) -> [u8; 3] {
    [
        LEVELS_3_BITS[usize::from(r >> 5)],
        LEVELS_3_BITS[usize::from(g >> 5)],
        LEVELS_2_BITS[usize::from(b >> 6)],
    ]
}

// ARGB4444: four bits a channel, each shown as the nibble repeated.
fn at_12_bits(rgb: [u8; 3]) -> [u8; 3] {
    rgb.map(|c| (c >> 4) * 17)
}

// The firmware dims the 16 and 32-bit depths by a constant alpha of 0xE0 over black, to match
// the top level of the 8-bit table. This is c x 224 / 255 to the nearest level, which is the
// model's rounding and not necessarily the chip's (to be checked against RM0433).
fn dimmed(c: u8) -> u8 {
    (f64::from(c) * 224.0 / 255.0).round() as u8
}

// RGB565: five, six and five bits, each widened by repeating its high bits.
fn at_16_bits([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (r >> 3, g >> 2, b >> 3);
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2].map(dimmed)
}

fn at_32_bits(rgb: [u8; 3]) -> [u8; 3] {
    rgb.map(dimmed)
}

const RED: [u8; 3] = [255, 0, 0];
// ARGB4444 alpha 8 is 8 x 17 of 255: that much green over the rest of the red.
const HALF_GREEN_OVER_RED: [u8; 3] = [255 - 8 * 17, 8 * 17, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const SKY: [u8; 3] = [64, 128, 192];
const NAVY: [u8; 3] = [0, 0, 64];

// Each depth of the two resolutions the firmware's MODE gives in every depth: the command, the
// size, the pixel clock's frame rate to a hundredth of a Hz, and the colour as shown.
// 800 x 600 is 75 Hz, and 640 x 480 is 74.73 Hz, which agrees with what the board measured
// (748 ticks in 10 s, 74.8 +- 0.1 Hz) without the measurement confirming the decimals.
#[allow(clippy::type_complexity)]
const DEPTHS: [(&str, (usize, usize), fn([u8; 3]) -> [u8; 3]); 8] = [
    ("MODE 1,8", (800, 600), at_8_bits),
    ("MODE 1,12", (800, 600), at_12_bits),
    ("MODE 1,16", (800, 600), at_16_bits),
    ("MODE 1,32", (800, 600), at_32_bits),
    ("MODE 8,8", (640, 480), at_8_bits),
    ("MODE 8,12", (640, 480), at_12_bits),
    ("MODE 8,16", (640, 480), at_16_bits),
    ("MODE 8,32", (640, 480), at_32_bits),
];

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_draws_shapes_in_every_colour_depth() {
    let (mut cpu, console) = booted();
    for (mode, (width, height), shown) in DEPTHS {
        say_all(
            &mut cpu,
            &console,
            &[
                mode,
                "CLS RGB(0,0,64)",
                "BOX 20,100,100,60,1,RGB(255,0,0),RGB(255,0,0)",
                "CIRCLE 250,130,40,1,1,RGB(0,255,0),RGB(0,255,0)",
                "BOX 400,100,100,60,1,RGB(64,128,192),RGB(64,128,192)",
                "TEXT 20,300,\"Hello\",,1,1,RGB(255,255,255)",
            ],
        );
        let frame = cpu
            .memory()
            .frame()
            .unwrap_or_else(|e| panic!("{mode}: {e}"));
        screenshot(&frame, &mode.replace([' ', ','], "_"));
        assert_eq!((frame.width, frame.height), (width, height), "{mode}");
        assert_eq!(frame.pixel(70, 130), shown(RED), "{mode} box");
        assert_eq!(frame.pixel(250, 130), shown(GREEN), "{mode} circle");
        assert_eq!(frame.pixel(450, 130), shown(SKY), "{mode} box");
        assert_eq!(frame.pixel(600, 400), shown(NAVY), "{mode} background");
        // The text is in the box 20,300 to 60,312: some of it is white.
        let white = (300..312)
            .flat_map(|y| (20..60).map(move |x| (x, y)))
            .filter(|&(x, y)| frame.pixel(x, y) == shown(WHITE))
            .count();
        assert!(white > 20, "{mode} text: {white} white pixels");
    }
}

// PAGE DISPLAY flips for real, after waiting for the frame blanking, and in the 12-bit depth
// has a second layer: page 0 is the lower and page 1 the upper, over the background colour.
// Pages are 614,400 bytes apart from 0xD0000000 at 640 x 480.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_flips_the_displayed_page_and_blends_the_layers() {
    let (mut cpu, console) = booted();
    let see = |cpu: &mut Cpu, name: &str| {
        let frame = cpu.memory().frame().unwrap();
        screenshot(&frame, name);
        frame
    };
    // A blue background, and a red box on page 0.
    say_all(
        &mut cpu,
        &console,
        &[
            "MODE 8,12,RGB(0,0,128)",
            "BOX 0,100,320,100,1,RGB(255,0,0),RGB(255,0,0)",
        ],
    );
    let frame = see(&mut cpu, "pages_0");
    assert_eq!(frame.pixel(100, 150), RED);
    // Page 0 is clear to the right of the box, so the background shows through.
    assert_eq!(frame.pixel(500, 150), [0, 0, 0x80]);

    // A green box on page 2 is not shown until the page is.
    say_all(
        &mut cpu,
        &console,
        &[
            "PAGE WRITE 2",
            "BOX 0,100,320,100,1,RGB(0,255,0),RGB(0,255,0)",
        ],
    );
    assert_eq!(see(&mut cpu, "pages_2_hidden").pixel(100, 150), RED);
    say_all(&mut cpu, &console, &["PAGE DISPLAY 2"]);
    let frame = see(&mut cpu, "pages_2");
    assert_eq!(frame.pixel(100, 150), GREEN);
    assert_eq!(frame.pixel(500, 150), [0, 0, 0x80]);
    // Layer 1 reads from page 2 now.
    assert_eq!(cpu.memory().read32(0x5000_10AC), 0xD000_0000 + 2 * 614_400);
    say_all(&mut cpu, &console, &["PAGE DISPLAY 0"]);
    assert_eq!(see(&mut cpu, "pages_0_again").pixel(100, 150), RED);

    // One pixel of the upper layer, page 1, half transparent green: alpha 8 is 8 x 17 = 136 of
    // 255, so 136 of the green and 255 - 136 = 119 of the red below it. Both are whole levels
    // (136 x 255 / 255), so this does not depend on how the blend rounds.
    say_all(
        &mut cpu,
        &console,
        &["POKE SHORT MM.INFO(PAGE ADDRESS 1)+(150*640+100)*2,&H80F0"],
    );
    let frame = see(&mut cpu, "pages_layer_2");
    assert_eq!(frame.pixel(100, 150), HALF_GREEN_OVER_RED);
    assert_eq!(frame.pixel(101, 150), RED);
    // The upper layer can be flipped too: page 3 is clear.
    say_all(&mut cpu, &console, &["PAGE DISPLAY 3,1"]);
    assert_eq!(cpu.memory().read32(0x5000_112C), 0xD000_0000 + 3 * 614_400);
    assert_eq!(see(&mut cpu, "pages_layer_2_page_3").pixel(100, 150), RED);
    say_all(&mut cpu, &console, &["PAGE DISPLAY 1,1"]);
    assert_eq!(
        see(&mut cpu, "pages_layer_2_page_1").pixel(100, 150),
        HALF_GREEN_OVER_RED
    );
}

// The frame comes round at the rate the pixel clock gives it, which the firmware sets for the
// mode by programming PLL3: the line interrupt's handler writes LTDC_LIPCR once a frame.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_each_mode_at_its_frame_rate() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let console = memory.console();
    let clock = memory.clock();
    let frames = Rc::new(RefCell::new(Vec::<u64>::new()));
    let sink = frames.clone();
    memory.set_peripheral_trace(Some(Box::new(move |access| {
        if access.write && access.addr == 0x5000_1040 {
            sink.borrow_mut().push(clock.now());
        }
    })));
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);

    // The frame rate over twenty frames at the prompt, from the cycles between the writes.
    let hz = |cpu: &mut Cpu| {
        frames.borrow_mut().clear();
        while frames.borrow().len() < 21 {
            assert!(!matches!(cpu.run(100_000), Stop::Fault(_)));
        }
        let times = frames.borrow();
        let period = (times[20] - times[0]) as f64 / 20.0;
        cmm2_core::CORE_HZ as f64 / period
    };
    // 49.5 MHz over 1056 x 625.
    let hz_800 = hz(&mut cpu);
    println!("800 x 600: {hz_800:.3} Hz");
    assert!((hz_800 - 75.0).abs() < 0.01, "{hz_800}");
    // 8 MHz x 102 / 26 = 31.38 MHz over 840 x 500: 74.725 Hz, within the board's 74.8 +- 0.1.
    say_all(&mut cpu, &console, &["MODE 8,12"]);
    let hz_640 = hz(&mut cpu);
    println!("640 x 480: {hz_640:.3} Hz");
    assert!((hz_640 - 74.725).abs() < 0.01, "{hz_640}");
}
