use super::format::div255;
use super::*;

// The board's active area: 800 x 600, with the porches the firmware programs.
const BPCR: u32 = 0x00EF_0017;
const AWCR: u32 = 0x040F_026F;
const WHPCR: u32 = 0x040F_00F0;
const WVPCR: u32 = 0x026F_0018;
const BASE: u32 = 0x2400_0000;

// The pixel formats, as PFCR numbers them.
const ARGB8888: u32 = 0;
const RGB888: u32 = 1;
const RGB565: u32 = 2;
const ARGB1555: u32 = 3;
const ARGB4444: u32 = 4;
const L8: u32 = 5;

// What the firmware programs in BFCR: constant alpha, or pixel alpha times it.
const BF_CONSTANT: u32 = 0x405;
const BF_PIXEL: u32 = 0x607;

// The firmware's layer 1 on the board: opaque black as the default colour, constant alpha
// 255 and blending with the constant alpha.
fn layer(format: u32, cr: u32, pitch: u32) -> Layer {
    Layer {
        cr,
        whpcr: WHPCR,
        wvpcr: WVPCR,
        pfcr: format,
        cacr: 0xFF,
        dccr: 0xFF00_0000,
        bfcr: BF_CONSTANT,
        cfbar: BASE,
        cfblr: pitch << 16 | (pitch + 7),
        cfblnr: 600,
    }
}

fn board(format: u32, cr: u32) -> Display {
    let display = Display::default();
    display.update(|s| {
        s.enabled = true;
        s.bpcr = BPCR;
        s.awcr = AWCR;
        // Layer 2 is off, as the firmware leaves it until it is used.
        s.layers = [layer(format, cr, 800), Layer::default()];
    });
    display
}

// The board with a second layer that shows the same part of the screen from `BASE`, the
// way the firmware sets up the 12-bit depth: blended by pixel alpha, a transparent
// default colour.
fn two_layers(format1: u32, format2: u32) -> Display {
    let display = board(format1, LEN);
    display.update(|s| {
        s.layers[1] = Layer {
            dccr: 0,
            bfcr: BF_PIXEL,
            ..layer(format2, LEN, 800)
        };
    });
    display
}

// Layer 1 shows through where it has nothing: a transparent default colour, blended by pixel
// alpha.
fn transparent_default(display: &Display) {
    display.update(|s| {
        s.layers[0].dccr = 0;
        s.layers[0].bfcr = BF_PIXEL;
    });
}

// A RAM that holds `bytes` from `BASE` on and nothing else.
fn ram<'a>(bytes: &'a [u8]) -> impl Fn(u32, usize) -> Option<&'a [u8]> {
    move |addr, len| {
        let start = addr.checked_sub(BASE)? as usize;
        bytes.get(start..start + len)
    }
}

// A RAM of 16-bit pixels, the first of which is `first`, and then `rest`.
fn pixels16(first: u16, rest: u16) -> Vec<u8> {
    let mut memory = rest.to_le_bytes().repeat(1600 * 600 / 2);
    memory[..2].copy_from_slice(&first.to_le_bytes());
    memory
}

fn pitch16(display: &Display) {
    display.update(|s| {
        for layer in &mut s.layers {
            layer.cfblr = 1600 << 16 | 1607;
        }
    });
}

#[test]
fn nothing_is_shown_while_the_controller_is_off() {
    let display = Display::default();
    assert_eq!(display.frame(ram(&[])), Err(FrameError::Off));
}

#[test]
fn the_frame_is_the_active_area() {
    let display = board(L8, LEN | CLUTEN);
    let frame = display.frame(ram(&vec![0; 800 * 600])).unwrap();
    assert_eq!((frame.width, frame.height), (800, 600));
    assert_eq!(frame.rgb.len(), 800 * 600 * 3);
}

#[test]
fn l8_looks_each_pixel_up_in_the_colour_table() {
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.clut[0][1] = 0x0011_2233;
        s.clut[0][255] = 0x00AA_BBCC;
    });
    let mut memory = vec![0; 800 * 600];
    memory[0] = 1;
    memory[799] = 255;
    memory[800 * 599 + 5] = 1;
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
    assert_eq!(frame.pixel(1, 0), [0, 0, 0]);
    assert_eq!(frame.pixel(799, 0), [0xAA, 0xBB, 0xCC]);
    assert_eq!(frame.pixel(5, 599), [0x11, 0x22, 0x33]);
}

#[test]
fn each_layer_has_its_own_colour_table() {
    let display = two_layers(L8, L8);
    display.update(|s| {
        s.layers[1].cr |= CLUTEN;
        s.layers[0].cr |= CLUTEN;
        s.clut[0][1] = 0x0011_2233;
        s.clut[1][1] = 0x0044_5566;
        // Layer 2's window covers only the first 2 pixels of a row, layer 1 shows elsewhere.
        s.layers[1].whpcr = (0xF0 + 1) << 16 | 0xF0;
    });
    let frame = display.frame(ram(&vec![1; 800 * 600])).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x44, 0x55, 0x66]);
    assert_eq!(frame.pixel(1, 0), [0x44, 0x55, 0x66]);
    assert_eq!(frame.pixel(2, 0), [0x11, 0x22, 0x33]);
}

