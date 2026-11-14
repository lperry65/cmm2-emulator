//! Guest code never panics the host. Every 16-bit opcode, and a fixed pseudo-random sample of
//! the 32-bit ones, run on registers that are small, large, near the top of memory, and
//! pointing into RAM. A guest that does something the manual does not define is meant to get an
//! architectural fault or an undefined instruction, not take the emulator down.
//!
//! `ZMU_GUEST_FUZZ` sets the number of opcodes (the default is a few seconds in a debug build).

use super::{ExecuteSuccess, Executor, ExecutorHelper};
use crate::core::instruction::instruction_size;
use crate::core::register::{BaseReg, Reg};
use crate::core::thumb::ThumbCode;
use crate::decoder::{Decoder, is_thumb32};
use crate::{Processor, ProcessorMode};
use std::panic::{AssertUnwindSafe, catch_unwind};

const START: u32 = 0x100;
const DEFAULT_OPCODES: u64 = 1_000_000;

// xorshift, so the sample is the same on every run
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    // a register value: an address in RAM, a small number, near a power of two, or anything
    fn value(&mut self) -> u32 {
        match self.next() % 6 {
            0 => 0x2000_0000 + (self.next() & 0x1_fff0),
            1 => 0x2000_0000 + (self.next() & 0x1_ffff),
            2 => self.next() & 0xff,
            3 => 0xffff_fff0 | (self.next() & 15),
            4 => 0x7fff_fff0 | (self.next() & 15),
            _ => self.next(),
        }
    }
}

fn processor() -> Processor {
    #[cfg_attr(not(feature = "has-fp"), allow(unused_mut))]
    let mut core = Processor::new();
    #[cfg(feature = "has-fp")]
    {
        core.cpacr = 0x00f0_0000;
    }
    core
}

fn load_registers(core: &mut Processor, rng: &mut Rng) {
    for number in 0..13 {
        core.set_r(Reg::from(number as u8), rng.value());
    }
    core.set_r(Reg::SP, 0x2000_0000 + (rng.next() & 0x1_fff8));
    core.set_r(Reg::LR, rng.value());
    core.set_pc(START);
    core.itstate = 0;
    core.mode = ProcessorMode::ThreadMode;
    core.psr.value = rng.next() & 0xf800_0000;
    core.fpscr = rng.next() & 0xfc9f_ffff;
}

fn run(code: ThumbCode, core: &mut Processor, through_faults: bool) {
    let instruction = core.decode(code);
    if through_faults {
        // the whole of a step, with the fault and exception entry that may follow
        core.execute(&instruction, instruction_size(&instruction));
        return;
    }
    let result = core.execute_internal(&instruction);
    // An instruction that is not a branch leaves PC for `execute` to advance. A write to PC that
    // does not come from a branch is UNPREDICTABLE, and has to be UDF in the decoder instead.
    if let Ok(ExecuteSuccess::Taken { .. } | ExecuteSuccess::NotTaken) = result {
        assert_eq!(
            core.get_pc(),
            START,
            "{instruction:?} moved PC and did not branch"
        );
    }
}

fn opcode(code: ThumbCode) -> u32 {
    match code {
        ThumbCode::Thumb32 { opcode } => opcode,
        ThumbCode::Thumb16 { opcode } => u32::from(opcode),
        ThumbCode::Undefined => 0,
    }
}

