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
    // BX LR, POP {PC}, LDR PC, [SP], #4 and LDM SP!, {PC}
    let returns = [
        decode_16(0x4770),
        decode_16(0xbd00),
        decode_32(0xf85d_fb04),
        decode_32(0xe8bd_8000),
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
        let frame = if form == 0 { sp } else { sp + 4 };
        core.set_psp(frame);
        // a frame of random words, with the xPSR and the return address that are likely to be taken
        for word in 0..26 {
            core.write32(frame + word * 4, rng.value()).unwrap();
        }
        let ipsr = if rng.next() % 3 == 0 {
            rng.next() & 0x1ff
        } else {
            numbers[rng.next() as usize % numbers.len()] as u32
        };
        let thumb = (rng.next() % 4 != 0) as u32;
        core.write32(frame + 0x18, rng.next()).unwrap();
        core.write32(frame + 0x1c, rng.next() & 0xf800_fc00 | thumb << 24 | ipsr)
            .unwrap();

        // the active exceptions: one or two, the first being the one that is running
        let running = numbers[rng.next() as usize % numbers.len()];
        let other = (rng.next() % 2 == 0).then(|| numbers[rng.next() as usize % numbers.len()]);
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
        core.write32(sp, exc_return).unwrap();
        core.set_pc(START);
        core.cfsr = 0;

        // what the manual says the return does: EXC_RETURN and the nesting first (INVPC), then
        // after the pop the IPSR against the mode (INVPC), then the Thumb bit (INVSTATE)
        let to_handler = exc_return & 0xf == 0x1;
        let to_thread = matches!(exc_return & 0xf, 0x9 | 0xd);
        let exists = matches!(ipsr, 2..=6 | 11 | 12 | 14 | 15 | 16..=255);
        let (invpc, invstate) = if !(to_handler || to_thread)
            || (to_handler && active == 1)
            || (to_thread && active != 1)
        {
            (true, false)
        } else if (to_handler && !exists) || (to_thread && ipsr != 0) {
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
