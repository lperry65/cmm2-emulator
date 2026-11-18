//! SEL (A7.7.128).

use super::model::encode;
use super::*;

// --- SEL (A7.7.128): Rd byte i = Rn byte i if GE<i> is set, else Rm byte i ---

const SEL: u32 = 0xfaa2_f183; // sel r1, r2, r3

fn sel_with_ge(ge: u32) -> u32 {
    let core = run(
        SEL,
        &[(Reg::R2, 0x4433_2211), (Reg::R3, 0xddcc_bbaa)],
        ge,
        false,
    );
    core.get_r(Reg::R1)
}

#[test]
fn test_sel_for_all_sixteen_ge_patterns() {
    let (rn, rm) = ([0x11, 0x22, 0x33, 0x44], [0xaa, 0xbb, 0xcc, 0xdd]);
    for ge in 0..16u32 {
        let mut expected = 0;
        for byte in 0..4 {
            let pick = if ge >> byte & 1 == 1 {
                rn[byte]
            } else {
                rm[byte]
            };
            expected |= pick << (8 * byte);
        }
        assert_eq!(sel_with_ge(ge), expected, "GE {ge:04b}");
    }
}

#[test]
fn test_sel_leaves_ge_q_and_the_operands_alone() {
    let core = run(
        SEL,
        &[(Reg::R2, 0x4433_2211), (Reg::R3, 0xddcc_bbaa)],
        0b0110,
        true,
    );
    assert_eq!(ge_of(&core), 0b0110);
    assert!(core.psr.get_q());
    assert_eq!(core.get_r(Reg::R2), 0x4433_2211);
    assert_eq!(core.get_r(Reg::R3), 0xddcc_bbaa);
}

#[test]
fn test_sel_picks_by_the_ge_flags_that_uadd8_and_usub8_set() {
    // uadd8 r4, r2, r3: bytes ff + 01 carry (GE set), 10 + 01 do not
    let mut core = Processor::new();
    core.set_r(Reg::R2, 0x10ff_10ff);
    core.set_r(Reg::R3, 0x0101_0101);
    core.execute_internal(&decode_32(encode("uadd8", 4, 2, 3)))
        .unwrap();
    assert_eq!(ge_of(&core), 0b0101);
    core.set_r(Reg::R2, 0x1111_1111);
    core.set_r(Reg::R3, 0xaaaa_aaaa);
    core.execute_internal(&decode_32(SEL)).unwrap();
    assert_eq!(core.get_r(Reg::R1), 0xaa11_aa11);

    // usub8 r4, r2, r3: GE is set where Rn >= Rm, equal bytes included
    let mut core = Processor::new();
    core.set_r(Reg::R2, 0x0305_0807);
    core.set_r(Reg::R3, 0x0406_0807);
    core.execute_internal(&decode_32(encode("usub8", 4, 2, 3)))
        .unwrap();
    assert_eq!(ge_of(&core), 0b0011);
    core.set_r(Reg::R2, 0x1111_1111);
    core.set_r(Reg::R3, 0xaaaa_aaaa);
    core.execute_internal(&decode_32(SEL)).unwrap();
    assert_eq!(core.get_r(Reg::R1), 0xaaaa_1111);
}

#[test]
fn test_sel_picks_by_the_ge_flags_that_msr_apsr_g_writes() {
    // msr apsr_g, r0 is f380 8400; only GE (R0 bits 19:16) is written, and nzcvq is not
    for ge in 0..16u32 {
        let mut core = Processor::new();
        core.set_r(Reg::R0, 0xf800_0000 | ge << 16);
        core.execute_internal(&decode_32(0xf380_8400)).unwrap();
        assert_eq!(ge_of(&core), ge);
        assert_eq!(core.psr.value & 0xf800_0000, 0);
        core.set_r(Reg::R2, 0x4433_2211);
        core.set_r(Reg::R3, 0xddcc_bbaa);
        core.execute_internal(&decode_32(SEL)).unwrap();
        assert_eq!(core.get_r(Reg::R1), sel_with_ge(ge), "GE {ge:04b}");
    }
}
