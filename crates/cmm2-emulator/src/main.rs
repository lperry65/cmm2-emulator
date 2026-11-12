use std::process::ExitCode;

const USAGE: &str = "usage: cmm2-emulator --version";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("cmm2-emulator {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(arg) => {
            eprintln!("cmm2-emulator: unknown argument '{arg}'\n{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
