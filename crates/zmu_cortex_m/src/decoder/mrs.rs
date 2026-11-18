use crate::core::{
    bits::Bits,
    instruction::{Instruction, MrsParams},
    register::Reg,
};

/// The special registers MRS and MSR have: `SYSm IN {0..3,5..9,16..20}` (B5.2.2, B5.2.3), which
/// are APSR, IAPSR, EAPSR, XPSR, IPSR, EPSR, IEPSR, MSP, PSP, PRIMASK, BASEPRI, `BASEPRI_MAX`,
/// FAULTMASK and CONTROL. Any other `SYSm` is UNPREDICTABLE.
#[cfg(not(feature = "armv6m"))]
pub(super) fn is_special_register(sysm: u32) -> bool {
    matches!(sysm, 0..=3 | 5..=9 | 16..=20)
}

/// Armv6-M has no BASEPRI, `BASEPRI_MAX` or FAULTMASK: its set is `{0..3,5..9,16,20}`
#[cfg(feature = "armv6m")]
pub(super) fn is_special_register(sysm: u32) -> bool {
    matches!(sysm, 0..=3 | 5..=9 | 16 | 20)
}

/// MRS (A7.7.82, B5.2.2), the one place Rd = PC is refused. Rd = SP or PC, and a `SYSm` that is not a special register, are
/// UNPREDICTABLE and decode as UDF.
#[allow(non_snake_case)]
pub fn decode_MRS_t1(opcode: u32) -> Instruction {
    let reg_rd = opcode.get_bits(8..12);
    let sysm = opcode.get_bits(0..8);
    if reg_rd == 13 || reg_rd == 15 || !is_special_register(sysm) {
        return super::decode_UDF_t2(opcode);
    }
    Instruction::MRS {
        params: MrsParams {
            rd: Reg::from(reg_rd as u8),
            sysm: sysm as u8,
        },
    }
}
