//! An exception return as the last instruction of an IT block (issue 79), the order of write back
//! and exception return in the instructions that can do one (issue 80), and what each of them
//! costs.
//!
//! Every test takes an SVC from Thread mode inside `itt eq` (so the frame holds an ITSTATE that is
//! not zero), then runs the returning instruction in Handler mode.

use super::{Executor, ExecutorHelper};
use crate::Processor;
use crate::ProcessorMode;
use crate::bus::Bus;
use crate::core::register::{Apsr, BaseReg, Epsr, Reg};
use crate::decoder::{decode_16, decode_32};

const RAM: u32 = 0x2000_0000;
const EXC_RETURN: u32 = 0xffff_fff9;

/// The ITSTATE of the thread after `svceq` in `itt eq`: one instruction left, the last
const STACKED_ITSTATE: u8 = 0x08;

/// A handler running after `svceq` from `itt eq`, and the address of its frame. The thread had
/// r2 = 0x22222222, and returns to 0x102.
fn in_handler_after_svceq() -> (Processor, u32) {
    let mut core = Processor::new();
    core.set_msp(RAM + 0x400);
    core.set_pc(0x100);
    core.psr.set_t(true);
    core.psr.set_z_bit(true);
    core.set_r(Reg::R2, 0x2222_2222);
    // itt eq: ITSTATE = firstcond:mask = 0000:0100, and svceq is the first of the two
    core.set_itstate(0x04);
    core.execute(&decode_16(0xdf00), 2);
    assert_eq!(core.mode, ProcessorMode::HandlerMode);
    // the entry cleared the handler's ITSTATE, and stacked the thread's: ITSTATE<7:2> are
    // xPSR<15:10> and ITSTATE<1:0> are xPSR<26:25>
    assert_eq!(core.itstate, 0);
    let frame = core.get_msp();
    let xpsr = core.read32(frame + 0x1c).unwrap();
    let stacked = ((xpsr >> 10) & 0x3f) << 2 | ((xpsr >> 25) & 3);
    assert_eq!(stacked as u8, STACKED_ITSTATE);
    (core, frame)
}

fn is_thread_mode_again(core: &mut Processor) {
    assert_eq!(core.mode, ProcessorMode::ThreadMode);
    assert_eq!(core.get_pc(), 0x102);
    // r2 is the thread's r2 from the frame, whatever the instruction did to it before
    assert_eq!(core.get_r(Reg::R2), 0x2222_2222);
    // the thread is in the last instruction of its IT block, as the frame said
    assert_eq!(core.itstate, STACKED_ITSTATE);
}

/// How each instruction is set up in the handler: its opcode (one or two halfwords), the size, and
/// the registers and memory it needs to find `EXC_RETURN` and to land SP on the frame.
struct Case {
    name: &'static str,
    opcode: u32,
    size: usize,
    setup: fn(&mut Processor, u32),
}

fn exec(core: &mut Processor, case: &Case) -> u32 {
    let instruction = if case.size == 2 {
        decode_16(case.opcode as u16)
    } else {
        decode_32(case.opcode)
    };
    core.execute(&instruction, case.size)
}

// a data area below the frame for the instructions whose base is not SP
fn buffer(frame: u32) -> u32 {
    frame - 0x80
}

