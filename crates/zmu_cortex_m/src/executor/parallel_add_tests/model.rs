//! The encoder and the reference model.

/// The encoding from the diagrams: 1111 1010 1 op1 Rn / 1111 Rd 0 U op2 Rm. op1 comes from the
/// base operation (Tables A5-25 and A5-26), U from the sign, and op2 from the kind.
pub(super) fn encode(name: &str, rd: u32, rn: u32, rm: u32) -> u32 {
    let (prefix, base) = split(name);
    let op1 = match base {
        "add8" => 0b000,
        "add16" => 0b001,
        "asx" => 0b010,
        "sub8" => 0b100,
        "sub16" => 0b101,
        "sax" => 0b110,
        _ => unreachable!(),
    };
    let (unsigned, op2) = match prefix {
        "s" => (0, 0b00),
        "q" => (0, 0b01),
        "sh" => (0, 0b10),
        "u" => (1, 0b00),
        "uq" => (1, 0b01),
        "uh" => (1, 0b10),
        _ => unreachable!(),
    };
    (0xfa80 | op1 << 4 | rn) << 16 | 0xf000 | rd << 8 | unsigned << 6 | op2 << 4 | rm
}

pub(super) fn split(name: &str) -> (&str, &str) {
    let prefix_length = ["uq", "uh", "sh", "s", "q", "u"]
        .iter()
        .find(|prefix| name.starts_with(**prefix))
        .map(|prefix| prefix.len())
        .unwrap();
    name.split_at(prefix_length)
}

/// The manual's pseudocode, lane by lane. Lane 0 is the lowest. Each lane gives the sum or
/// difference the pseudocode names (so `add` says whether it is a sum), before it is wrapped,
/// saturated or halved.
pub(super) fn model(name: &str, rn: u32, rm: u32) -> (u32, Option<u32>) {
    let (prefix, base) = split(name);
    let width: u32 = if base.ends_with('8') { 8 } else { 16 };
    let signed = prefix.starts_with('s') || prefix == "q";
    let lanes = |x: u32| -> Vec<i64> {
        (0..32 / width)
            .map(|i| {
                let raw = i64::from((x >> (i * width)) & ((1 << width) - 1));
                if signed && raw >= 1 << (width - 1) {
                    raw - (1 << width)
                } else {
                    raw
                }
            })
            .collect()
    };
    let (n, m) = (lanes(rn), lanes(rm));
    let values: Vec<(i64, bool)> = match base {
        "add8" | "add16" => n.iter().zip(&m).map(|(a, b)| (a + b, true)).collect(),
        "sub8" | "sub16" => n.iter().zip(&m).map(|(a, b)| (a - b, false)).collect(),
        // diff = Rn<15:0> - Rm<31:16>, sum = Rn<31:16> + Rm<15:0>
        "asx" => vec![(n[0] - m[1], false), (n[1] + m[0], true)],
        // sum = Rn<15:0> + Rm<31:16>, diff = Rn<31:16> - Rm<15:0>
        "sax" => vec![(n[0] + m[1], true), (n[1] - m[0], false)],
        _ => unreachable!(),
    };
    let (mut result, mut ge) = (0u32, 0u32);
    for (i, (value, add)) in values.into_iter().enumerate() {
        let modulus = 1i64 << width;
        let kept = match prefix {
            "s" | "u" => value.rem_euclid(modulus),
            "q" => value
                .clamp(-(modulus / 2), modulus / 2 - 1)
                .rem_euclid(modulus),
            "uq" => value.clamp(0, modulus - 1),
            // bits <width:1> of the sum
            "sh" | "uh" => (value.div_euclid(2)).rem_euclid(modulus),
            _ => unreachable!(),
        };
        result |= (kept as u32) << (i as u32 * width);
        let flag = if signed || !add {
            value >= 0
        } else {
            value >= modulus
        };
        if flag {
            // GE<i> for bytes, GE<2i+1:2i> for halfwords
            ge |= (if width == 8 { 0b1 } else { 0b11 }) << (i as u32 * width / 8);
        }
    }
    (
        result,
        if prefix == "s" || prefix == "u" {
            Some(ge)
        } else {
            None
        },
    )
}
