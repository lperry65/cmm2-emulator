//! The command line, parsed by hand: the options are few and fixed.

use std::fmt;

pub const USAGE: &str = "usage: cmm2-emulator --version | --lookup <address> | --firmware <file> [--run <count>] [--stop-at <address>] [--trace <file>] [--trace-mmio] [--console] [--on <text> <keys>]";

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
    pub console: bool,
    pub on: Vec<OnRule>,
}

/// Keys to send once the guest has printed some text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnRule {
    pub text: String,
    pub keys: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    /// Nothing to complain about by name: no arguments, or an option without its value.
    Usage,
    Unknown(String),
    BadCount(String),
    BadAddress(String),
    NeedsFirmware(String),
    NeedsConsole(String),
    ConsoleWithTrace,
    BadKeys(String),
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
            ArgError::NeedsConsole(option) => {
                writeln!(f, "cmm2-emulator: {option} needs --console")?
            }
            ArgError::ConsoleWithTrace => writeln!(
                f,
                "cmm2-emulator: --console cannot be traced, the screen belongs to the guest"
            )?,
            ArgError::BadKeys(arg) => writeln!(f, "cmm2-emulator: bad keys '{arg}'")?,
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
            check_console(&options)?;
            if !options.console && options.run.is_none() && options.stop_at.is_none() {
                return Err(ArgError::NothingToRun);
            }
            Ok(Command::Run { firmware, options })
        }
        Some(
            option @ ("--run" | "--stop-at" | "--trace" | "--trace-mmio" | "--console" | "--on"),
        ) => Err(ArgError::NeedsFirmware(option.to_string())),
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
            "--console" => options.console = true,
            "--on" => {
                let text = args.next().ok_or(ArgError::Usage)?;
                let keys = parse_keys(&args.next().ok_or(ArgError::Usage)?)?;
                options.on.push(OnRule { text, keys });
            }
            _ => return Err(ArgError::Unknown(arg)),
        }
    }
    Ok(options)
}

fn check_console(options: &RunOptions) -> Result<(), ArgError> {
    if options.console && (options.trace.is_some() || options.trace_mmio) {
        return Err(ArgError::ConsoleWithTrace);
    }
    if !options.console && !options.on.is_empty() {
        return Err(ArgError::NeedsConsole("--on".to_string()));
    }
    Ok(())
}

/// The keys of an `--on` rule: plain text, with `\r`, `\n`, `\t`, `\e`, `\\` and `\xHH` for the
/// rest.
fn parse_keys(arg: &str) -> Result<Vec<u8>, ArgError> {
    let bad = || ArgError::BadKeys(arg.to_string());
    let mut keys = Vec::new();
    let mut chars = arg.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            keys.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        keys.push(match chars.next().ok_or_else(bad)? {
            'r' => b'\r',
            'n' => b'\n',
            't' => b'\t',
            'e' => 0x1B,
            '\\' => b'\\',
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() != 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(bad());
                }
                u8::from_str_radix(&hex, 16).map_err(|_| bad())?
            }
            _ => return Err(bad()),
        });
    }
    Ok(keys)
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
                ..RunOptions::default()
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

    #[test]
    fn console_runs_without_a_limit() {
        let want = Command::Run {
            firmware: "f.bin".into(),
            options: RunOptions {
                console: true,
                ..RunOptions::default()
            },
        };
        assert_eq!(parse_args(&["--firmware", "f.bin", "--console"]), Ok(want));
    }

    #[test]
    fn console_takes_a_limit_too() {
        let Ok(Command::Run { options, .. }) =
            parse_args(&["--firmware", "f", "--console", "--run", "9"])
        else {
            panic!("not a run");
        };
        assert!(options.console);
        assert_eq!(options.run, Some(9));
    }

    #[test]
    fn on_is_repeatable_and_keeps_the_order() {
        let Ok(Command::Run { options, .. }) = parse_args(&[
            "--firmware",
            "f",
            "--console",
            "--on",
            "Keyboard type",
            "1",
            "--on",
            "HH:MM:SS",
            "12:00:00\\r",
        ]) else {
            panic!("not a run");
        };
        assert_eq!(
            options.on,
            vec![
                OnRule {
                    text: "Keyboard type".into(),
                    keys: b"1".to_vec()
                },
                OnRule {
                    text: "HH:MM:SS".into(),
                    keys: b"12:00:00\r".to_vec()
                },
            ]
        );
    }

    #[test]
    fn on_needs_two_values() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--on"]),
            Err(ArgError::Usage)
        );
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--on", "text"]),
            Err(ArgError::Usage)
        );
    }

    #[test]
    fn on_needs_the_console() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--run", "1", "--on", "a", "b"]),
            Err(ArgError::NeedsConsole("--on".into()))
        );
    }

    #[test]
    fn console_and_on_need_the_firmware() {
        assert_eq!(
            parse_args(&["--console"]),
            Err(ArgError::NeedsFirmware("--console".into()))
        );
        assert_eq!(
            parse_args(&["--on", "a", "b"]),
            Err(ArgError::NeedsFirmware("--on".into()))
        );
    }

    #[test]
    fn console_cannot_be_traced() {
        for trace in [&["--trace", "t"][..], &["--trace-mmio"][..]] {
            let mut args = vec!["--firmware", "f", "--console"];
            args.extend_from_slice(trace);
            assert_eq!(parse_args(&args), Err(ArgError::ConsoleWithTrace));
        }
    }

    #[test]
    fn a_bad_key_escape_is_reported_with_its_option_value() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--on", "a", "\\q"]),
            Err(ArgError::BadKeys("\\q".into()))
        );
    }

    #[test]
    fn key_escapes() {
        assert_eq!(parse_keys("\\r"), Ok(vec![b'\r']));
        assert_eq!(parse_keys("\\n"), Ok(vec![b'\n']));
        assert_eq!(parse_keys("\\t"), Ok(vec![b'\t']));
        assert_eq!(parse_keys("\\e"), Ok(vec![0x1B]));
        assert_eq!(parse_keys("\\\\"), Ok(vec![b'\\']));
        assert_eq!(parse_keys("\\x41"), Ok(vec![b'A']));
        assert_eq!(parse_keys("a\\x1bb\\r"), Ok(vec![b'a', 0x1B, b'b', b'\r']));
    }

    #[test]
    fn plain_keys_are_taken_as_they_are() {
        assert_eq!(parse_keys("12:00:00"), Ok(b"12:00:00".to_vec()));
        assert_eq!(parse_keys(""), Ok(Vec::new()));
    }

    #[test]
    fn bad_key_escapes_are_errors() {
        for bad in ["\\q", "\\x4", "\\x", "\\xZZ", "ab\\"] {
            assert_eq!(parse_keys(bad), Err(ArgError::BadKeys(bad.into())), "{bad}");
        }
    }
}
