//! The commands that only look at something and run nothing: `--firmware` alone (it prints the
//! vector table) and `--lookup`.

use crate::error::RunError;
use cmm2_core::{Firmware, Svd};

pub(crate) fn show_vectors(path: &str) -> Result<bool, RunError> {
    let firmware = Firmware::load(path)?;
    let vectors = firmware.vector_table();
    println!("initial SP {:#010X}", vectors.initial_sp);
    println!("reset handler {:#010X}", vectors.reset_handler);
    Ok(true)
}

pub(crate) fn show_register(addr: u32) {
    let Some(found) = Svd::stm32h743().lookup(addr) else {
        println!("unknown address {addr:#010X}");
        return;
    };
    print!("{} {:#010X}", found.name, found.start);
    if found.offset != 0 {
        print!(" +{}", found.offset);
    }
    println!();
    for field in found.fields {
        if field.bit_width == 1 {
            println!("  {} bit {}", field.name, field.bit_offset);
        } else {
            let high = field.bit_offset + field.bit_width - 1;
            println!("  {} bits {}-{}", field.name, field.bit_offset, high);
        }
    }
}
