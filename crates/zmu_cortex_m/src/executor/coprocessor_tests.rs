//! The coprocessor instructions (A5.3.18, Table A5-30, A7.7.22, .39, .40, .72, .73, .80, .81,
//! .158): the Cortex-M7 has CP10 and CP11, the floating-point unit, and no other coprocessor, so
//! an instruction of any of the 14 kinds for another number takes the `UsageFault` with UFSR.NOCP
//! (CFSR bit 19) and does nothing else.
//!
//! The opcodes are written from the diagrams and checked against llvm-mc (assembled for p7 and
//! given each number in bits 11:8).

use super::Executor;
#[cfg(feature = "has-fp")]
use super::ExecutorHelper;
use crate::Processor;
use crate::bus::Bus;
use crate::core::instruction::Instruction;
use crate::core::register::{Apsr, BaseReg, Epsr, Ipsr, Reg};
use crate::decoder::{decode_16, decode_32};

const RAM: u32 = 0x2000_0000;
const NOCP: u32 = 1 << 19;
const UNDEFINSTR: u32 = 1 << 16;
const USAGE_FAULT: usize = 6;

/// The 18 forms, assembled for p7 with Rn = r2, Rt = r1 (llvm-mc), and their names
const FORMS: [(u32, &str); 18] = [
    (0xee22_1793, "mcr"),
    (0xfe22_1793, "mcr2"),
    (0xee32_1793, "mrc"),
    (0xfe32_1793, "mrc2"),
    (0xee32_f793, "mrc to apsr_nzcv"),
    (0xec42_1713, "mcrr"),
    (0xfc42_1713, "mcrr2"),
    (0xec52_1713, "mrrc"),
    (0xfc52_1713, "mrrc2"),
    (0xed92_1702, "ldc"),
    (0xfd92_1702, "ldc2"),
    (0xedf2_1702, "ldcl with write back"),
    (0xed82_1702, "stc"),
    (0xfc22_1702, "stc2 post-indexed"),
    (0xede2_1702, "stcl with write back"),
    (0xee12_1783, "cdp"),
    (0xfe12_1783, "cdp2"),
    (0xed9f_1702, "ldc literal"),
];

fn for_coprocessor(opcode: u32, coproc: u32) -> u32 {
    opcode & !0x0f00 | coproc << 8
}

/// A core with the FPU off (the reset state) and registers and memory worth keeping
fn core() -> Processor {
    let mut core = Processor::new();
    core.set_msp(RAM + 0x800);
    for number in 0..13u8 {
        core.set_r(
            Reg::from(number),
            0x1000_0000 + u32::from(number) * 0x0101_0101,
        );
    }
    core.set_r(Reg::R2, RAM + 0x100);
    core.write32(RAM + 0x100, 0xcafe_f00d).unwrap();
    core.write32(RAM + 0xf8, 0xcafe_f00d).unwrap();
    core.write32(RAM + 0x108, 0xcafe_f00d).unwrap();
    core.set_pc(0x100);
    core.psr.set_t(true);
    // SHCSR.USGFAULTENA, so that the UsageFault is taken as itself and not as a HardFault
    core.write32(0xE000_ED24, 1 << 18).unwrap();
    core
}

fn registers(core: &Processor) -> Vec<u32> {
    (0..13)
        .map(|number| core.get_r(Reg::from(number as u8)))
        .collect()
}

fn memory(core: &mut Processor) -> Vec<u32> {
    (0..0x80)
        .map(|word| core.read32(RAM + 0xc0 + word * 4).unwrap())
        .collect()
}

#[test]
fn test_every_form_for_every_absent_coprocessor_takes_a_nocp_usage_fault() {
    let mut runs = 0;
    for (opcode, name) in FORMS {
        for coproc in (0..10).chain(12..16) {
            let opcode = for_coprocessor(opcode, coproc);
            let mut core = core();
            let (registers_before, memory_before) = (registers(&core), memory(&mut core));
            core.execute(&decode_32(opcode), 4);
            let context = format!("{name} p{coproc} {opcode:#010x}");
            assert_eq!(core.cfsr & NOCP, NOCP, "NOCP of {context}");
            assert_eq!(core.cfsr & UNDEFINSTR, 0, "UNDEFINSTR of {context}");
            assert_eq!(
                core.psr.get_isr_number(),
                USAGE_FAULT,
                "the exception of {context}"
            );
            // the frame of the exception is stacked below the old stack pointer; the registers
            // and the memory of the instruction are as they were
            assert_eq!(registers(&core), registers_before, "registers of {context}");
            assert_eq!(memory(&mut core), memory_before, "memory of {context}");
            runs += 1;
        }
    }
    assert_eq!(runs, 18 * 14);
}

#[test]
fn test_the_fault_is_the_same_whatever_the_operands_say() {
    // the operand fields are the coprocessor's: all 256 values of hw2<7:0> and of the opcode
    // field of CDP raise the same fault for an absent coprocessor
    for low in 0..=0xff_u32 {
        let mut core = core();
        core.execute(&decode_32(0xee12_1700 | low & !0x10), 4); // cdp p7 (bit 4 clear)
        assert_eq!(core.cfsr & NOCP, NOCP, "{low:#x}");
        let mut core = self::core();
        core.execute(&decode_32(0xee22_1710 | low & !0x10 | 0x10), 4); // mcr p7 (bit 4 set)
        assert_eq!(core.cfsr & NOCP, NOCP, "{low:#x}");
    }
}