#[test]
fn the_pitch_steps_from_line_to_line() {
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.clut[0][7] = 0x0000_00FF;
        s.layers[0].cfblr = 1024 << 16 | 807;
    });
    let mut memory = vec![0; 1024 * 600];
    memory[1024 * 3 + 2] = 7;
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(2, 3), [0, 0, 0xFF]);
    assert_eq!(frame.pixel(2, 2), [0, 0, 0]);
}

#[test]
fn the_pitch_is_thirteen_bits() {
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.clut[0][1] = 0x00FF_FFFF;
        s.layers[0].whpcr = 0xF3 << 16 | 0xF0;
        s.layers[0].wvpcr = 0x19 << 16 | 0x18;
        s.layers[0].cfblr = 5000 << 16 | 11;
    });
    let mut memory = vec![0; 5004];
    memory[5000] = 1;
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0, 0, 0]);
    assert_eq!(frame.pixel(0, 1), [255, 255, 255]);
}

#[test]
fn the_layer_window_is_placed_inside_the_active_area() {
    // A 4 x 2 window whose top left pixel is at (10, 20) of the active area.
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.clut[0][9] = 0x0000_FF00;
        s.bccr = 0x0010_2030;
        s.layers[0].whpcr = (0xF0 + 13) << 16 | (0xF0 + 10);
        s.layers[0].wvpcr = (0x18 + 21) << 16 | (0x18 + 20);
        s.layers[0].cfblr = 4 << 16 | 11;
    });
    transparent_default(&display);
    let frame = display.frame(ram(&[9; 8])).unwrap();
    assert_eq!(frame.pixel(9, 20), [0x10, 0x20, 0x30]);
    assert_eq!(frame.pixel(10, 20), [0, 0xFF, 0]);
    assert_eq!(frame.pixel(13, 21), [0, 0xFF, 0]);
    assert_eq!(frame.pixel(14, 21), [0x10, 0x20, 0x30]);
    assert_eq!(frame.pixel(10, 22), [0x10, 0x20, 0x30]);
    assert_eq!(frame.pixel(10, 19), [0x10, 0x20, 0x30]);
}

#[test]
fn a_window_that_misses_the_active_area_or_ends_before_it_starts_draws_nothing() {
    for (whpcr, wvpcr) in [
        (0x0600_0500, WVPCR), // right of the active area
        (0x0100_0200, WVPCR), // ends before it starts
        (WHPCR, 0x0600_0500), // below it
        (WHPCR, 0x0100_0200),
    ] {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| {
            s.clut[0][1] = 0x00FF_FFFF;
            s.layers[0].whpcr = whpcr;
            s.layers[0].wvpcr = wvpcr;
        });
        transparent_default(&display);
        let frame = display.frame(ram(&[1; 800 * 600])).unwrap();
        assert!(frame.rgb.iter().all(|&b| b == 0), "{whpcr:#x} {wvpcr:#x}");
    }
}

#[test]
fn a_disabled_layer_with_nothing_to_show_leaves_the_background() {
    // A default colour with alpha 0 and the blending factors as a reset leaves them.
    let display = board(L8, CLUTEN);
    display.update(|s| {
        s.bccr = 0x0001_0203;
        s.layers[0].dccr = 0;
        s.layers[0].bfcr = BF_PIXEL;
    });
    let frame = display.frame(ram(&[])).unwrap();
    assert_eq!(frame.pixel(0, 0), [1, 2, 3]);
    assert_eq!(frame.pixel(799, 599), [1, 2, 3]);
}

#[test]
fn a_disabled_layer_blends_its_default_colour_over_the_whole_active_area() {
    // RM0433 32.4.2 (p. 1212) and 32.7.20 (p. 1226): the default colour is used outside the
    // window and when the layer is disabled, and the blending is done whether it is or not.
    // Here the board's layer 1 is off, with its opaque black default and constant alpha.
    let display = board(L8, CLUTEN);
    display.update(|s| s.bccr = 0x0001_0203);
    let frame = display.frame(ram(&[])).unwrap();
    assert_eq!(frame.pixel(0, 0), [0, 0, 0]);
    assert_eq!(frame.pixel(799, 599), [0, 0, 0]);
    // Half of it, by the pixel alpha of the default colour with the reset factors.
    display.update(|s| {
        s.layers[0].dccr = 0x80FF_FFFF;
        s.layers[0].bfcr = BF_PIXEL;
        s.bccr = 0;
    });
    let frame = display.frame(ram(&[])).unwrap();
    assert_eq!(frame.pixel(400, 300), [128, 128, 128]);
}

