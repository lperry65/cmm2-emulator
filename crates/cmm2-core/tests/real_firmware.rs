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

// A 64 MB FAT32 card image made with mkfs.vfat, or None, with a note on stderr, when the tool
// is not installed. It lives in the temp directory under the test's own name.
fn fat32_image(name: &str) -> Option<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("cmm2-core-{}-{name}.img", std::process::id()));
    std::fs::File::create(&path)
        .and_then(|file| file.set_len(64 << 20))
        .unwrap();
    let made = std::process::Command::new("mkfs.vfat")
        .args(["-F", "32", "-n", "CMM2"])
        .arg(&path)
        .output();
    let image = match made {
        Ok(output) if output.status.success() => Some(std::fs::read(&path).unwrap()),
        Ok(output) => panic!("mkfs.vfat: {}", String::from_utf8_lossy(&output.stderr)),
        Err(err) => {
            eprintln!("skipped: no mkfs.vfat on this machine ({err})");
            None
        }
    };
    std::fs::remove_file(&path).unwrap();
    image
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
    let Some(image) = fat32_image("saves") else {
        return;
    };
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
