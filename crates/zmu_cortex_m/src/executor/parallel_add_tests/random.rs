//! Edge and random inputs against the model, in every register layout.

use super::super::dsp_test_support::Rng;
use super::model::{encode, model};
use super::*;

const EDGES: [u32; 20] = [
    0x0000_0000,
    0xffff_ffff,
    0x7f7f_7f7f,
    0x8080_8080,
    0xff01_ff01,
    0x01ff_01ff,
    0x7f80_017f,
    0x80ff_ff80,
    0x7fff_7fff,
    0x8000_8000,
    0x7fff_8000,
    0x8000_7fff,
    0x0001_ffff,
    0xffff_0001,
    0x0001_0001,
    0x00ff_ff00,
    0x0100_00ff,
    0xff00_00ff,
    0x1234_5678,
    0x9abc_def0,
];

// the register layouts of Rd, Rn, Rm: all different, then Rd = Rn, Rd = Rm, Rn = Rm, all the same
const LAYOUTS: [(u32, u32, u32); 5] = [(1, 2, 3), (2, 2, 3), (3, 2, 3), (1, 3, 3), (4, 4, 4)];
/// Runs `name` on `rn` and `rm` in the given layout and compares everything with the model:
/// Rd, GE (preset to `ge`, which only the plain forms change), Q (never changed), and the other
/// flags and registers. One core is reused: every register and flag the case looks at is set
/// first, and a core is slow to make.
fn check(
    core: &mut Processor,
    name: &str,
    layout: (u32, u32, u32),
    rn: u32,
    rm: u32,
    ge: u32,
    q: bool,
) {
    let (rd, rn_number, rm_number) = layout;
    let opcode = encode(name, rd, rn_number, rm_number);
    // with Rn and Rm the same register, both operands are its value
    let rm = if rn_number == rm_number { rn } else { rm };
    for (register, value) in [
        (Reg::R0, 0x1111_1111),
        (Reg::R5, 0x5555_5555),
        (Reg::R12, 0xcccc_cccc),
    ] {
        core.set_r(register, value);
    }
    core.set_r(Reg::from(rn_number as u8), rn);
    core.set_r(Reg::from(rm_number as u8), rm);
    core.psr.value = FLAGS;
    core.psr.value.set_bits(16..20, ge);
    core.psr.set_q(q);
    core.execute_internal(&decode_32(opcode)).unwrap();
    let (expected, expected_ge) = model(name, rn, rm);
    let context = format!("{name} {layout:?} rn {rn:#010x} rm {rm:#010x} ge {ge:#x} q {q}");
    assert_eq!(core.get_r(Reg::from(rd as u8)), expected, "Rd of {context}");
    assert_eq!(ge_of(core), expected_ge.unwrap_or(ge), "GE of {context}");
    assert_eq!(core.psr.get_q(), q, "Q of {context}");
    // nothing else in the PSR moved
    assert_eq!(core.psr.value & !0x080f_0000, FLAGS, "PSR of {context}");
    for (register, value) in [(Reg::R0, 0x1111_1111), (Reg::R5, 0x5555_5555)] {
        assert_eq!(core.get_r(register), value, "{register:?} of {context}");
    }
}

#[test]
fn test_the_edge_values_match_the_model_in_every_layout() {
    let mut core = Processor::new();
    for (name, _) in OPCODES {
        for layout in LAYOUTS {
            for rn in EDGES {
                for rm in EDGES {
                    check(&mut core, name, layout, rn, rm, 0b1010, false);
                }
            }
        }
    }
}

#[test]
fn test_random_values_match_the_model() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut core = Processor::new();
    for (name, _) in OPCODES {
        for case in 0..3000 {
            let (rn, rm) = (rng.next(), rng.next());
            let layout = LAYOUTS[case % LAYOUTS.len()];
            // a preset GE pattern and Q that the instruction must keep or overwrite as it says
            check(
                &mut core,
                name,
                layout,
                rn,
                rm,
                rng.next() & 0xf,
                rng.next() & 1 == 1,
            );
        }
    }
}
