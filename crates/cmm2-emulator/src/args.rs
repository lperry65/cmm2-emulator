//! The command line, parsed by hand: the options are few and fixed.

use std::fmt;

pub const USAGE: &str = "usage: cmm2-emulator --version | --lookup <address> | --firmware <file> [--run <count>] [--stop-at <address>] [--trace <file>] [--trace-mmio]";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Version,
    Lookup(u32),
    /// Print the vector table.
    Vectors(String),
    Run {
        firmware: String,
        options: RunOptions,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// How many instructions to run; without it the run goes on until `stop_at` or a fault.
    pub run: Option<u64>,
    pub stop_at: Option<u32>,
    pub trace: Option<String>,
    pub trace_mmio: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    /// Nothing to complain about by name: no arguments, or an option without its value.
    Usage,
    Unknown(String),
    BadCount(String),
    BadAddress(String),
    NeedsFirmware(String),
    NothingToRun,
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgError::Usage => {}
            ArgError::Unknown(arg) => writeln!(f, "cmm2-emulator: unknown argument '{arg}'")?,
            ArgError::BadCount(arg) => writeln!(f, "cmm2-emulator: bad count '{arg}'")?,
            ArgError::BadAddress(arg) => writeln!(f, "cmm2-emulator: bad address '{arg}'")?,
            ArgError::NeedsFirmware(option) => {
                writeln!(f, "cmm2-emulator: {option} needs --firmware")?
            }
            ArgError::NothingToRun => {
                writeln!(f, "cmm2-emulator: nothing to run, give --run or --stop-at")?
            }
        }
        write!(f, "{USAGE}")
    }
}

impl std::error::Error for ArgError {}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, ArgError> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None => Err(ArgError::Usage),
        Some("--version") => Ok(Command::Version),
        Some("--lookup") => {
            let arg = args.next().ok_or(ArgError::Usage)?;
            parse_address(&arg).map(Command::Lookup)
        }
        Some("--firmware") => {
            let firmware = args.next().ok_or(ArgError::Usage)?;
            let options = parse_run_options(args)?;
            if options == RunOptions::default() {
                return Ok(Command::Vectors(firmware));
            }
            if options.run.is_none() && options.stop_at.is_none() {
                return Err(ArgError::NothingToRun);
            }
            Ok(Command::Run { firmware, options })
        }
        Some(option @ ("--run" | "--stop-at" | "--trace" | "--trace-mmio")) => {
            Err(ArgError::NeedsFirmware(option.to_string()))
        }
        Some(arg) => Err(ArgError::Unknown(arg.to_string())),
    }
}

fn parse_run_options(mut args: impl Iterator<Item = String>) -> Result<RunOptions, ArgError> {
    let mut options = RunOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--run" => options.run = Some(parse_count(&args.next().ok_or(ArgError::Usage)?)?),
            "--stop-at" => {
                options.stop_at = Some(parse_address(&args.next().ok_or(ArgError::Usage)?)?)
            }
            "--trace" => options.trace = Some(args.next().ok_or(ArgError::Usage)?),
            "--trace-mmio" => options.trace_mmio = true,
            _ => return Err(ArgError::Unknown(arg)),
        }
    }
    Ok(options)
}

/// Decimal, or hex with a `0x` prefix.
fn parse_count(arg: &str) -> Result<u64, ArgError> {
    let count = match arg.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => arg.parse(),
    };
    count.map_err(|_| ArgError::BadCount(arg.to_string()))
}

fn parse_address(arg: &str) -> Result<u32, ArgError> {
    arg.strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .ok_or_else(|| ArgError::BadAddress(arg.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, ArgError> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn firmware_alone_shows_the_vectors() {
        assert_eq!(
            parse_args(&["--firmware", "f.bin"]),
            Ok(Command::Vectors("f.bin".into()))
        );
    }

    #[test]
    fn run_options_come_in_any_order() {
        let want = Command::Run {
            firmware: "f.bin".into(),
            options: RunOptions {
                run: Some(0x10),
                stop_at: Some(0x0800_0100),
                trace: Some("t.txt".into()),
                trace_mmio: true,
            },
        };
        assert_eq!(
            parse_args(&[
                "--firmware",
                "f.bin",
                "--trace-mmio",
                "--stop-at",
                "0x08000100",
                "--trace",
                "t.txt",
                "--run",
                "0x10",
            ]),
            Ok(want)
        );
    }

    #[test]
    fn counts_are_decimal_or_hex() {
        assert_eq!(parse_count("100000"), Ok(100_000));
        assert_eq!(parse_count("0x1F"), Ok(31));
        assert_eq!(parse_count("0x"), Err(ArgError::BadCount("0x".into())));
        assert_eq!(parse_count("-1"), Err(ArgError::BadCount("-1".into())));
    }

    #[test]
    fn addresses_need_the_hex_prefix() {
        assert_eq!(parse_address("0x0809DF7C"), Ok(0x0809_DF7C));
        assert_eq!(
            parse_address("1234"),
            Err(ArgError::BadAddress("1234".into()))
        );
    }

    #[test]
    fn trace_alone_has_nothing_to_run() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--trace-mmio"]),
            Err(ArgError::NothingToRun)
        );
    }
}
