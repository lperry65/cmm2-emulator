mod args;
mod atomic_write;
mod batch;
mod board;
mod error;
mod inspect;
mod keys;
mod launch;
mod live;
mod local_clock;
mod machine;
mod output;
mod pacing;
mod panic_hook;
mod report;
mod run_and_save;
#[cfg(test)]
mod run_id;
mod saveable;
mod script;
mod signals;
mod slice;
mod storage;
mod terminal;
#[cfg(test)]
mod test_support;
mod trace;
mod usb_keys;
mod window;

use std::io::{self, Write};
use std::panic::{catch_unwind, resume_unwind};
use std::process::ExitCode;

use args::Command;
use error::{RunError, say};
use inspect::{show_register, show_vectors};
use output::Output;
use panic_hook::{install_panic_hook, say_late_panic};

fn main() -> ExitCode {
    signals::catch();
    install_panic_hook();
    let result = catch_unwind(run);
    // A panic's message that was held back (see `install_panic_hook`) is said now at the latest.
    say_late_panic(&mut Output::stderr());
    let Ok(code) = result else {
        resume_unwind(result.unwrap_err());
    };
    // A run that a signal ended exits by that signal, once everything is written and said.
    let _ = io::stdout().flush();
    signals::die_if_caught();
    code
}

fn run() -> ExitCode {
    let command = match args::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Version => {
            println!("cmm2-emulator {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Lookup(addr) => {
            show_register(addr);
            ExitCode::SUCCESS
        }
        Command::Vectors(path) => finish(show_vectors(&path)),
        Command::Run { start, options } => finish(launch::run(start, options)),
    }
}

/// Exit 0 when the run ended normally, 1 on a fault or any error.
fn finish(result: Result<bool, RunError>) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            say(err);
            ExitCode::from(1)
        }
    }
}
