//! Addresses and register values are modulo 2^32: an access that runs over the top of the address
//! space carries on at 0, and an offset that goes below 0 comes out at the top. None of that may
//! panic the host. A device at the top of memory lets the first access succeed, so the arithmetic
//! after it gets reached.

use super::{ExecuteSuccess, ExecutorHelper};
use crate::Processor;
use crate::bus::Bus;
use crate::core::register::{Apsr, BaseReg, Reg};
use crate::decoder::{decode_16, decode_32};
use crate::memory::ram::RAM;

const TOP: u32 = 0xffff_f000;

fn processor() -> Processor {
    let mut core = Processor::new();
    #[cfg(feature = "has-fp")]
    {
        core.cpacr = 0x00f0_0000;
    }
    core.device(Some(Box::new(RAM::new(TOP, 0x1000))));
    core
}

fn run(core: &mut Processor, opcode: u32) -> ExecuteSuccess {
    core.execute_internal(&decode_32(opcode))
        .unwrap_or_else(|fault| panic!("{opcode:#010x} faulted with {fault:?}"))
}

#[test]
fn test_multiple_register_accesses_wrap_at_the_top() {
    let mut core = processor();
    core.set_r(Reg::R1, 0x1111_1111);
    core.set_r(Reg::R2, 0x2222_2222);

    // stm.w r0, {r1, r2} from the last word of memory goes on at address 0
    core.set_r(Reg::R0, 0xffff_fffc);
    run(&mut core, 0xe880_0006);
    assert_eq!(core.read32(0xffff_fffc), Ok(0x1111_1111));
    assert_eq!(core.read32(0), Ok(0x2222_2222));

    // ldm.w r0, {r1, r2} reads them back
    core.set_r(Reg::R1, 0);
    core.set_r(Reg::R2, 0);
    run(&mut core, 0xe890_0006);
    assert_eq!(core.get_r(Reg::R1), 0x1111_1111);
    assert_eq!(core.get_r(Reg::R2), 0x2222_2222);

    // stmdb r0, {r1, r2} from 0 starts 8 below it, at the top
    core.set_r(Reg::R0, 0);
    run(&mut core, 0xe900_0006);
    assert_eq!(core.read32(0xffff_fff8), Ok(0x1111_1111));
    assert_eq!(core.read32(0xffff_fffc), Ok(0x2222_2222));
}

#[test]
fn test_push_and_pop_wrap_the_stack_pointer() {
    let mut core = processor();
    core.set_r(Reg::R1, 0x1111_1111);
    core.set_r(Reg::R2, 0x2222_2222);

    // push.w {r1, r2} with sp 0 stores at the top and leaves sp at 0xfffffff8
    core.set_r(Reg::SP, 0);
    run(&mut core, 0xe92d_0006);
    assert_eq!(core.get_r(Reg::SP), 0xffff_fff8);
    assert_eq!(core.read32(0xffff_fffc), Ok(0x2222_2222));

    // pop.w {r1, r2} over the top leaves sp at 0
    core.set_r(Reg::SP, 0xffff_fff8);
    core.set_r(Reg::R1, 0);
    run(&mut core, 0xe8bd_0006);
    assert_eq!(core.get_r(Reg::SP), 0);
    assert_eq!(core.get_r(Reg::R1), 0x1111_1111);
}

#[test]
fn test_double_word_accesses_wrap_at_the_top() {
    let mut core = processor();
    core.set_r(Reg::R1, 0xaaaa_aaaa);
    core.set_r(Reg::R2, 0xbbbb_bbbb);
    core.set_r(Reg::R0, 0xffff_fffc);

    // strd r1, r2, [r0] and ldrd r1, r2, [r0]
    run(&mut core, 0xe9c0_1200);
    assert_eq!(core.read32(0), Ok(0xbbbb_bbbb));
    core.set_r(Reg::R2, 0);
    run(&mut core, 0xe9d0_1200);
    assert_eq!(core.get_r(Reg::R2), 0xbbbb_bbbb);
}

#[test]
fn test_literal_load_below_zero_wraps() {
    let mut core = processor();
    core.write32(0xffff_fffc, 0x1234_5678).unwrap();
    core.set_pc(0);

    // ldr.w r1, [pc, #-8]: the aligned pc reads 4, so the address is 4 - 8
    run(&mut core, 0xf85f_1008);
    assert_eq!(core.get_r(Reg::R1), 0x1234_5678);
}

#[test]
fn test_adr_wraps() {
    let mut core = processor();

    // adr.w r1, #8 at the top of memory
    core.set_pc(0xffff_fff0);
    run(&mut core, 0xf20f_0108);
    assert_eq!(core.get_r(Reg::R1), 0xffff_fff4u32.wrapping_add(8));
}