const CASES: [Case; 7] = [
    Case {
        name: "bx lr",
        opcode: 0x4770,
        size: 2,
        setup: |core, _| core.set_r(Reg::LR, EXC_RETURN),
    },
    Case {
        name: "pop {r4, pc}",
        opcode: 0xbd10,
        size: 2,
        setup: |core, frame| {
            // SP is 8 under the frame; the pop leaves it on the frame
            core.set_r(Reg::SP, frame - 8);
            core.write32(frame - 4, EXC_RETURN).unwrap();
        },
    },
    Case {
        name: "ldm.w r2!, {r4, pc}",
        opcode: 0xe8b2_8010,
        size: 4,
        setup: |core, frame| {
            core.set_r(Reg::R2, buffer(frame));
            core.write32(buffer(frame) + 4, EXC_RETURN).unwrap();
        },
    },
    Case {
        name: "ldmdb r2!, {r4, pc}",
        opcode: 0xe932_8010,
        size: 4,
        setup: |core, frame| {
            core.set_r(Reg::R2, buffer(frame) + 8);
            core.write32(buffer(frame) + 4, EXC_RETURN).unwrap();
        },
    },
    Case {
        name: "ldmdb sp!, {r4, pc}",
        opcode: 0xe93d_8010,
        size: 4,
        setup: |core, frame| {
            core.set_r(Reg::SP, frame + 8);
            core.write32(frame + 4, EXC_RETURN).unwrap();
        },
    },
    Case {
        name: "ldr pc, [r2], #4",
        opcode: 0xf852_fb04,
        size: 4,
        setup: |core, frame| {
            core.set_r(Reg::R2, buffer(frame));
            core.write32(buffer(frame), EXC_RETURN).unwrap();
        },
    },
    Case {
        name: "ldr pc, [sp], #4",
        opcode: 0xf85d_fb04,
        size: 4,
        setup: |core, frame| {
            core.set_r(Reg::SP, frame - 4);
            core.write32(frame - 4, EXC_RETURN).unwrap();
        },
    },
];

#[test]
fn test_an_exception_return_from_the_last_instruction_of_an_it_block_keeps_the_restored_itstate() {
    for case in &CASES {
        let (mut core, frame) = in_handler_after_svceq();
        (case.setup)(&mut core, frame);
        // the handler's own IT block, one instruction left: it al, and this is the last
        core.set_itstate(0xe8);
        exec(&mut core, case);
        assert_eq!(core.itstate, STACKED_ITSTATE, "ITSTATE after {}", case.name);
        is_thread_mode_again(&mut core);
    }
}

#[test]
fn test_an_exception_return_outside_an_it_block_restores_the_itstate_too() {
    for case in &CASES {
        let (mut core, frame) = in_handler_after_svceq();
        (case.setup)(&mut core, frame);
        exec(&mut core, case);
        is_thread_mode_again(&mut core);
    }
}

#[test]
fn test_an_exception_return_inside_an_it_block_but_not_last_is_unpredictable_and_runs_here() {
    // The manual makes BX, POP and LDM with PC UNPREDICTABLE unless they are outside an IT block
    // or the last in it (A7.7.20, A7.7.99, A7.7.41). zmu runs the return, and the ITSTATE is the
    // frame's: what comes back from the stack replaces what the handler had, however it got there.
    for case in &CASES {
        let (mut core, frame) = in_handler_after_svceq();
        (case.setup)(&mut core, frame);
        // itt al with the first of the two left
        core.set_itstate(0xe4);
        exec(&mut core, case);
        assert_eq!(core.itstate, STACKED_ITSTATE, "ITSTATE after {}", case.name);
        is_thread_mode_again(&mut core);
    }
}

#[test]
fn test_a_branch_that_is_not_an_exception_return_still_advances_the_it_block() {
    // bx r0 to an ordinary address, last in the block: the block ends
    let (mut core, _) = in_handler_after_svceq();
    core.set_r(Reg::R0, 0x0800_0001);
    core.set_itstate(0xe8);
    core.execute(&decode_16(0x4700), 2);
    assert_eq!(core.mode, ProcessorMode::HandlerMode);
    assert_eq!(core.itstate, 0);
    // and not last: one instruction fewer left
    let (mut core, _) = in_handler_after_svceq();
    core.set_r(Reg::R0, 0x0800_0001);
    core.set_itstate(0xe4);
    core.execute(&decode_16(0x4700), 2);
    assert_eq!(core.itstate, 0xe8);
}