#[test]
fn rgb565_is_widened_to_eight_bits() {
    let display = board(RGB565, LEN);
    pitch16(&display);
    let mut memory = vec![0; 1600 * 600];
    memory[..2].copy_from_slice(&0xF800u16.to_le_bytes());
    memory[2..4].copy_from_slice(&0x07E0u16.to_le_bytes());
    memory[4..6].copy_from_slice(&0x001Fu16.to_le_bytes());
    memory[6..8].copy_from_slice(&0x8410u16.to_le_bytes());
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
    assert_eq!(frame.pixel(1, 0), [0, 255, 0]);
    assert_eq!(frame.pixel(2, 0), [0, 0, 255]);
    // 10000 100000 10000: the high bits fill the low ones.
    assert_eq!(frame.pixel(3, 0), [0x84, 0x82, 0x84]);
}

#[test]
fn rgb888_is_stored_blue_first() {
    let display = board(RGB888, LEN);
    display.update(|s| s.layers[0].cfblr = 2400 << 16 | 2407);
    let mut memory = vec![0; 2400 * 600];
    memory[..3].copy_from_slice(&[0x33, 0x22, 0x11]);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
}

#[test]
fn argb8888_is_stored_blue_first_with_the_alpha_last() {
    let display = board(ARGB8888, LEN);
    display.update(|s| s.layers[0].cfblr = 3200 << 16 | 3207);
    let mut memory = vec![0; 3200 * 600];
    memory[..4].copy_from_slice(&[0x33, 0x22, 0x11, 0xFF]);
    // The constant alpha is the blending factor here, so the pixel's own alpha does not
    // count.
    memory[4..8].copy_from_slice(&[0x66, 0x55, 0x44, 0x00]);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
    assert_eq!(frame.pixel(1, 0), [0x44, 0x55, 0x66]);
}

#[test]
fn argb4444_widens_each_nibble_by_repeating_it() {
    let display = board(ARGB4444, LEN);
    pitch16(&display);
    let memory = pixels16(0xF123, 0xFFFF);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
    assert_eq!(frame.pixel(1, 0), [0xFF, 0xFF, 0xFF]);
}

#[test]
fn argb1555_widens_the_colours_like_rgb565_and_the_alpha_to_0_or_255() {
    let display = board(ARGB1555, LEN);
    pitch16(&display);
    let memory = pixels16(0x8000 | 31 << 10 | 16 << 5 | 1, 0);
    let frame = display.frame(ram(&memory)).unwrap();
    // 31 is 255, 16 is 10000 which fills to 10000100, 1 is 00001 which fills to 00001000.
    assert_eq!(frame.pixel(0, 0), [255, 0x84, 0x08]);
}

#[test]
fn the_alpha_of_argb1555_is_all_or_nothing() {
    let display = board(ARGB1555, LEN);
    pitch16(&display);
    display.update(|s| {
        s.bccr = 0x0001_0203;
        s.layers[0].bfcr = BF_PIXEL;
        s.layers[0].dccr = 0;
    });
    let memory = pixels16(0x8000 | 31 << 10, 31 << 10);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
    assert_eq!(frame.pixel(1, 0), [1, 2, 3]);
}

#[test]
fn constant_alpha_blends_the_layer_with_the_background() {
    // What the firmware does to 16 and 32-bit colour: constant alpha 0xE0 over black.
    let display = board(RGB565, LEN);
    pitch16(&display);
    display.update(|s| s.layers[0].cacr = 0xE0);
    let memory = pixels16(0xFFFF, 0xF800);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0xE0, 0xE0, 0xE0]);
    assert_eq!(frame.pixel(1, 0), [0xE0, 0, 0]);
}

// The tests that round pin this model's rounding (to the nearest level), which is my choice
// and not measured on the chip: RM0433 only says the constant alpha is divided by 255.
#[test]
fn blending_rounds_to_the_nearest_level() {
    // 128/255 of 255 over 100 is 128 + 127/255 x 100 = 177.8.
    let display = board(RGB565, LEN);
    pitch16(&display);
    display.update(|s| {
        s.bccr = 0x0064_6464;
        s.layers[0].cacr = 0x80;
        s.layers[0].dccr = 0;
    });
    let memory = pixels16(0xFFFF, 0);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [178, 178, 178]);
    // And 128/255 of 0 over 100 is 49.8.
    assert_eq!(frame.pixel(1, 0), [50, 50, 50]);
}

#[test]
fn pixel_alpha_times_constant_alpha_blends_with_what_is_below() {
    let display = board(ARGB4444, LEN);
    pitch16(&display);
    display.update(|s| {
        s.bccr = 0x0000_0064;
        s.layers[0].bfcr = BF_PIXEL;
        s.layers[0].dccr = 0;
    });
    // Alpha 8 is 136, so 136/255 of the red over 136/255 of the blue below: the blue is
    // 100 x 119/255 = 46.7.
    let memory = pixels16(0x8F00, 0x0F00);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [136, 0, 47]);
    // Alpha 0 shows the background whole.
    assert_eq!(frame.pixel(1, 0), [0, 0, 100]);
}