#[test]
fn test_no_opcode_panics_the_host() {
    let count = std::env::var("ZMU_GUEST_FUZZ")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(DEFAULT_OPCODES);
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut core = processor();
    let mut wide = 0_u64;

    for index in 0..count {
        // Every 16-bit opcode first. After that, every other opcode is made 32-bit on purpose (a
        // first halfword of 0xe800 or more, with a random second half) and the rest are random,
        // which are 32-bit about one time in ten. The default run is about half a million 32-bit
        // opcodes, and about as many 16-bit ones again, and not the 9% or so that random first
        // halves alone would give.
        let first = if index < 0x1_0000 {
            index as u32
        } else if index % 2 == 0 {
            0xe800 + rng.next() % 0x1800
        } else {
            rng.next() & 0xffff
        };
        let code = if is_thumb32(first as u16) {
            if index < 0x1_0000 {
                continue;
            }
            wide += 1;
            ThumbCode::Thumb32 {
                opcode: first << 16 | (rng.next() & 0xffff),
            }
        } else {
            ThumbCode::Thumb16 {
                opcode: first as u16,
            }
        };

        // a fresh processor now and then, for the state a panic leaves, and for the memory
        if index % 1000 == 0 {
            core = processor();
        }
        for through_faults in [false, true] {
            load_registers(&mut core, &mut rng);
            let outcome = catch_unwind(AssertUnwindSafe(|| run(code, &mut core, through_faults)));
            if let Err(panic) = outcome {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
                    .unwrap_or_default();
                panic!("opcode {:#010x} panicked: {message}", opcode(code));
            }
        }
    }

    // the claim in the comment above: a third of the opcodes at least are 32-bit
    if count >= DEFAULT_OPCODES {
        assert!(
            wide * 3 >= count,
            "only {wide} of {count} opcodes were 32-bit"
        );
    }
}

// Exception returns are made in Handler mode, with an EXC_RETURN in PC and a frame on the stack
// that the guest wrote, so the opcode test above (which starts in Thread mode) never gets there.
// This one starts in Handler mode with a random set of active exceptions, a frame of random words
// (the xPSR with a random IPSR and Thumb bit among them) and a random EXC_RETURN, returns with BX,
// POP, LDM or LDR, and then lets the processor run on for a few steps, so that what a refused
// return leaves behind is used.
#[cfg(not(feature = "armv6m"))]
#[test]
fn test_exception_returns_with_forged_frames_never_panic_the_host() {
    use crate::bus::Bus;
    use crate::core::exception::Exception;
    use crate::decoder::{decode_16, decode_32};

    let count = std::env::var("ZMU_GUEST_FUZZ")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(DEFAULT_OPCODES)
        / 20;
    let mut rng = Rng(0x0bad_f00d_dead_beef);
    // BX LR, POP {PC}, LDR PC, [SP], #4 and POP.W {R0, PC} (a list of one register is UNPREDICTABLE)
    let returns = [
        decode_16(0x4770),
        decode_16(0xbd00),
        decode_32(0xf85d_fb04),
        decode_32(0xe8bd_8001),
    ];
    let numbers = [3_usize, 2, 4, 5, 6, 11, 12, 14, 15, 16, 17, 100, 255];

    for index in 0..count {
        let mut core = processor();
        core.set_msp(0x2000_0000 + (rng.next() & 0xff00) + 0x100);
        let sp = core.get_msp();
        let form = index as usize % returns.len();
        let instruction = returns[form];
        // BX takes the EXC_RETURN from LR and the frame is at SP. POP, LDR and LDM take it from the
        // word at SP, and the frame is the 32 bytes after it (the 104 of an FP frame, which is
        // bigger than the random words below, and are random too).
        let frame = match form {
            0 => sp,
            3 => sp + 8,
            _ => sp + 4,
        };
        core.set_psp(frame);
        // a frame of random words, with the xPSR and the return address that are likely to be taken
        for word in 0..26 {
            core.write32(frame + word * 4, rng.value()).unwrap();
        }
        let ipsr = if rng.next().is_multiple_of(3) {
            rng.next() & 0x1ff
        } else {
            numbers[rng.next() as usize % numbers.len()] as u32
        };
        let thumb = u32::from(!rng.next().is_multiple_of(4));
        core.write32(frame + 0x18, rng.next()).unwrap();
        core.write32(frame + 0x1c, rng.next() & 0xf800_fc00 | thumb << 24 | ipsr)
            .unwrap();

        // the active exceptions: one or two, the first being the one that is running
        let running = numbers[rng.next() as usize % numbers.len()];
        let other = rng
            .next()
            .is_multiple_of(2)
            .then(|| numbers[rng.next() as usize % numbers.len()]);
        if let Some(other) = other {
            core.test_set_active_exception(Exception::from(other));
        }
        core.test_set_active_exception(Exception::from(running));
        let active = if other.is_some_and(|other| other != running) {
            2
        } else {
            1
        };
        let exc_return = match rng.next() % 5 {
            0 => 0xffff_fff1,
            1 => 0xffff_fff9,
            2 => 0xffff_fffd,
            3 => 0xffff_ffe1 | (rng.next() & 0x1c),
            _ => 0xf000_0000 | (rng.next() & 0x0fff_ffff),
        };
        core.set_r(Reg::LR, exc_return);
        // POP.W {R0, PC} pops R0 first
        core.write32(if form == 3 { sp + 4 } else { sp }, exc_return)
            .unwrap();
        core.set_pc(START);
        core.cfsr = 0;

        // what the manual says the return does (B1.5.8): EXC_RETURN and, for Thread mode, the
        // nesting first (INVPC, no frame), then after the pop the IPSR against the mode (INVPC),
        // then the Thumb bit (INVSTATE). A return to Handler mode has no nesting check.
        let to_handler = exc_return & 0xf == 0x1;
        let to_thread = matches!(exc_return & 0xf, 0x9 | 0xd);
        let exists = matches!(ipsr, 2..=6 | 11 | 12 | 14 | 15 | 16..=255);
        let (invpc, invstate) = if !(to_handler || to_thread)
            || (to_thread && active != 1)
            || (to_handler && !exists)
            || (to_thread && ipsr != 0)
        {
            (true, false)
        } else {
            (false, thumb == 0)
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            core.execute(&instruction, instruction_size(&instruction));
            let faults = (core.cfsr & (1 << 18) != 0, core.cfsr & (1 << 17) != 0);
            assert_eq!(
                faults,
                (invpc, invstate),
                "the crafted frame was not the one used"
            );
            for _ in 0..4 {
                core.step();
            }
        }));
        if let Err(panic) = outcome {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
                .unwrap_or_default();
            panic!(
                "{instruction:?} with EXC_RETURN {exc_return:#x}, running {running}, stacked IPSR {ipsr}, \
                 Thumb {thumb} panicked: {message}"
            );
        }
    }
}