#[test]
fn test_an_ordinary_branch_after_an_exception_return_advances_its_it_block() {
    // the flag that says "an exception return happened" is per instruction: it must not outlive
    // the instruction that set it
    let (mut core, frame) = in_handler_after_svceq();
    (CASES[0].setup)(&mut core, frame);
    exec(&mut core, &CASES[0]);
    is_thread_mode_again(&mut core);
    core.set_r(Reg::R0, 0x0800_0001);
    core.set_itstate(0xe8);
    core.execute(&decode_16(0x4700), 2); // bx r0
    assert_eq!(core.itstate, 0);
}

#[test]
fn test_mov_pc_is_a_plain_branch_and_not_an_exception_return() {
    // MOV PC, LR is BranchWritePC (ALUWritePC): only BX, POP, LDM and LDR with PC are
    // LoadWritePC/BXWritePC, the ones that can start an exception return (B1.5.8)
    let (mut core, _) = in_handler_after_svceq();
    core.set_r(Reg::LR, EXC_RETURN);
    core.set_itstate(0xe8);
    core.execute(&decode_16(0x46f7), 2);
    assert_eq!(core.mode, ProcessorMode::HandlerMode);
    assert_eq!(core.itstate, 0);
}

// Issue 80: the write back of LDM, LDMDB, POP and LDR comes before the exception return, so that
// what the return unstacks is not changed afterwards. Each case above checks r2 against the frame
// (`is_thread_mode_again`); this one is the case from the issue, by itself.
#[test]
fn test_ldm_writes_back_before_the_exception_return() {
    let (mut core, frame) = in_handler_after_svceq();
    core.set_r(Reg::R2, buffer(frame));
    core.write32(buffer(frame) + 4, EXC_RETURN).unwrap();
    // ldm.w r2!, {r4, pc}
    core.execute(&decode_32(0xe8b2_8010), 4);
    // r2 came back from the frame as 0x22222222 and was not changed by the write back (+8)
    assert_eq!(core.get_r(Reg::R2), 0x2222_2222);
}

// One rule for a load of PC: the pipeline refill P is added to 1 + N, with the same P for LDM, POP
// and LDMDB. P = 3 is what POP had; the M4 TRM gives P as 1 to 3, and ARM publishes no figure for
// the Cortex-M7.
#[test]
fn test_ldm_pop_and_ldmdb_count_one_plus_n_and_three_more_for_a_pc_load() {
    // (registers loaded, ldm and ldmdb list, pop list, loads PC)
    let shapes: [(u32, u32, u16, bool); 4] = [
        (2, 0x0030, 0x0030, false),
        (3, 0x0070, 0x0070, false),
        (2, 0x8010, 0x0110, true),
        (3, 0x8030, 0x0130, true),
    ];
    for (registers, list, pop_list, with_pc) in shapes {
        let expected = 1 + registers + if with_pc { 3 } else { 0 };
        for (name, opcode) in [
            ("ldm.w r2!", 0xe8b2_0000 | list),
            ("ldmdb r2!", 0xe932_0000 | list),
            ("ldmdb sp!", 0xe93d_0000 | list),
        ] {
            let mut core = Processor::new();
            core.set_msp(RAM + 0x400);
            core.set_r(Reg::R2, RAM + 0x100);
            core.set_r(Reg::SP, RAM + 0x100);
            // an odd PC word: a Thumb target
            for word in 0..16 {
                core.write32(RAM + 0xe0 + 4 * word, 0x0800_0101).unwrap();
            }
            assert_eq!(
                core.execute(&decode_32(opcode), 4),
                expected,
                "{name} {list:#06x}"
            );
        }
        // pop (the 16-bit form has r0 to r7 and PC) from the same stack
        let mut core = Processor::new();
        core.set_msp(RAM + 0x400);
        core.set_r(Reg::SP, RAM + 0x100);
        for word in 0..16 {
            core.write32(RAM + 0xe0 + 4 * word, 0x0800_0101).unwrap();
        }
        assert_eq!(
            core.execute(&decode_16(0xbc00 | pop_list), 2),
            expected,
            "pop {pop_list:#06x}"
        );
    }
}