#[test]
fn the_constant_alpha_scales_the_pixel_alpha() {
    let display = board(ARGB4444, LEN);
    pitch16(&display);
    display.update(|s| {
        s.layers[0].bfcr = BF_PIXEL;
        s.layers[0].cacr = 0x80;
        s.layers[0].dccr = 0;
    });
    // 136 x 128 / 255 = 68.3.
    let memory = pixels16(0x8F00, 0);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [68, 0, 0]);
}

#[test]
fn the_product_of_the_alphas_is_rounded_to_the_nearest_level() {
    // 136 x 129 / 255 is 68.8.
    let display = board(ARGB4444, LEN);
    pitch16(&display);
    display.update(|s| {
        s.layers[0].bfcr = BF_PIXEL;
        s.layers[0].cacr = 0x81;
    });
    let memory = pixels16(0x8F00, 0);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [69, 0, 0]);
}

#[test]
fn layer_2_is_blended_over_layer_1() {
    let display = two_layers(RGB565, ARGB4444);
    display.update(|s| {
        // Layer 2's pixels are the next page along: 800 x 2 bytes on from layer 1's.
        s.layers[0].cfblr = 1600 << 16 | 1607;
        s.layers[1].cfbar = BASE + 1600 * 600;
        s.layers[1].cfblr = 1600 << 16 | 1607;
    });
    let mut memory = vec![0; 2 * 1600 * 600];
    // Layer 1 is red, layer 2 half transparent green, then opaque, then nothing.
    for x in 0..3 {
        memory[2 * x..2 * x + 2].copy_from_slice(&0xF800u16.to_le_bytes());
    }
    let page2 = 1600 * 600;
    memory[page2..page2 + 2].copy_from_slice(&0x80F0u16.to_le_bytes());
    memory[page2 + 2..page2 + 4].copy_from_slice(&0xF0F0u16.to_le_bytes());
    let frame = display.frame(ram(&memory)).unwrap();
    // Alpha 8 of ARGB4444 is 8 x 17 = 136 of 255, so layer 2's green counts 136/255 and the
    // red of layer 1 below it 119/255. Both products come to a whole level (136 x 255 / 255
    // and 119 x 255 / 255), so no rounding is pinned here.
    let alpha = 8 * 17;
    assert_eq!(frame.pixel(0, 0), [255 - alpha, alpha, 0]);
    assert_eq!(frame.pixel(1, 0), [0, 0xFF, 0]);
    assert_eq!(frame.pixel(2, 0), [255, 0, 0]);
}

#[test]
fn layer_2_alone_shows_over_the_background() {
    // The 12-bit depth with page 0 transparent: the background colour shows through.
    let display = two_layers(ARGB4444, ARGB4444);
    pitch16(&display);
    display.update(|s| {
        s.bccr = 0x0010_2030;
        s.layers[0].dccr = 0;
        s.layers[0].bfcr = BF_PIXEL;
    });
    let memory = pixels16(0, 0);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x10, 0x20, 0x30]);
}

#[test]
fn a_layer_that_is_off_with_reserved_blending_factors_adds_nothing() {
    // The firmware's layer 2 after the 12-bit depth: everything zero, blending factors too.
    // RM0433 (p. 1227) reserves those codes and does not say what they do, so the layer is
    // taken to add nothing.
    let display = board(L8, CLUTEN | LEN);
    display.update(|s| {
        s.bccr = 0x0001_0203;
        s.layers[0].dccr = 0;
        s.layers[0].cacr = 0;
        s.layers[0].cr = 0;
        s.layers[1] = Layer {
            bfcr: 0,
            ..Layer::default()
        };
        s.layers[0].bfcr = 0;
    });
    let frame = display.frame(ram(&[])).unwrap();
    assert_eq!(frame.pixel(0, 0), [1, 2, 3]);
    assert_eq!(frame.pixel(799, 599), [1, 2, 3]);
}

#[test]
fn the_default_colour_fills_what_the_window_leaves_and_is_blended_by_its_alpha() {
    let display = board(RGB565, LEN);
    display.update(|s| {
        s.bccr = 0x00FF_FFFF;
        s.layers[0].bfcr = BF_PIXEL;
        s.layers[0].dccr = 0x8010_2030;
        s.layers[0].whpcr = (0xF0 + 1) << 16 | 0xF0;
        s.layers[0].cfblr = 1600 << 16 | 11;
    });
    let memory = pixels16(0xF800, 0xF800);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(1, 0), [255, 0, 0]);
    // Alpha 128 of (0x10, 0x20, 0x30) over white.
    assert_eq!(frame.pixel(2, 0), [135, 143, 151]);
}

