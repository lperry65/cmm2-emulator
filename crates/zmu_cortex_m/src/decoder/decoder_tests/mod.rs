use crate::core::instruction::{
    BfcParams, BfiParams, BfxParams, CondBranchParams, Imm32Carry, MovtParams, ParamsRegImm32,
    Reg2DoubleParams, Reg2FullParams, Reg2ImmCarryParams, Reg2ImmParams, Reg2Params,
    Reg2RdRmParams, Reg2RnRmParams, Reg2RtRnImm32Params, Reg2ShiftNParams,
    Reg2ShiftNoSetFlagsParams, Reg2ShiftParams, Reg2UsizeParams, Reg3FullParams,
    Reg3NoSetFlagsParams, Reg3Params, Reg3RdRtRnImm32Params, Reg3RdRtRnParams, Reg3ShiftParams,
    Reg4NoSetFlagsParams, Reg643232Params, RegImm32AddParams, RegImmCarryNoSetFlagsParams,
    RegImmCarryParams, RegImmParams, SRType, SatParams, SetFlags,
};
#[cfg(feature = "has-dsp-ext")]
use crate::core::instruction::{Reg3HighParams, Reg3UsizeParams, Reg4HighParams};

#[cfg(feature = "has-fp")]
use crate::core::instruction::{
    VAddSubParamsf32, VAddSubParamsf64, VCVTParams, VCVTParamsF32F64, VCVTParamsF64F32,
    VCmpParamsf32, VLoadAndStoreParams, VMRSTarget, VMovCr2DpParams, VMovCr2Sp2Params,
    VMovCrScalarParams, VMovCrSpParams, VMovImmParams32, VMovImmParams64, VMovRegParamsf32,
    VMovRegParamsf64, VSelParamsf32,
};
#[cfg(feature = "has-fp")]
use crate::core::register::{DoubleReg, ExtensionReg, SingleReg};

use super::*;

use crate::core::register::Reg;

mod branch_control;
mod data_proc;
#[cfg(feature = "has-fp")]
mod floating_point;
#[cfg(feature = "has-fp")]
mod fp_registers;
mod fundamentals;
mod load_store;
mod multiply_divide;
mod saturation_pack_misc;
mod system_barrier;
mod unpredictable;

/// The encodings the manual calls UNPREDICTABLE decode as undefined, like BFI with `msbit < lsbit`.
fn assert_udf_32(opcode: u32) {
    match decode_32(opcode) {
        Instruction::UDF { thumb32, .. } => assert!(thumb32),
        other => panic!("expected UDF for {opcode:#010x}, got {other:?}"),
    }
}

/// Decodes every halfword offset of a firmware image and writes one line each, to compare what a
/// change to the decoder alters. `ZMU_DUMP_IMAGE` is the image, `ZMU_DUMP_OUT` the file to write.
#[test]
#[ignore]
fn dump_the_decode_of_an_image() {
    use std::io::Write;
    let (Ok(image), Ok(out)) = (
        std::env::var("ZMU_DUMP_IMAGE"),
        std::env::var("ZMU_DUMP_OUT"),
    ) else {
        return;
    };
    let image = std::fs::read(image).unwrap();
    let mut out = std::io::BufWriter::new(std::fs::File::create(out).unwrap());
    for offset in (0..image.len().saturating_sub(4)).step_by(2) {
        let first = u16::from_le_bytes([image[offset], image[offset + 1]]);
        let instruction = if is_thumb32(first) {
            let second = u16::from_le_bytes([image[offset + 2], image[offset + 3]]);
            let opcode = u32::from(first) << 16 | u32::from(second);
            format!("{opcode:08x} {:?}", decode_32(opcode))
        } else {
            format!("{first:04x} {:?}", decode_16(first))
        };
        writeln!(out, "{offset:08x} {instruction}").unwrap();
    }
}
