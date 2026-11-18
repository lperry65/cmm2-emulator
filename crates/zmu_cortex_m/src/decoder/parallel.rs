use crate::core::bits::Bits;
use crate::core::instruction::{
    Instruction, ParallelLanes, ParallelOp, ParallelResult, Reg3NoSetFlagsParams,
};

/// The 36 parallel adds and subtracts (A5.3.13 and A5.3.14, Tables A5-25 and A5-26) are one
/// encoding: hw1<6:4> (op1) picks the lane arrangement, hw2<6> the sign (0 signed, 1 unsigned) and
/// hw2<5:4> (op2) what is done with each sum: 00 keep it and set GE, 01 saturate it, 10 halve it.
/// op1 = 011 and 111, and op2 = 11, are not allocated, so they are UNDEFINED.
#[allow(non_snake_case)]
pub fn decode_PARALLEL_t1(opcode: u32) -> Instruction {
    let lanes = match opcode.get_bits(20..23) {
        0b000 => ParallelLanes::Add8,
        0b001 => ParallelLanes::Add16,
        0b010 => ParallelLanes::Asx,
        0b100 => ParallelLanes::Sub8,
        0b101 => ParallelLanes::Sub16,
        0b110 => ParallelLanes::Sax,
        _ => return undefined(opcode),
    };
    let result = match opcode.get_bits(4..6) {
        0b00 => ParallelResult::Wrap,
        0b01 => ParallelResult::Saturate,
        0b10 => ParallelResult::Halve,
        _ => return undefined(opcode),
    };
    Instruction::PARALLEL {
        params: Reg3NoSetFlagsParams {
            rd: opcode.get_bits(8..12).into(),
            rn: opcode.get_bits(16..20).into(),
            rm: opcode.get_bits(0..4).into(),
        },
        op: ParallelOp {
            signed: !opcode.get_bit(6),
            result,
            lanes,
        },
    }
}

fn undefined(opcode: u32) -> Instruction {
    Instruction::UDF {
        imm32: 0,
        opcode: opcode.into(),
        thumb32: true,
    }
}