#[test]
fn a_layer_that_is_off_with_the_reset_blending_leaves_the_picture_alone() {
    let display = two_layers(RGB565, RGB565);
    pitch16(&display);
    display.update(|s| s.layers[1].cr = 0);
    let memory = pixels16(0xF800, 0xF800);
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [255, 0, 0]);
}

#[test]
fn blending_factors_that_the_firmware_does_not_use_are_reported() {
    // Neither a factor of 0 nor of 1, nor the products of the layer's own alpha, which are
    // other values of BF1 and BF2.
    for bfcr in [0x0000, 0x0405 ^ 0x0100, 0x0405 ^ 0x0001, 0x0707, 0x0506] {
        let display = board(RGB565, LEN);
        display.update(|s| s.layers[0].bfcr = bfcr);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("blending factors")),
            "{bfcr:#x}"
        );
    }
}

#[test]
fn the_blending_factors_can_be_paired_either_way() {
    for (bf1, bf2) in [(4, 5), (4, 7), (6, 5), (6, 7)] {
        let display = board(RGB565, LEN);
        pitch16(&display);
        display.update(|s| s.layers[0].bfcr = bf1 << 8 | bf2);
        assert!(display.frame(ram(&pixels16(0, 0))).is_ok(), "{bf1} {bf2}");
    }
}

#[test]
fn formats_that_are_not_drawn_are_reported() {
    for format in [6, 7] {
        let display = board(format, LEN);
        assert_eq!(
            display.frame(ram(&[])),
            Err(FrameError::Unsupported("pixel format AL44 or AL88")),
            "format {format}"
        );
    }
}

#[test]
fn a_layer_that_is_off_is_not_held_to_its_format() {
    let display = board(7, 0);
    assert!(display.frame(ram(&[])).is_ok());
}

#[test]
fn l8_needs_the_colour_table() {
    let display = board(L8, LEN);
    assert_eq!(
        display.frame(ram(&[])),
        Err(FrameError::Unsupported("L8 without the colour table"))
    );
}

#[test]
fn colour_keying_is_reported_for_either_layer() {
    let display = board(L8, LEN | CLUTEN | COLKEN);
    assert_eq!(
        display.frame(ram(&[])),
        Err(FrameError::Unsupported("colour keying"))
    );
    let display = two_layers(RGB565, RGB565);
    pitch16(&display);
    display.update(|s| s.layers[1].cr |= COLKEN);
    assert_eq!(
        display.frame(ram(&pixels16(0, 0))),
        Err(FrameError::Unsupported("colour keying"))
    );
}

#[test]
fn a_framebuffer_outside_ram_is_reported_at_its_first_missing_line() {
    let display = board(L8, LEN | CLUTEN);
    // Two whole lines are there, the third is not.
    assert_eq!(
        display.frame(ram(&[0; 1600])),
        Err(FrameError::Unreadable(BASE + 1600))
    );
}

#[test]
fn an_active_area_with_no_width_or_no_height_is_not_a_picture() {
    // The end of the active width at or before the back porch, or the same for the height,
    // with a layer on and with none: nothing is scanned out, and nothing panics.
    for (bpcr, awcr) in [
        (BPCR, BPCR),
        (BPCR, BPCR & 0xFFFF_0000 | (AWCR & 0x7FF)),
        (BPCR, AWCR & 0xFFFF_0000 | (BPCR & 0x7FF)),
        (BPCR, 0),
    ] {
        for cr in [LEN | CLUTEN, 0] {
            let display = board(L8, cr);
            display.update(|s| {
                s.bpcr = bpcr;
                s.awcr = awcr;
            });
            assert_eq!(
                display.frame(ram(&vec![0; 800 * 600])),
                Err(FrameError::Empty),
                "{awcr:#x} {cr:#x}"
            );
        }
    }
}

#[test]
fn a_window_wider_than_the_line_the_layer_fetches_is_reported() {
    // CFBLL is the bytes of a line plus 7, which is what the firmware writes; one byte less
    // is not drawn, and neither is the F4's plus 3.
    for (cfbll, drawn) in [(800 + 7, true), (800 + 6, false), (800 + 3, false)] {
        let display = board(L8, LEN | CLUTEN);
        display.update(|s| s.layers[0].cfblr = 800 << 16 | cfbll);
        let frame = display.frame(ram(&vec![0; 800 * 600]));
        if drawn {
            assert!(frame.is_ok(), "{cfbll}");
        } else {
            assert_eq!(
                frame,
                Err(FrameError::Unsupported(
                    "window wider than the fetched line"
                )),
                "{cfbll}"
            );
        }
    }
}

