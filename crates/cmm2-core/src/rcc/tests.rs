use super::*;
use crate::{MemoryMap, ResetCause};

const CR: u32 = RCC_BASE;
const CFGR: u32 = RCC_BASE + 0x10;
const BDCR: u32 = RCC_BASE + 0x70;
const CSR: u32 = RCC_BASE + 0x74;
const PLL1DIVR: u32 = RCC_BASE + 0x30;
const AHB4ENR: u32 = RCC_BASE + 0xE0;

// (name, on bit, ready bit) of the RCC_CR pairs that follow.
const PAIRS: [(&str, u32, u32); 7] = [
    ("HSI", 1 << 0, 1 << 2),
    ("CSI", 1 << 7, 1 << 8),
    ("RC48", 1 << 12, 1 << 13),
    ("HSE", 1 << 16, 1 << 17),
    ("PLL1", 1 << 24, 1 << 25),
    ("PLL2", 1 << 26, 1 << 27),
    ("PLL3", 1 << 28, 1 << 29),
];
// Always set: D1CKRDY, D2CKRDY and HSIDIVF.
const ALWAYS_READY: u32 = 1 << 14 | 1 << 15 | 1 << 5;
const ON_BITS: u32 = 0x1501_1001;
const PLL3_ON: u32 = 1 << 28;
const ALL_PLLS_ON: u32 = 0x1500_0000;
const BOARD_CR: u32 = 0x3F03_F025;

fn machine() -> MemoryMap {
    let mut m = MemoryMap::new();
    m.map_peripheral(RCC_BASE, RCC_SIZE, Box::new(Rcc::new(Clock::new())))
        .unwrap();
    m
}

#[test]
fn each_ready_bit_follows_its_on_bit_both_ways() {
    for (name, on, ready) in PAIRS {
        let mut m = machine();
        m.write32(CR, 0);
        m.write32(CR, on);
        assert_eq!(m.read32(CR) & ready, ready, "{name} set");
        m.write32(CR, 0);
        assert_eq!(m.read32(CR) & ready, 0, "{name} cleared");
    }
}

#[test]
fn domain_clocks_and_hsidivf_always_read_set() {
    let mut m = machine();
    m.write32(CR, 0);
    assert_eq!(m.read32(CR), ALWAYS_READY);
}

#[test]
fn writes_to_ready_bits_do_nothing() {
    let mut m = machine();
    let on: u32 = PAIRS.iter().map(|p| p.1).sum();
    let ready: u32 = PAIRS.iter().map(|p| p.2).sum();
    m.write32(CR, !on);
    assert_eq!(m.read32(CR) & ready, 0);
}

// The reset value 0x83 has HSION, HSIKERON and CSION set.
#[test]
fn unwritten_cr_reads_the_reset_value_with_its_ready_bits() {
    let mut m = machine();
    assert_eq!(m.read32(CR), 0x0000_C1A7);
}

#[test]
fn the_board_value_of_cr_comes_from_its_on_bits() {
    let mut m = machine();
    m.write32(CR, ON_BITS);
    assert_eq!(m.read32(CR), BOARD_CR);
}

#[test]
fn cfgr_sws_follows_sw() {
    let mut m = machine();
    for sw in 0..=3 {
        m.write32(CFGR, sw);
        assert_eq!(m.read32(CFGR), sw | sw << 3, "SW {sw}");
    }
}

#[test]
fn cfgr_stores_the_other_fields() {
    let mut m = machine();
    m.write32(CFGR, 0xFFFF_FFC3);
    assert_eq!(m.read32(CFGR), 0xFFFF_FFDB);
}

#[test]
fn cfgr_byte_write_to_the_low_byte() {
    let mut m = machine();
    m.write32(CFGR, 0x0000_0100);
    m.write8(CFGR, 3);
    assert_eq!(m.read32(CFGR), 0x0000_011B);
}

#[test]
fn bdcr_lserdy_follows_lseon_and_the_rest_is_stored() {
    let mut m = machine();
    m.write32(BDCR, 0x0000_8111);
    assert_eq!(m.read32(BDCR), 0x0000_8113);
    m.write32(BDCR, 0x0000_8110);
    assert_eq!(m.read32(BDCR), 0x0000_8110);
    m.write32(BDCR, 0x2);
    assert_eq!(m.read32(BDCR), 0);
}

#[test]
fn csr_lsirdy_follows_lsion() {
    let mut m = machine();
    m.write32(CSR, 1);
    assert_eq!(m.read32(CSR), 3);
    m.write32(CSR, 0);
    assert_eq!(m.read32(CSR), 0);
    m.write32(CSR, 2);
    assert_eq!(m.read32(CSR), 0);
}

