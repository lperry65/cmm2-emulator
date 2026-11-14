//! The video modes, the colour depths and the layers, as the firmware draws them.

mod common;

use common::*;

use std::cell::RefCell;
use std::rc::Rc;

use cmm2_core::{Console, Cpu, Irqs, Stop, cmm2_memory};

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