#[test]
fn a_window_is_fetched_from_its_own_first_pixel_and_what_is_before_the_active_area_is_not_shown() {
    // The window starts 2 columns and 2 lines before the active area (WHSTPOS and WVSTPOS are
    // 2 less than the porch plus 1), so the framebuffer's pixel 2 of line 2 is the first
    // one shown. The window is 802 pixels wide and 602 lines.
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.clut[0][1] = 0x0011_2233;
        s.layers[0].whpcr = 0x040F_00EE;
        s.layers[0].wvpcr = 0x026F_0016;
        s.layers[0].cfblr = 800 << 16 | (802 + 7);
        s.layers[0].cfblnr = 602;
    });
    let mut memory = vec![0; 800 * 603];
    memory[0] = 1;
    memory[2] = 1;
    memory[800 * 2] = 1;
    memory[800 * 2 + 2] = 1;
    let frame = display.frame(ram(&memory)).unwrap();
    assert_eq!(frame.pixel(0, 0), [0x11, 0x22, 0x33]);
    assert_eq!(frame.pixel(1, 0), [0, 0, 0]);
    assert_eq!(frame.pixel(0, 1), [0, 0, 0]);
}

#[test]
fn the_layer_fetches_only_the_lines_it_is_told_to() {
    // 3 lines of 800 are in RAM and CFBLNR says 3: the rest of the window is not fetched, so
    // it is not looked for either, and shows what is outside a layer.
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.bccr = 0x0001_0203;
        s.clut[0][1] = 0x00FF_FFFF;
        s.layers[0].cfblnr = 3;
        s.layers[0].dccr = 0;
        s.layers[0].bfcr = BF_PIXEL;
    });
    let frame = display.frame(ram(&vec![1; 800 * 3])).unwrap();
    assert_eq!(frame.pixel(0, 2), [255, 255, 255]);
    assert_eq!(frame.pixel(0, 3), [1, 2, 3]);
    assert_eq!(frame.pixel(799, 599), [1, 2, 3]);
}

#[test]
fn a_window_with_no_width_fetches_nothing() {
    let display = board(L8, LEN | CLUTEN);
    display.update(|s| {
        s.layers[0].whpcr = 0x00EF_00F0;
        s.layers[0].dccr = 0;
        s.layers[0].bfcr = BF_PIXEL;
    });
    assert!(display.frame(ram(&[])).is_ok());
}

#[test]
fn dividing_by_255_rounds_to_the_nearest_for_every_product_there_can_be() {
    // The most a blend adds up is 510 x 255.
    for x in 0..=130_050u32 {
        assert_eq!(div255(x), (f64::from(x) / 255.0).round() as u32, "{x}");
    }
}

// A small xorshift, so the random layers are the same every run.
struct Random(u64);

impl Random {
    fn below(&mut self, n: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % u64::from(n)) as u32
    }
}

// A channel of `bits` bits as 8, the pattern repeated to fill the byte from the top.
fn repeated(value: u32, bits: u32) -> u8 {
    (0..8).fold(0, |byte, i| {
        byte | ((value >> (bits - 1 - i % bits) & 1) as u8) << (7 - i)
    })
}

// What a pixel is by the book, one at a time, from the raw register values and nothing of the
// code that builds a frame: where the window is, whether its line is fetched, the address of
// the pixel, its decoding by PFCR, and the blend, with the sum divided by 255 to the nearest.
// None if a layer's window needs more than the line it fetches.
fn by_the_book(s: &Scanout, memory: &[u8]) -> Option<Vec<u8>> {
    let (h_porch, v_porch) = (i64::from(s.bpcr >> 16 & 0xFFF), i64::from(s.bpcr & 0x7FF));
    let width = i64::from(s.awcr >> 16 & 0xFFF) - h_porch;
    let height = i64::from(s.awcr & 0x7FF) - v_porch;
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let mut c = [(s.bccr >> 16) as u8, (s.bccr >> 8) as u8, s.bccr as u8];
            for (layer, clut) in s.layers.iter().zip(&s.clut) {
                let left = i64::from(layer.whpcr & 0xFFF) - (h_porch + 1);
                let right = i64::from(layer.whpcr >> 16 & 0xFFF) - h_porch;
                let top = i64::from(layer.wvpcr & 0x7FF) - (v_porch + 1);
                let bottom = i64::from(layer.wvpcr >> 16 & 0x7FF) - v_porch;
                let bytes = [4, 3, 2, 2, 2, 1][(layer.pfcr & 7) as usize];
                let inside = layer.cr & 1 != 0
                    && (left..right).contains(&x)
                    && (top..bottom).contains(&y)
                    && y - top < i64::from(layer.cfblnr & 0x7FF);
                let (alpha, colour) = if inside {
                    if (right - left) * bytes > i64::from(layer.cfblr & 0x1FFF) - 7 {
                        return None;
                    }
                    let at = i64::from(layer.cfbar - BASE)
                        + i64::from(layer.cfblr >> 16 & 0x1FFF) * (y - top)
                        + (x - left) * bytes;
                    let raw = &memory[at as usize..(at + bytes) as usize];
                    let v = u32::from(raw[0]) | u32::from(raw.get(1).copied().unwrap_or(0)) << 8;
                    match layer.pfcr & 7 {
                        0 => (raw[3], [raw[2], raw[1], raw[0]]),
                        1 => (255, [raw[2], raw[1], raw[0]]),
                        2 => (
                            255,
                            [
                                repeated(v >> 11, 5),
                                repeated(v >> 5 & 63, 6),
                                repeated(v & 31, 5),
                            ],
                        ),
                        3 => (
                            if v >> 15 == 1 { 255 } else { 0 },
                            [
                                repeated(v >> 10 & 31, 5),
                                repeated(v >> 5 & 31, 5),
                                repeated(v & 31, 5),
                            ],
                        ),
                        4 => (
                            repeated(v >> 12, 4),
                            [
                                repeated(v >> 8 & 15, 4),
                                repeated(v >> 4 & 15, 4),
                                repeated(v & 15, 4),
                            ],
                        ),
                        _ => {
                            let entry = clut[raw[0] as usize];
                            (255, [(entry >> 16) as u8, (entry >> 8) as u8, entry as u8])
                        }
                    }
                } else {
                    (
                        (layer.dccr >> 24) as u8,
                        [
                            (layer.dccr >> 16) as u8,
                            (layer.dccr >> 8) as u8,
                            layer.dccr as u8,
                        ],
                    )
                };
                let constant = u64::from(layer.cacr & 0xFF);
                let times_pixel = (u64::from(alpha) * constant * 2 + 255) / 510;
                let bf1 = if layer.bfcr >> 8 & 7 == 6 {
                    times_pixel
                } else {
                    constant
                };
                let bf2 = 255
                    - if layer.bfcr & 7 == 7 {
                        times_pixel
                    } else {
                        constant
                    };
                for (below, above) in c.iter_mut().zip(colour) {
                    let sum = bf1 * u64::from(above) + bf2 * u64::from(*below);
                    *below = ((sum * 2 + 255) / 510).min(255) as u8;
                }
            }
            rgb.extend_from_slice(&c);
        }
    }
    Some(rgb)
}