#[test]
fn test_branches_wrap_the_target() {
    let mut core = processor();

    // b.w, bl and beq.w with a positive offset from a pc that is past the middle of memory
    for opcode in [0xf000_b880, 0xf000_f880, 0xf000_8080] {
        core.set_pc(0x7fff_fff0);
        core.psr.set_z(0);
        run(&mut core, opcode);
        assert_eq!(core.get_pc(), 0x7fff_fff4 + 0x100);
    }

    // blx at the top of memory has pc + 4 wrap to 0
    core.set_pc(0xffff_fffc);
    core.set_r(Reg::R0, 0x1001);
    core.execute_internal(&decode_16(0x4780)).unwrap(); // blx r0
    assert_eq!(core.get_r(Reg::LR), 0xffff_fffe | 1);

    // and a backward one from the bottom
    core.set_pc(0);
    run(&mut core, 0xf7ff_bf82); // b.w #-252
    assert_eq!(core.get_pc(), 4u32.wrapping_sub(252));
}

#[test]
fn test_table_branches_wrap_the_table_address() {
    let mut core = processor();

    // tbb [r0, r1] and tbh [r0, r1, lsl #1] with a table address that wraps to the bottom
    core.set_r(Reg::R0, 0xffff_ffff);
    core.set_r(Reg::R1, 2);
    run(&mut core, 0xe8d0_f001);
    run(&mut core, 0xe8d0_f011);
}

#[cfg(feature = "has-fp")]
#[test]
fn test_floating_point_accesses_wrap_at_the_top() {
    let mut core = processor();
    core.set_r(Reg::R0, 0xffff_fffc);

    // vstr d0, [r0, #8] and vldr d0, [r0, #8] end up at address 4
    run(&mut core, 0xed80_0b02);
    run(&mut core, 0xed90_0b02);
    // vldr s0, [r0, #-8], vstr s0, [pc, #-8] with pc 0
    core.set_r(Reg::R0, 4);
    run(&mut core, 0xed10_0a02);
    core.set_pc(0);
    run(&mut core, 0xed0f_0a02);

    // vstmia r0, {d0, d1} and vldmia r0, {d0, d1} run over the top
    core.set_r(Reg::R0, 0xffff_fffc);
    run(&mut core, 0xec80_0b04);
    run(&mut core, 0xec90_0b04);
    run(&mut core, 0xec80_0a02);
    run(&mut core, 0xec90_0a02);
    // vstmdb r0!, {d0, d1} from 0, vpush and vpop with sp at 0 and at the top
    core.set_r(Reg::R0, 0);
    run(&mut core, 0xed20_0b04);
    core.set_r(Reg::SP, 0);
    run(&mut core, 0xed2d_0b04);
    run(&mut core, 0xed2d_0a02);
    core.set_r(Reg::SP, 0xffff_fff8);
    run(&mut core, 0xecbd_0b04);
    run(&mut core, 0xecbd_0a02);
}

#[test]
fn test_adr_forms_align_pc_and_add_or_subtract() {
    let mut core = processor();
    // pc 0x102 reads as 0x106, which aligns to 0x104
    core.set_pc(0x102);

    run(&mut core, 0xf20f_0108); // adr.w r1, #8
    assert_eq!(core.get_r(Reg::R1), 0x104 + 8);
    run(&mut core, 0xf2af_0108); // adr.w r1, #-8
    assert_eq!(core.get_r(Reg::R1), 0x104 - 8);
    core.execute_internal(&decode_16(0xa102)).unwrap(); // adr r1, #8 (T1)
    assert_eq!(core.get_r(Reg::R1), 0x104 + 8);

    // below zero it wraps
    core.set_pc(0);
    run(&mut core, 0xf2af_0108);
    assert_eq!(core.get_r(Reg::R1), 4u32.wrapping_sub(8));
}

#[test]
fn test_byte_and_halfword_literal_loads_read_from_the_aligned_pc() {
    let mut core = processor();
    // pc 0x102 reads as 0x106 and aligns to 0x104
    core.set_pc(0x102);
    core.write32(0x108, 0x0000_8ff0).unwrap();
    core.write32(0x100, 0x0000_80f1).unwrap();

    run(&mut core, 0xf8bf_1004); // ldrh.w r1, [pc, #4]
    assert_eq!(core.get_r(Reg::R1), 0x8ff0);
    run(&mut core, 0xf9bf_1004); // ldrsh.w r1, [pc, #4]
    assert_eq!(core.get_r(Reg::R1), 0xffff_8ff0);
    run(&mut core, 0xf89f_1004); // ldrb.w r1, [pc, #4]
    assert_eq!(core.get_r(Reg::R1), 0xf0);
    run(&mut core, 0xf91f_1004); // ldrsb.w r1, [pc, #-4]
    assert_eq!(core.get_r(Reg::R1), 0xffff_fff1);
}

#[test]
fn test_ldrd_literal_reads_from_the_aligned_pc() {
    let mut core = processor();
    core.set_pc(0x102); // reads as 0x106, aligned 0x104
    core.write32(0x10c, 0x1111_2222).unwrap();
    core.write32(0x110, 0x3333_4444).unwrap();
    core.write32(0x0fc, 0x5555_6666).unwrap();

    run(&mut core, 0xe9df_1202); // ldrd r1, r2, [pc, #8]
    assert_eq!(core.get_r(Reg::R1), 0x1111_2222);
    assert_eq!(core.get_r(Reg::R2), 0x3333_4444);
    run(&mut core, 0xe95f_1202); // ldrd r1, r2, [pc, #-8]
    assert_eq!(core.get_r(Reg::R1), 0x5555_6666);
}