#[test]
fn read_modify_write_of_cr_brings_pll1_up_and_down() {
    let (hse_on, hse_rdy) = (1 << 16, 1 << 17);
    let (pll1_on, pll1_rdy) = (1 << 24, 1 << 25);
    let mut m = machine();
    let cr = m.read32(CR);
    m.write32(CR, cr | hse_on);
    assert_eq!(m.read32(CR) & hse_rdy, hse_rdy);
    let cr = m.read32(CR);
    m.write32(CR, cr | pll1_on);
    let cr = m.read32(CR);
    assert_eq!(cr & pll1_rdy, pll1_rdy);
    assert_eq!(cr & hse_rdy, hse_rdy);
    m.write32(CR, cr & !pll1_on);
    let cr = m.read32(CR);
    assert_eq!(cr & pll1_rdy, 0);
    assert_eq!(cr & hse_rdy, hse_rdy);
}

#[test]
fn ordinary_registers_store_writes_and_start_at_their_reset_values() {
    let mut m = machine();
    assert_eq!(m.read32(PLL1DIVR), 0x0101_0280);
    m.write32(PLL1DIVR, 0x0102_03FF);
    assert_eq!(m.read32(PLL1DIVR), 0x0102_03FF);
    m.write16(AHB4ENR, 0x00FF);
    assert_eq!(m.read32(AHB4ENR), 0x00FF);
}

#[test]
fn window_ends_at_rcc_size() {
    let mut m = machine();
    m.write32(RCC_BASE + RCC_SIZE - 4, 5);
    assert_eq!(m.read32(RCC_BASE + RCC_SIZE - 4), 5);
    assert!(m.take_unmapped().is_empty());
}

const RSR: u32 = RCC_BASE + 0xD0;
const POWER_ON_FLAGS: u32 = 0x00E0_0000;
const SOFTWARE_FLAGS: u32 = 0x0140_0000;

#[test]
fn reset_puts_registers_back_but_keeps_bdcr() {
    let mut m = machine();
    m.write32(CR, ON_BITS);
    m.write32(PLL1DIVR, 0x0102_03FF);
    m.write32(BDCR, 0x0000_8111);
    m.reset_peripherals(ResetCause::Software);
    assert_eq!(m.read32(CR), 0x0000_C1A7);
    assert_eq!(m.read32(PLL1DIVR), 0x0101_0280);
    assert_eq!(m.read32(BDCR), 0x0000_8113);
}

#[test]
fn rsr_reads_the_power_on_flags_at_construction() {
    let mut m = machine();
    assert_eq!(m.read32(RSR), POWER_ON_FLAGS);
}

#[test]
fn rsr_says_software_reset_after_one() {
    let mut m = machine();
    m.reset_peripherals(ResetCause::Software);
    assert_eq!(m.read32(RSR), SOFTWARE_FLAGS);
    m.reset_peripherals(ResetCause::PowerOn);
    assert_eq!(m.read32(RSR), POWER_ON_FLAGS);
}

#[test]
fn rsr_flags_clear_with_rmvf_and_nothing_else_writes_them() {
    let mut m = machine();
    m.write32(RSR, !RMVF);
    assert_eq!(m.read32(RSR), POWER_ON_FLAGS);
    m.write32(RSR, 1 << 16);
    assert_eq!(m.read32(RSR), 0);
    m.reset_peripherals(ResetCause::Software);
    m.write32(RSR, 1 << 16 | 1 << 24);
    assert_eq!(m.read32(RSR), 0);
}

// The registers the firmware writes to bring up the board's pixel clock: the crystal as
// the source and DIVM3 of 1, PLL3 on with its R output enabled and the VCO in the wide
// range, and the dividers of the video mode.
fn program_pll3(m: &mut MemoryMap, divr: u32) {
    m.write32(RCC_BASE + 0x28, 0x0010_1012); // PLLCKSELR
    m.write32(RCC_BASE + 0x40, divr); // PLL3DIVR
    m.write32(RCC_BASE + 0x2C, 0x013F_0CDD); // PLLCFGR
    m.write32(CR, ON_BITS);
}

fn machine_and_pixel_clock() -> (MemoryMap, PixelClock) {
    let rcc = Rcc::new(Clock::new());
    let pixel_clock = rcc.pixel_clock();
    let mut m = MemoryMap::new();
    m.map_peripheral(RCC_BASE, RCC_SIZE, Box::new(rcc)).unwrap();
    (m, pixel_clock)
}