#[test]
fn a_frame_is_what_the_pixels_are_by_the_book_whatever_the_layers() {
    let mut random = Random(0x2545_F491_4F6C_DD1D);
    let (h_porch, v_porch) = (4, 3);
    let (mut drawn, mut rejected) = (0, 0);
    for trial in 0..600 {
        let (width, height) = (1 + random.below(23), 1 + random.below(9));
        let memory: Vec<u8> = (0..0x10000).map(|_| random.below(256) as u8).collect();
        let mut layers = [Layer::default(); 2];
        for (n, layer) in layers.iter_mut().enumerate() {
            let format = random.below(6);
            let bytes = [4, 3, 2, 2, 2, 1][format as usize];
            // The whole of the active area two times in five, else a window that starts up to
            // 3 before it and runs up to 2 past it.
            let full = random.below(5) < 2;
            let (left, right, top, bottom) = if full {
                (0, width as i64, 0, height as i64)
            } else {
                let left = random.below(width + 4) as i64 - 3;
                let top = random.below(height + 4) as i64 - 3;
                (
                    left,
                    left + random.below((width as i64 + 3 - left) as u32) as i64,
                    top,
                    top + random.below((height as i64 + 3 - top) as u32) as i64,
                )
            };
            let window_bytes = (right - left) as u32 * bytes;
            let pitch = window_bytes + random.below(9);
            // The line length is enough nearly always, as the firmware writes it, and short now
            // and then.
            let cfbll = if random.below(4) == 0 {
                random.below(window_bytes + 8)
            } else {
                window_bytes + 7 + random.below(5)
            };
            let lines = (bottom - top) as u32;
            *layer = Layer {
                cr: if random.below(10) == 0 { 0 } else { LEN }
                    | if format == L8 { CLUTEN } else { 0 },
                whpcr: ((h_porch as i64 + right) as u32) << 16 | (h_porch as i64 + 1 + left) as u32,
                wvpcr: ((v_porch as i64 + bottom) as u32) << 16 | (v_porch as i64 + 1 + top) as u32,
                pfcr: format,
                cacr: [0, 0x80, 0xE0, 0xFF, 1 + random.below(255)][random.below(5) as usize],
                dccr: random.below(1 << 30) << 2 | random.below(4) << 30,
                bfcr: [0x405, 0x407, 0x605, 0x607][random.below(4) as usize],
                cfbar: BASE + n as u32 * 0x8000,
                cfblr: pitch << 16 | cfbll,
                cfblnr: (lines + random.below(3)).saturating_sub(random.below(2)),
            };
        }
        let display = Display::default();
        display.update(|s| {
            s.enabled = true;
            s.bpcr = h_porch << 16 | v_porch;
            s.awcr = (h_porch + width) << 16 | (v_porch + height);
            s.bccr = random.below(1 << 24);
            s.layers = layers;
            for clut in &mut s.clut {
                *clut = std::array::from_fn(|_| random.below(1 << 24));
            }
        });
        let expected = by_the_book(&display.0.borrow(), &memory);
        match (display.frame(ram(&memory)), expected) {
            (Ok(frame), Some(expected)) => {
                drawn += 1;
                assert!(frame.rgb == expected, "trial {trial}: {layers:?}");
            }
            (Err(FrameError::Unsupported("window wider than the fetched line")), None) => {
                rejected += 1
            }
            (got, expected) => {
                panic!(
                    "trial {trial}: {got:?} against {:?}: {layers:?}",
                    expected.is_some()
                )
            }
        }
    }
    assert!(
        drawn > 100 && rejected > 50,
        "{drawn} drawn, {rejected} rejected"
    );
}