#[test]
fn test_in_an_it_block_that_does_not_pass_there_is_no_fault() {
    for (opcode, name) in FORMS {
        let mut core = core();
        core.psr.set_z_bit(false);
        // `it eq` with Z clear
        core.execute(&decode_16(0xbf08), 2);
        core.execute(&decode_32(opcode), 4);
        assert_eq!(core.cfsr, 0, "{name}");
        assert_eq!(core.psr.get_isr_number(), 0, "{name}");
    }
}

/// Coprocessor instructions for CP10 or CP11 that are not instructions of the FPU: a move to and
/// from a core register with operands that no VMOV or VMRS has, the two-register moves with an
/// opc1 that VMOV does not use, a load and a store that are not VLDR, VSTR or VLDM (P = 0,
/// U = 0 and W = 1 is no FP form), and VMLA, VMLS, VNMLA and VNMLS (llvm-mc: vmla.f32 s0, s1,
/// s2 and so on), single and double. Only the first group is new behaviour of the batch: VNMLA
/// and VNMLS were already UDF, VMLA and VMLS were swallowed by the MCR entry and ran as a silent
/// no-op. Until the floating-point batch adds all four they are not FP instructions.
const NOT_FP: [(u32, &str); 14] = [
    (0xee22_1a93, "mcr p10"),
    (0xee32_1b93, "mrc p11"),
    (0xec42_1a03, "mcrr p10"),
    (0xec52_1b03, "mrrc p11"),
    // odd imm8 is not a VLDM or VSTM of doubles (it is FLDMX and FSTMX, which no FP instruction
    // here is); the even ones are in the VLDM and VSTM patterns, which refuse P = 0, U = 0 as UDF
    (0xec32_1b03, "ldc p11"),
    (0xec22_1b03, "stc p11"),
    (0xee00_0a81, "vmla.f32"),
    (0xee01_0b02, "vmla.f64"),
    (0xee00_0ac1, "vmls.f32"),
    (0xee01_0b42, "vmls.f64"),
    (0xee10_0ac1, "vnmla.f32"),
    (0xee11_0b42, "vnmla.f64"),
    (0xee10_0a81, "vnmls.f32"),
    (0xee11_0b02, "vnmls.f64"),
];

/// CP10 and CP11 exist when the FPU is built in, so an instruction for them that is not the FPU's
/// is UNDEFINED with the FPU enabled (UFSR.UNDEFINSTR), and NOCP with it disabled (A2.6, A5.3.18).
/// Without the FPU they are absent coprocessors: NOCP.
#[test]
fn test_a_non_fp_encoding_for_cp10_or_cp11_is_undefined_enabled_and_nocp_disabled() {
    for (opcode, name) in NOT_FP {
        // never a silent no-op, and never an FP instruction
        assert!(
            matches!(
                decode_32(opcode),
                Instruction::COPROC { .. } | Instruction::UDF { .. }
            ),
            "{name}: {:?}",
            decode_32(opcode)
        );
        let mut off = core();
        off.execute(&decode_32(opcode), 4);
        assert_eq!(off.cfsr, NOCP, "{name} with the FPU off");
        assert_eq!(off.psr.get_isr_number(), USAGE_FAULT, "{name}");
        #[cfg(feature = "has-fp")]
        {
            let mut on = core();
            on.write32(0xE000_ED88, 0x00f0_0000).unwrap(); // CPACR: CP10 and CP11 full access
            on.execute(&decode_32(opcode), 4);
            assert_eq!(on.cfsr, UNDEFINSTR, "{name} with the FPU on");
            assert_eq!(on.psr.get_isr_number(), USAGE_FAULT, "{name}");
        }
    }
}

#[test]
#[cfg(feature = "has-fp")]
fn test_the_fp_instructions_that_share_the_encodings_still_run() {
    // llvm-mc: vmov s0, r0 / vmov r1, s2 / vmrs r1, fpscr / vmsr fpscr, r1 (the move forms of
    // MCR and MRC), vldr s0, [r2] / vstr s0, [r2] (LDC and STC), vldmia r2, {s0, s1} and
    // vmov r1, r2, d0 and vmov d0, r1, r2 (MRRC and MCRR), vadd.f32 s0, s1, s2 (CDP)
    let mut core = core();
    core.write32(0xE000_ED88, 0x00f0_0000).unwrap();
    for opcode in [
        0xee00_1a10_u32, // vmov s0, r1
        0xee11_2a10,     // vmov r2, s2
        0xeef1_1a10,     // vmrs r1, fpscr
        0xeee1_1a10,     // vmsr fpscr, r1
        0xed92_0a00,     // vldr s0, [r2]
        0xed82_0a00,     // vstr s0, [r2]
        0xec92_0a02,     // vldmia r2, {s0, s1}
        0xec52_1b10,     // vmov r1, r2, d0
        0xec42_1b10,     // vmov d0, r1, r2
        0xee30_0a81,     // vadd.f32 s0, s1, s2
    ] {
        let instruction = decode_32(opcode);
        assert!(
            !matches!(
                instruction,
                Instruction::UDF { .. } | Instruction::COPROC { .. }
            ),
            "{opcode:#010x}: {instruction:?}"
        );
        core.cfsr = 0;
        core.execute_internal(&instruction)
            .unwrap_or_else(|fault| panic!("{opcode:#010x} faulted with {fault:?}"));
    }
    assert_eq!(core.cfsr, 0);
}