#[test]
fn there_is_no_pixel_clock_until_pll3_is_set_up() {
    let (_, pixel_clock) = machine_and_pixel_clock();
    assert_eq!(pixel_clock.hz(), 0);
}

#[test]
fn the_pixel_clock_of_800_x_600_is_49_5_mhz() {
    // 8 MHz x 99 = 792 MHz, divided by 16.
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x0F0F_1E62);
    assert_eq!(pixel_clock.hz(), 49_500_000);
}

#[test]
fn the_pixel_clock_of_640_x_480_is_31_4_mhz() {
    // 8 MHz x 102 = 816 MHz, divided by 26.
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x1919_3265);
    assert_eq!(pixel_clock.hz(), 31_384_615);
}

#[test]
fn the_pixel_clock_follows_each_register_it_depends_on() {
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x0F0F_1E62);
    // DIVM3 of 2 halves it, with the reference range for 4 MHz (PLL3RGE of 2). The settings
    // are written with PLL3 off, as the chip wants.
    m.write32(CR, ON_BITS & !PLL3_ON);
    m.write32(RCC_BASE + 0x2C, 0x013F_08DD);
    m.write32(RCC_BASE + 0x28, 0x0020_1012);
    m.write32(CR, ON_BITS);
    assert_eq!(pixel_clock.hz(), 24_750_000);
    m.write32(CR, ON_BITS & !PLL3_ON);
    m.write32(RCC_BASE + 0x28, 0x0010_1012);
    m.write32(RCC_BASE + 0x2C, 0x013F_0CDD);
    m.write32(CR, ON_BITS);
    assert_eq!(pixel_clock.hz(), 49_500_000);
    // PLL3 off stops it, on starts it again.
    m.write32(CR, ON_BITS & !(1 << 28));
    assert_eq!(pixel_clock.hz(), 0);
    m.write32(CR, ON_BITS);
    assert_eq!(pixel_clock.hz(), 49_500_000);
    // So does the R output being disabled.
    m.write32(CR, ON_BITS & !PLL3_ON);
    m.write32(RCC_BASE + 0x2C, 0x003F_0CDD);
    m.write32(CR, ON_BITS);
    assert_eq!(pixel_clock.hz(), 0);
}

#[test]
fn the_fraction_adds_to_the_multiplier_only_when_it_is_on() {
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x0F0F_1E62);
    // FRACN3 of 4096 is a half: 99.5 x 8 MHz = 796 MHz.
    m.write32(RCC_BASE + 0x44, 4096 << 3);
    assert_eq!(pixel_clock.hz(), 49_500_000);
    m.write32(RCC_BASE + 0x2C, 0x013F_0DDD);
    assert_eq!(pixel_clock.hz(), 49_750_000);
}

#[test]
fn a_pll_that_is_not_fed_from_the_crystal_gives_no_pixel_clock() {
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x0F0F_1E62);
    m.write32(CR, ON_BITS & !ALL_PLLS_ON);
    for source in [0, 1, 3] {
        m.write32(RCC_BASE + 0x28, 0x0010_1010 | source);
        m.write32(CR, ON_BITS);
        assert_eq!(pixel_clock.hz(), 0, "source {source}");
        m.write32(CR, ON_BITS & !ALL_PLLS_ON);
    }
    // And a DIVM3 of 0 switches the PLL off.
    m.write32(RCC_BASE + 0x28, 0x0000_1012);
    m.write32(CR, ON_BITS);
    assert_eq!(pixel_clock.hz(), 0);
}

// PLL3 with the crystal as its source, DIVM3 of `divm`, a VCO of `divn` x the reference (the
// register holds DIVN3 = divn - 1), and R undivided. `rge` is PLL3RGE, bits 11:10 of PLLCFGR
// (3 is the firmware's: 8 to 16 MHz), and `medium_vco` is bit 9 (PLL3VCOSEL).
fn pll3_locks_with(divm: u32, divn: u32, rge: u32, medium_vco: bool) -> (bool, u64) {
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    m.write32(RCC_BASE + 0x28, divm << 20 | 0x1012);
    m.write32(RCC_BASE + 0x40, divn - 1);
    m.write32(
        RCC_BASE + 0x2C,
        0x013F_00DD | rge << 10 | u32::from(medium_vco) << 9,
    );
    m.write32(CR, ON_BITS);
    let ready = m.read32(CR) & 1 << 29 != 0;
    (ready, pixel_clock.hz())
}