// the stack pointer that an entry uses now, and the size of the frame it pushes
#[cfg(not(feature = "armv6m"))]
fn uses_psp(core: &Processor) -> bool {
    core.control.sp_sel && core.mode == ProcessorMode::ThreadMode
}
#[cfg(not(feature = "armv6m"))]
fn sp_of(core: &Processor, psp: bool) -> u32 {
    if psp { core.get_psp() } else { core.get_msp() }
}
#[cfg(not(feature = "armv6m"))]
fn frame_bound(core: &Processor) -> u32 {
    #[cfg(feature = "has-fp")]
    if core.control.fpca {
        return 0x68 + 4;
    }
    let _ = core;
    0x20 + 4
}

// Exception entries on stacks that fail and vector tables that cannot be read: random stack
// pointers (MSP and PSP, SPSEL, FPCA) near the end of SRAM, vector tables of random length in
// flash and at an aligned VTOR (VTOR<31:7>, B1-533), priorities, fault enables, PRIGROUP and
// pending exceptions, entered by SVC, BKPT, UDF and from the pending ones. Nothing panics, and an
// entry lowers the stack pointer by at most one frame (B1.5.11: a derived exception reuses the
// one PushStack), whatever fails: 0x20 plus the 4 of the alignment, or 0x68 plus 4 with FPCA.
#[cfg(not(feature = "armv6m"))]
#[test]
fn test_failing_stacks_and_unreadable_vectors_never_panic_and_lower_sp_by_one_frame() {
    use crate::bus::Bus;
    use crate::core::exception::{Exception, ExceptionHandling};
    use crate::decoder::decode_16;

    let count = std::env::var("ZMU_GUEST_FUZZ")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(DEFAULT_OPCODES)
        / 100;
    let mut rng = Rng(0x5eed_1234_9876_abcd);
    let stacks = [
        0x2000_0200_u32,
        0x2001_fff0,
        0x2002_0000,
        0x2002_0004,
        0x2002_0010,
        0x2002_0048,
        0x2002_0070,
        0,
        0x0000_0010,
        0xffff_fff0,
    ];
    let vtors = [0_u32, 0x2001_ff80, 0x2001_ff00, 0xffff_ff80, 0x2002_0000];
    let table_sizes = [0x4_usize, 0x8, 0xc, 0x10, 0x20, 0x40, 0x100];
    let priorities = [0_u8, 0x20, 0x40, 0x80];
    let faults = [
        Exception::MemoryManagementFault,
        Exception::BusFault,
        Exception::UsageFault,
        Exception::SVCall,
        Exception::SysTick,
        Exception::PendSV,
        Exception::DebugMonitor,
    ];
    let actions = [0xdf00_u16, 0xbe00, 0xde00, 0xbf00];

    for _ in 0..count {
        let mut core = processor();
        let size = table_sizes[rng.next() as usize % table_sizes.len()];
        core.flash_memory(size, &vec![0; size]);
        core.cache_instructions();
        core.ccr = rng.next() & 0x201;
        core.write32(0xE000_ED24, (rng.next() & 7) << 16).unwrap();
        core.write32(0xE000_ED0C, 0x05fa_0000 | ((rng.next() % 8) << 8))
            .unwrap();
        for exception in faults {
            core.set_exception_priority(exception, priorities[rng.next() as usize % 4]);
        }
        core.write32(0xE000_EDFC, (rng.next() & 3) << 16).unwrap();
        core.write32(0xE000_ED08, vtors[rng.next() as usize % vtors.len()])
            .unwrap();
        core.set_msp(stacks[rng.next() as usize % stacks.len()]);
        core.set_psp(stacks[rng.next() as usize % stacks.len()]);
        core.control.sp_sel = rng.next().is_multiple_of(2);
        #[cfg(feature = "has-fp")]
        {
            core.control.fpca = rng.next().is_multiple_of(2);
            core.fpccr = (rng.next() & 3) << 30;
        }
        core.execution_priority = core.get_execution_priority();
        for _ in 0..(rng.next() % 3) {
            core.set_exception_pending(faults[rng.next() as usize % faults.len()]);
        }
        if rng.next().is_multiple_of(2) {
            core.write32(0xE000_E100, 1).unwrap();
            core.write32(0xE000_E200, 1).unwrap();
        }
        core.set_pc(START);
        let action = actions[rng.next() as usize % actions.len()];

        let psp = uses_psp(&core);
        let before = sp_of(&core, psp);
        let bound = frame_bound(&core);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            core.execute(&decode_16(action), 2);
            let lowered = before.wrapping_sub(sp_of(&core, psp));
            assert!(
                lowered <= bound || lowered > 0xffff_ff00,
                "the action lowered SP by {lowered:#x}, the frame is {bound:#x}"
            );
            for _ in 0..3 {
                let psp = uses_psp(&core);
                let sp = sp_of(&core, psp);
                let bound = frame_bound(&core);
                core.check_exceptions();
                let lowered = sp.wrapping_sub(sp_of(&core, psp));
                assert!(
                    lowered <= bound || lowered > 0xffff_ff00,
                    "an entry lowered SP by {lowered:#x}, the frame is {bound:#x}"
                );
            }
        }));
        if let Err(panic) = outcome {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
                .unwrap_or_default();
            panic!(
                "action {action:#x}, SP {before:#x}, VTOR {:#x}, table {size:#x} panicked: {message}",
                core.vtor
            );
        }
    }
}