// How long a frame takes to build, for the biggest frame of each depth that the firmware
// makes and for the prompt; the two ARGB4444 layers are drawn (runs of opaque and clear
// pixels) or have random alpha. Run in release:
// cargo test -p cmm2-core --release compositing_speed -- --ignored --nocapture
#[test]
#[ignore = "a timing, not a check: see the comment"]
fn compositing_speed() {
    const PORCH: (u32, u32) = (16, 8);
    let set_up = |width: u32, height: u32, format: u32, layers: usize, random_alpha: bool| {
        let (h, v) = PORCH;
        let display = Display::default();
        let pitch = width * [4, 3, 2, 2, 2, 1][format as usize];
        display.update(|s| {
            s.enabled = true;
            s.bpcr = h << 16 | v;
            s.awcr = (h + width) << 16 | (v + height);
            s.clut[0] = std::array::from_fn(|i| i as u32 * 0x0001_0101);
            for (n, layer) in s.layers.iter_mut().take(layers).enumerate() {
                *layer = Layer {
                    cr: LEN | if format == L8 { CLUTEN } else { 0 },
                    whpcr: (h + width) << 16 | (h + 1),
                    wvpcr: (v + height) << 16 | (v + 1),
                    pfcr: format,
                    cacr: if format == ARGB4444 || format == L8 {
                        0xFF
                    } else {
                        0xE0
                    },
                    dccr: if n == 0 && layers == 1 {
                        0xFF00_0000
                    } else {
                        0
                    },
                    bfcr: if format == ARGB4444 {
                        BF_PIXEL
                    } else {
                        BF_CONSTANT
                    },
                    cfbar: BASE + n as u32 * pitch * height,
                    cfblr: pitch << 16 | (pitch + 7),
                    cfblnr: height,
                };
            }
        });
        // Drawn pixels are opaque or clear, in runs of 64 as drawing leaves them; random ones
        // have every alpha, which is the worst case for the blend.
        let hash = |i: u32| (i.wrapping_mul(2_654_435_761) >> 13) as u16;
        let memory: Vec<u8> = if random_alpha || format != ARGB4444 {
            (0..layers as u32 * pitch * height)
                .map(|i| hash(i) as u8)
                .collect()
        } else {
            (0..layers as u32 * pitch * height / 2)
                .flat_map(|i| {
                    let v = hash(i);
                    (if i / 64 % 4 == 0 { v | 0xF000 } else { 0 }).to_le_bytes()
                })
                .collect()
        };
        (display, memory)
    };
    // The biggest frames this firmware makes in each depth (MODE 16, 11,16, 10,32 and 18,12),
    // which are the worst cases: it has no 1920 x 1080 in the 12-bit depth.
    for (name, width, height, format, layers, random_alpha) in [
        ("1920 x 1080, L8, one layer", 1920, 1080, L8, 1, false),
        ("1280 x 720, RGB565, one layer", 1280, 720, RGB565, 1, false),
        (
            "848 x 480, ARGB8888, one layer",
            848,
            480,
            ARGB8888,
            1,
            false,
        ),
        (
            "1024 x 600, two ARGB4444 layers",
            1024,
            600,
            ARGB4444,
            2,
            false,
        ),
        (
            "1024 x 600, two ARGB4444 layers, random alpha",
            1024,
            600,
            ARGB4444,
            2,
            true,
        ),
        (
            "800 x 600, L8, one layer (the prompt)",
            800,
            600,
            L8,
            1,
            false,
        ),
    ] {
        let (display, memory) = set_up(width, height, format, layers, random_alpha);
        let ram = |addr: u32, len: usize| {
            let start = addr.checked_sub(BASE)? as usize;
            memory.get(start..start + len)
        };
        display.frame(ram).unwrap();
        let best = (0..40)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(display.frame(ram).unwrap());
                start.elapsed().as_secs_f64() * 1000.0
            })
            .fold(f64::MAX, f64::min);
        println!("{name}: {best:.2} ms (best of 40)");
    }
}

#[test]
fn the_ppm_has_a_header_and_the_pixels() {
    let frame = Frame {
        width: 2,
        height: 1,
        rgb: vec![1, 2, 3, 4, 5, 6],
    };
    assert_eq!(frame.to_ppm(), b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06");
}