#[test]
fn the_reference_is_divided_only_after_the_vco_is_worked_out() {
    // 8 MHz / 3 x 300 is 800 MHz, whole; dividing 8 MHz by 3 first loses 200 Hz of it.
    assert_eq!(pll3_locks_with(3, 300, 1, false), (true, 800_000_000));
}

#[test]
fn the_firmwares_own_pll3_settings_lock() {
    // DIVM3 of 1, PLL3RGE of 3 and the wide VCO, with the DIVN3 of every mode of docs/video.md.
    for divn in [99, 102, 104, 103, 89, 101, 81, 93] {
        assert!(pll3_locks_with(1, divn, 3, false).0, "{divn}");
    }
}

#[test]
fn pll3_only_locks_for_a_reference_and_a_vco_in_the_ranges_of_the_chip() {
    // RM0433 8.5.5 (PLL block diagram and text, pp. 346 to 348) and 8.7.17 (p. 410): the
    // reference is 1 to 16 MHz, VCOH (PLL3VCOSEL clear) takes a reference of 2 to 16 MHz and
    // makes 192 to 960 MHz, VCOL (set) takes 1 to 2 MHz and makes 150 to 420 MHz. 8 MHz /
    // DIVM3 x DIVN3 is the VCO. PLL3RGE is not a condition (see the next test).
    for (divm, divn, rge, medium, locks) in [
        (1, 24, 3, false, true),   // 192 MHz
        (1, 23, 3, false, false),  // 184
        (1, 120, 3, false, true),  // 960
        (1, 121, 3, false, false), // 968
        (1, 105, 3, false, true),  // 840, over what the datasheet gives but in the manual's
        (1, 19, 3, true, false),   // the medium VCO is for a reference of 2 MHz or less
        (4, 75, 1, true, true),    // 2 MHz x 75 = 150 in the medium range
        (4, 74, 1, true, false),   // 148
        (4, 210, 1, true, true),   // 420
        (4, 211, 1, true, false),  // 422
        (4, 80, 1, true, true),    // 2 MHz is in both references
        (8, 150, 0, true, true),   // 1 MHz x 150 = 150
        (8, 149, 0, true, false),  // 149
        (8, 200, 0, false, false), // 1 MHz is not for the wide VCO
        (9, 216, 0, true, false),  // 889 kHz is below the least reference
        (4, 100, 1, false, true),  // 2 MHz is in the wide VCO's range, 200 MHz
        (0, 100, 3, false, false), // DIVM3 of 0 is the PLL off
        (1, 1, 3, false, false),   // DIVN3 of 0, which is below the least the chip takes
        (1, 512, 3, false, false), // 4096 MHz
    ] {
        let (ready, hz) = pll3_locks_with(divm, divn, rge, medium);
        assert_eq!(ready, locks, "{divm} {divn} {rge} {medium}");
        assert_eq!(hz != 0, locks, "{divm} {divn} {rge} {medium}: {hz}");
    }
}

#[test]
fn pll3rge_does_not_decide_whether_pll3_locks() {
    // RM0433 (p. 348, 8.7.12 p. 401) has PLL3RGE set to the reference's range for optimal
    // performance, not that the PLL fails to lock with another one.
    for rge in 0..4 {
        assert_eq!(
            pll3_locks_with(1, 99, rge, false),
            (true, 792_000_000),
            "{rge}"
        );
    }
}

// The registers as the firmware's video mode writes them, then `more`, with PLL3 on.
fn pll3_on(more: impl FnOnce(&mut MemoryMap)) -> (MemoryMap, PixelClock) {
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x0F0F_1E62);
    more(&mut m);
    (m, pixel_clock)
}

#[test]
fn the_fraction_is_latched_when_fracen_goes_from_0_to_1() {
    // RM0433 8.5.5 (p. 347 and 348): the modulator works from SH_REG, which takes FRACN3 when
    // PLL3FRACEN goes 0 to 1, and keeps it when FRACEN goes back to 0.
    let (mut m, pixel_clock) = pll3_on(|_| {});
    // FRACEN is clear in the firmware's PLLCFGR (0x013F0CDD has bit 8 clear): nothing latched.
    m.write32(RCC_BASE + 0x44, 4096 << 3);
    assert_eq!(pixel_clock.hz(), 49_500_000);
    m.write32(RCC_BASE + 0x2C, 0x013F_0DDD); // the latch
    assert_eq!(pixel_clock.hz(), 49_750_000);
    // A new FRACN3 with FRACEN still set is not taken.
    m.write32(RCC_BASE + 0x44, 0);
    assert_eq!(pixel_clock.hz(), 49_750_000);
    // FRACEN clear keeps what was latched; setting it again takes the new one.
    m.write32(RCC_BASE + 0x2C, 0x013F_0CDD);
    assert_eq!(pixel_clock.hz(), 49_750_000);
    m.write32(RCC_BASE + 0x2C, 0x013F_0DDD);
    assert_eq!(pixel_clock.hz(), 49_500_000);
}

