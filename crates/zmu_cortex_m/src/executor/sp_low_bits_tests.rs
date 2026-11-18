//! SP bits 1:0 are RAZ/WI on ARMv7-M (issue 78): B1.4.1 says so in words, A5.1.3 makes them
//! SBZP (a non-zero write is UNPREDICTABLE, a read returns zero), and the pseudocode of R[] in
//! B1.4.7 reads and writes `SP<31:2>:'00'`. Reset takes the vector's word AND 0xFFFFFFFC
//! (B1.5.5). So every way to write `SP_main` or `SP_process` drops the two low bits.

use super::Executor;
use crate::Processor;
use crate::bus::Bus;
use crate::core::register::{BaseReg, Reg};
use crate::decoder::{decode_16, decode_32};

const RAM: u32 = 0x2000_0000;

/// Both stacks, so each test runs on the one that is current: SPSEL = 0 and 1
fn each_stack(test: impl Fn(&mut Processor, fn(&Processor) -> u32)) {
    for process_stack in [false, true] {
        let mut core = Processor::new();
        core.control.sp_sel = process_stack;
        let current: fn(&Processor) -> u32 = if process_stack {
            |core| core.get_psp()
        } else {
            |core| core.get_msp()
        };
        test(&mut core, current);
    }
}

#[test]
fn test_a_write_to_sp_drops_bits_1_and_0() {
    each_stack(|core, current| {
        for (written, kept) in [
            (RAM + 0x103, RAM + 0x100),
            (RAM + 0x102, RAM + 0x100),
            (RAM + 0x101, RAM + 0x100),
            (RAM + 0x104, RAM + 0x104),
            (0xffff_ffff, 0xffff_fffc),
        ] {
            core.set_r(Reg::SP, written);
            assert_eq!(core.get_r(Reg::SP), kept, "set_r {written:#x}");
            assert_eq!(current(core), kept);
        }
    });
}

#[test]
fn test_set_msp_and_set_psp_drop_bits_1_and_0() {
    let mut core = Processor::new();
    core.set_msp(0xcafe_babe);
    core.set_psp(0xdead_beef);
    assert_eq!(core.get_msp(), 0xcafe_babc);
    assert_eq!(core.get_psp(), 0xdead_beec);
}

#[test]
fn test_adding_to_or_subtracting_from_sp_drops_bits_1_and_0() {
    each_stack(|core, current| {
        core.set_r(Reg::SP, RAM + 0x100);
        core.set_r(Reg::R1, 3);
        // add sp, sp, r1 and sub sp, sp, r1 with a register that is not a multiple of 4
        core.execute(&decode_16(0x448d), 2);
        assert_eq!(current(core), RAM + 0x100);
        core.set_r(Reg::R1, 7);
        core.execute(&decode_32(0xebad_0d01), 4);
        assert_eq!(current(core), RAM + 0x0f8);
    });
}

#[test]
fn test_mov_and_ldr_into_sp_drop_bits_1_and_0() {
    each_stack(|core, current| {
        core.set_r(Reg::R1, RAM + 0x203);
        core.execute(&decode_16(0x468d), 2); // mov sp, r1
        assert_eq!(current(core), RAM + 0x200);
        core.set_r(Reg::R2, RAM + 0x10);
        core.write32(RAM + 0x10, RAM + 0x307).unwrap();
        core.execute(&decode_32(0xf8d2_d000), 4); // ldr sp, [r2]
        assert_eq!(current(core), RAM + 0x304);
        // and reading it back gives the same number
        core.execute(&decode_16(0x4669), 2); // mov r1, sp
        assert_eq!(core.get_r(Reg::R1), RAM + 0x304);
    });
}

#[test]
fn test_msr_to_msp_and_psp_drops_bits_1_and_0_and_mrs_reads_zeros() {
    let mut core = Processor::new();
    core.set_r(Reg::R1, 0x2000_0103);
    core.execute(&decode_32(0xf381_8808), 4); // msr msp, r1
    core.execute(&decode_32(0xf381_8809), 4); // msr psp, r1
    assert_eq!((core.get_msp(), core.get_psp()), (0x2000_0100, 0x2000_0100));
    core.set_r(Reg::R2, 0xffff_ffff);
    core.execute(&decode_32(0xf3ef_8208), 4); // mrs r2, msp
    assert_eq!(core.get_r(Reg::R2), 0x2000_0100);
    core.set_r(Reg::R2, 0xffff_ffff);
    core.execute(&decode_32(0xf3ef_8209), 4); // mrs r2, psp
    assert_eq!(core.get_r(Reg::R2), 0x2000_0100);
}

#[test]
fn test_pop_and_push_keep_sp_aligned_when_it_was_written_unaligned() {
    each_stack(|core, current| {
        core.set_r(Reg::SP, RAM + 0x203);
        // pop {r1}: SP was 0x200, so r1 comes from 0x200 and SP is 0x204
        core.write32(RAM + 0x200, 0x1234_5678).unwrap();
        core.execute(&decode_16(0xbc02), 2);
        assert_eq!(core.get_r(Reg::R1), 0x1234_5678);
        assert_eq!(current(core), RAM + 0x204);
        // push {r1}: from the aligned SP too, and it stores at SP - 4
        core.set_r(Reg::SP, RAM + 0x303);
        core.set_r(Reg::R1, 0x8765_4321);
        core.execute(&decode_16(0xb402), 2);
        assert_eq!(current(core), RAM + 0x2fc);
        assert_eq!(core.read32(RAM + 0x2fc), Ok(0x8765_4321));
    });
}

#[test]
fn test_add_r_and_sub_r_on_sp_keep_bits_1_and_0_zero() {
    each_stack(|core, current| {
        core.set_r(Reg::SP, RAM + 0x100);
        core.add_r(Reg::SP, 3);
        assert_eq!(current(core), RAM + 0x100);
        core.add_r(Reg::SP, 6);
        assert_eq!(current(core), RAM + 0x104);
        core.sub_r(Reg::SP, 3);
        assert_eq!(current(core), RAM + 0x100);
        core.sub_r(Reg::SP, 1);
        assert_eq!(current(core), RAM + 0x0fc);
    });
}
