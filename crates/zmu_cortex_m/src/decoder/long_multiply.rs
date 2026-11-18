use crate::core::bits::Bits;
use crate::core::instruction::{Instruction, Reg643232Params};

/// The long multiplies of the DSP extension (Table A5-29), which all name `RdLo` in hw2<15:12>,
/// `RdHi` in hw2<11:8>, Rn in hw1<3:0> and Rm in hw2<3:0>.
fn long_params(opcode: u32) -> Reg643232Params {
    Reg643232Params {
        rdlo: opcode.get_bits(12..16).into(),
        rdhi: opcode.get_bits(8..12).into(),
        rn: opcode.get_bits(16..20).into(),
        rm: opcode.get_bits(0..4).into(),
    }
}

/// SMLALBB, SMLALBT, SMLALTB and SMLALTT (A7.7.139): hw2<5> is N and hw2<4> is M, and the
/// letters are in that order, B for the bottom halfword and T for the top
#[allow(non_snake_case)]
pub fn decode_SMLAL_half_t1(opcode: u32) -> Instruction {
    Instruction::SMLAL_half {
        params: long_params(opcode),
        n_high: opcode.get_bit(5),
        m_high: opcode.get_bit(4),
    }
}

/// SMLALD and SMLALDX (A7.7.140), op1 = 100
#[allow(non_snake_case)]
pub fn decode_SMLALD_t1(opcode: u32) -> Instruction {
    smdual_long(opcode, false)
}

/// SMLSLD and SMLSLDX (A7.7.143), op1 = 101
#[allow(non_snake_case)]
pub fn decode_SMLSLD_t1(opcode: u32) -> Instruction {
    smdual_long(opcode, true)
}

fn smdual_long(opcode: u32, subtract: bool) -> Instruction {
    Instruction::SMDUAL_LONG {
        params: long_params(opcode),
        subtract,
        swap: opcode.get_bit(4),
    }
}

/// UMAAL (A7.7.202)
#[allow(non_snake_case)]
pub fn decode_UMAAL_t1(opcode: u32) -> Instruction {
    Instruction::UMAAL {
        params: long_params(opcode),
    }
}