#[test]
fn a_reset_clears_the_latched_fraction() {
    let (mut m, pixel_clock) = pll3_on(|m| {
        m.write32(RCC_BASE + 0x44, 4096 << 3);
        m.write32(RCC_BASE + 0x2C, 0x013F_0DDD);
    });
    assert_eq!(pixel_clock.hz(), 49_750_000);
    m.reset_peripherals(ResetCause::Software);
    program_pll3(&mut m, 0x0F0F_1E62);
    assert_eq!(pixel_clock.hz(), 49_500_000);
}

#[test]
fn the_settings_that_the_manual_locks_while_pll3_is_on_are_not_written() {
    // RM0433 8.7.11 (p. 398), 8.7.12 (p. 401) and 8.7.17 (p. 409): DIVM3, DIVN3 and the
    // dividers, PLL3RGE, PLL3VCOSEL and DIVR3EN can be written only with PLL3 off.
    let (mut m, pixel_clock) = pll3_on(|_| {});
    m.write32(RCC_BASE + 0x40, 0x0F0F_1E00 | 50);
    m.write32(RCC_BASE + 0x28, 0x0020_1012);
    m.write32(RCC_BASE + 0x2C, 0x003F_00DD);
    assert_eq!(pixel_clock.hz(), 49_500_000);
    assert_eq!(m.read32(RCC_BASE + 0x40), 0x0F0F_1E62);
    assert_eq!(m.read32(RCC_BASE + 0x28), 0x0010_1012);
    assert_eq!(m.read32(RCC_BASE + 0x2C), 0x013F_0CDD);
    // The same with PLL3 off, and the clock follows.
    m.write32(CR, ON_BITS & !PLL3_ON);
    m.write32(RCC_BASE + 0x40, 0x0F0F_1E00 | 50);
    assert_eq!(m.read32(RCC_BASE + 0x40), 0x0F0F_1E32);
    m.write32(CR, ON_BITS);
    assert_eq!(pixel_clock.hz(), 25_500_000);
}

#[test]
fn the_source_cannot_be_changed_while_a_pll_is_on() {
    // RM0433 8.5.5 (p. 347): not possible to change PLLSRC when one of the PLLs is on.
    let (mut m, _) = pll3_on(|_| {});
    m.write32(RCC_BASE + 0x28, 0x0010_1010);
    assert_eq!(m.read32(RCC_BASE + 0x28) & 3, 2);
    m.write32(CR, ON_BITS & !ALL_PLLS_ON);
    m.write32(RCC_BASE + 0x28, 0x0010_1010);
    assert_eq!(m.read32(RCC_BASE + 0x28) & 3, 0);
}

#[test]
fn every_change_of_the_pixel_clock_is_logged_with_its_cycle_and_asks_for_a_tick() {
    let clock = Clock::new();
    let pixel_clock = PixelClock::new(clock.clone());
    clock.set(500);
    pixel_clock.set(1000);
    assert_eq!(pixel_clock.hz(), 1000);
    assert!(clock.take_due());
    // No change, nothing logged; a stop and a restart at the same rate are two changes.
    clock.set(900);
    pixel_clock.set(1000);
    assert!(!clock.take_due());
    pixel_clock.set(0);
    clock.set(950);
    pixel_clock.set(1000);
    assert_eq!(
        pixel_clock.take_changes(),
        [(500, 1000), (900, 0), (950, 1000)]
    );
    assert!(pixel_clock.take_changes().is_empty());
}

#[test]
fn a_reset_stops_the_pixel_clock() {
    let (mut m, pixel_clock) = machine_and_pixel_clock();
    program_pll3(&mut m, 0x0F0F_1E62);
    m.reset_peripherals(ResetCause::Software);
    assert_eq!(pixel_clock.hz(), 0);
}

#[test]
fn a_fresh_rcc_gives_the_rng_the_state_that_the_rng_takes_at_reset() {
    // The RNG assumes this after a system reset, as the RCC is reset before it (rng.rs).
    let rcc = Rcc::new(Clock::new());
    assert_eq!(rcc.rng_clock().get(), RngClockState::RESET);
}
