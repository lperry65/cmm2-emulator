use std::process::ExitCode;

use cmm2_core::{Firmware, Svd};

const USAGE: &str = "usage: cmm2-emulator --version | --firmware <file> | --lookup <address>";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--version"), _) => {
            println!("cmm2-emulator {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        (Some("--firmware"), Some(path)) => show_vectors(&path),
        (Some("--lookup"), Some(arg)) => match parse_address(&arg) {
            Some(addr) => {
                show_register(addr);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("cmm2-emulator: bad address '{arg}'\n{USAGE}");
                ExitCode::from(2)
            }
        },
        (Some("--firmware" | "--lookup"), None) | (None, _) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        (Some(arg), _) => {
            eprintln!("cmm2-emulator: unknown argument '{arg}'\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn show_vectors(path: &str) -> ExitCode {
    match Firmware::load(path) {
        Ok(firmware) => {
            let vectors = firmware.vector_table();
            println!("initial SP {:#010X}", vectors.initial_sp);
            println!("reset handler {:#010X}", vectors.reset_handler);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("cmm2-emulator: {err}");
            ExitCode::from(1)
        }
    }
}

fn parse_address(arg: &str) -> Option<u32> {
    let hex = arg.strip_prefix("0x")?;
    u32::from_str_radix(hex, 16).ok()
}

fn show_register(addr: u32) {
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
