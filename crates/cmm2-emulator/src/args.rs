//! The command line, parsed by hand: the options are few and fixed.

use crate::error::shown;
use std::borrow::Cow;
use std::fmt;

pub const USAGE: &str = "usage: cmm2-emulator [--firmware <file>] [--run <count>] [--stop-at <address>] [--trace <file>] [--trace-mmio] [--trace-from <count>] [--console] [--on <text> <keys>] [--screenshot <file>] [--window] [--console-keys] [--unpaced] [--flash <file>] [--sd <file> | --no-sd] [--burn <file>] [--rng-seed <number>] | --version | --lookup <address>";

// One of these is made per process, so the size of the run variant costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Version,
    Lookup(u32),
    /// Print the vector table of a firmware image: `--firmware` alone.
    Vectors(String),
    Run {
        start: Start,
        options: RunOptions,
    },
}

/// Which machine a run is of.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// The board, as it was last left: no `--firmware`.
    Board,
    /// A scratch machine from the image in the file named by `--firmware`: nothing of the board
    /// is read or written, and `--flash` and `--sd` are its only files.
    File(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// How many instructions to run; without it the run goes on until `stop_at` or a fault.
    pub run: Option<u64>,
    pub stop_at: Option<u32>,
    pub trace: Option<String>,
    pub trace_mmio: bool,
    /// The instruction count at which the traces start; without it they start at once.
    pub trace_from: Option<u64>,
    pub console: bool,
    pub on: Vec<OnRule>,
    /// Where to write the screen as a PPM file when the run ends.
    pub screenshot: Option<String>,
    /// Show the screen in a window while the guest runs.
    pub window: bool,
    /// The window is on because the board's start has one unless told otherwise, and not
    /// because `--window` was given.
    pub window_by_default: bool,
    /// The window's keys go to the console, as bytes, and not to the USB keyboard.
    pub console_keys: bool,
    /// Run the live modes as fast as the host allows, not at the machine's own speed.
    pub unpaced: bool,
    /// A file that keeps the flash the user changes, and the clock, between runs: loaded at the start if it exists, written
    /// back at the end.
    pub flash: Option<String>,
    /// A card image for the SD socket, which has to exist already and is written back at the
    /// end if the guest changed the card.
    pub sd: Option<String>,
    /// Takes the card out of the socket, so that the board no longer remembers one.
    pub no_sd: bool,
    /// An image to burn onto the board before it starts: copied into the board's directory.
    pub burn: Option<String>,
    /// Fixes the RNG's values, so that a run repeats; without it they come from the host's
    /// entropy and differ from run to run, as the chip's do.
    pub rng_seed: Option<u64>,
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
    BadSeed(String),
    BadAddress(String),
    /// An option that is only for the board, with `--firmware`, which is a machine of its own.
    BoardOnly(String),
    /// `--burn` puts a firmware on the board and `--firmware` runs one that is not on it.
    BurnAndFirmware,
    NeedsConsole(String),
    NeedsWindow(String),
    ConsoleMmioNeedsFile,
    TraceFromNeedsTrace,
    BadKeys(String),
    /// A file option was given an empty name, as an unset shell variable gives.
    EmptyName(String),
    /// `--sd` puts a card in and `--no-sd` takes it out, so both is a contradiction.
    SdAndNoSd,
    /// An option that names the one file that a run burns, given twice.
    Repeated(String),
    NothingToRun,
}

/// `text` as it reads inside the single quotes a message puts round it: `shown` once for the
/// control characters, then every `'` doubled, so that a name carrying one cannot end the quoted
/// name early and the last quote on the line is the one that ends it. Doubled, not backslashed:
/// inside single quotes a backslash is a character of the name, so a backslashed quote would read
/// as a name with a backslash in it. `command_quoted` is not reused because it makes the shape a
/// shell needs — the quote closed, escaped and opened again, and `./` in front of a leading dash —
/// and this is a line a person reads, not one a shell runs.
///
/// `shown` is not touched, and a name with no quote in it comes back exactly as `shown` made it.
fn in_quotes(text: &str) -> Cow<'_, str> {
    let escaped = shown(text);
    if escaped.contains('\'') {
        Cow::Owned(escaped.replace('\'', "''"))
    } else {
        escaped
    }
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // An argument echoed back is a name a person reads, not one offered as a command, so the
        // arms that say one say it through `in_quotes` (see the strategy there): escaped once and
        // any quote doubled, here, from what `parse` handed in raw. The arms that name an option
        // say a word this file wrote itself, so they have nothing to escape.
        match self {
            ArgError::Usage => {}
            ArgError::Unknown(arg) => {
                writeln!(f, "cmm2-emulator: unknown argument '{}'", in_quotes(arg))?
            }
            ArgError::BadCount(arg) => {
                writeln!(f, "cmm2-emulator: bad count '{}'", in_quotes(arg))?
            }
            ArgError::BadSeed(arg) => writeln!(f, "cmm2-emulator: bad seed '{}'", in_quotes(arg))?,
            ArgError::BadAddress(arg) => {
                writeln!(f, "cmm2-emulator: bad address '{}'", in_quotes(arg))?
            }
            ArgError::BoardOnly(option) => writeln!(
                f,
                "cmm2-emulator: {option} is for the board, which --firmware does not use"
            )?,
            ArgError::BurnAndFirmware => writeln!(
                f,
                "cmm2-emulator: --burn puts the firmware on the board and --firmware runs one \
                 that is not on it, not both"
            )?,
            ArgError::NeedsConsole(option) => {
                writeln!(f, "cmm2-emulator: {option} needs --console")?
            }
            ArgError::NeedsWindow(option) => writeln!(f, "cmm2-emulator: {option} needs --window")?,
            ArgError::ConsoleMmioNeedsFile => writeln!(
                f,
                "cmm2-emulator: --console with --trace-mmio needs --trace, the screen belongs to the guest"
            )?,
            ArgError::TraceFromNeedsTrace => writeln!(
                f,
                "cmm2-emulator: --trace-from needs --trace or --trace-mmio"
            )?,
            ArgError::BadKeys(arg) => writeln!(f, "cmm2-emulator: bad keys '{}'", in_quotes(arg))?,
            ArgError::EmptyName(option) => writeln!(
                f,
                "cmm2-emulator: {option} needs a file name, not an empty one"
            )?,
            ArgError::Repeated(option) => writeln!(
                f,
                "cmm2-emulator: {option} is given twice, which one is meant?"
            )?,
            ArgError::NothingToRun => {
                writeln!(f, "cmm2-emulator: nothing to run, give --run or --stop-at")?
            }
            ArgError::SdAndNoSd => writeln!(
                f,
                "cmm2-emulator: --sd puts a card in the socket and --no-sd takes it out, not both"
            )?,
        }
        write!(f, "{USAGE}")
    }
}

impl std::error::Error for ArgError {}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, ArgError> {
    let args: Vec<String> = args.into_iter().collect();
    let second = || args.get(1).ok_or(ArgError::Usage);
    match args.first().map(String::as_str) {
        Some("--version") => Ok(Command::Version),
        Some("--lookup") => parse_address(second()?).map(Command::Lookup),
        _ => parse_run(args),
    }
}

/// A run of the board (no `--firmware`), or of the image that `--firmware` names, which is a
/// machine of its own whatever else is on the line.
fn parse_run(args: Vec<String>) -> Result<Command, ArgError> {
    let (firmware, mut options) = parse_run_options(args.into_iter())?;
    if firmware.is_some() && options.burn.is_some() {
        return Err(ArgError::BurnAndFirmware);
    }
    if options.sd.is_some() && options.no_sd {
        return Err(ArgError::SdAndNoSd);
    }
    let Some(file) = firmware else {
        // Switching a board on shows its screen, unless the run is for a terminal (`--console`)
        // or for a number of instructions or an address (`--run`, `--stop-at`).
        if !options.console && options.run.is_none() && options.stop_at.is_none() {
            options.window_by_default = !options.window;
            options.window = true;
        }
        check_console(&options)?;
        check_trace_from(&options)?;
        return Ok(Command::Run {
            start: Start::Board,
            options,
        });
    };
    if options.no_sd {
        return Err(ArgError::BoardOnly("--no-sd".to_string()));
    }
    if options == RunOptions::default() {
        return Ok(Command::Vectors(file));
    }
    check_console(&options)?;
    check_trace_from(&options)?;
    if !options.console && !options.window && options.run.is_none() && options.stop_at.is_none() {
        return Err(ArgError::NothingToRun);
    }
    Ok(Command::Run {
        start: Start::File(file),
        options,
    })
}

/// The value of a file option, which has to be there and cannot be empty (an unset shell
/// variable gives that, and it would only be found out when the run was over).
fn file_name(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, ArgError> {
    match args.next() {
        None => Err(ArgError::Usage),
        Some(name) if name.is_empty() => Err(ArgError::EmptyName(option.to_string())),
        Some(name) => Ok(name),
    }
}

fn parse_run_options(
    mut args: impl Iterator<Item = String>,
) -> Result<(Option<String>, RunOptions), ArgError> {
    let mut options = RunOptions::default();
    let mut firmware = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // A second `--firmware` is "unknown", as it was before `--firmware` could come
            // anywhere, and an empty name is loaded (and fails) as it was; `--burn`, which is new,
            // says what is wrong.
            "--firmware" if firmware.is_some() => return Err(ArgError::Unknown(arg)),
            "--firmware" => firmware = Some(args.next().ok_or(ArgError::Usage)?),
            "--burn" if options.burn.is_some() => return Err(ArgError::Repeated(arg)),
            "--run" => options.run = Some(parse_count(&args.next().ok_or(ArgError::Usage)?)?),
            "--stop-at" => {
                options.stop_at = Some(parse_address(&args.next().ok_or(ArgError::Usage)?)?)
            }
            "--trace" => options.trace = Some(file_name(&mut args, "--trace")?),
            "--trace-mmio" => options.trace_mmio = true,
            "--trace-from" => {
                options.trace_from = Some(parse_count(&args.next().ok_or(ArgError::Usage)?)?)
            }
            "--console" => options.console = true,
            "--window" => options.window = true,
            "--console-keys" => options.console_keys = true,
            "--unpaced" => options.unpaced = true,
            "--flash" => options.flash = Some(file_name(&mut args, "--flash")?),
            "--sd" => options.sd = Some(file_name(&mut args, "--sd")?),
            "--no-sd" => options.no_sd = true,
            "--burn" => options.burn = Some(file_name(&mut args, "--burn")?),
            "--rng-seed" => {
                let seed = args.next().ok_or(ArgError::Usage)?;
                options.rng_seed = Some(parse_count(&seed).map_err(|_| ArgError::BadSeed(seed))?)
            }
            "--screenshot" => options.screenshot = Some(file_name(&mut args, "--screenshot")?),
            "--on" => {
                let text = args.next().ok_or(ArgError::Usage)?;
                let keys = parse_keys(&args.next().ok_or(ArgError::Usage)?)?;
                options.on.push(OnRule { text, keys });
            }
            _ => return Err(ArgError::Unknown(arg)),
        }
    }
    Ok((firmware, options))
}

fn check_console(options: &RunOptions) -> Result<(), ArgError> {
    if options.console && options.trace_mmio && options.trace.is_none() {
        return Err(ArgError::ConsoleMmioNeedsFile);
    }
    if !options.console && !options.on.is_empty() {
        return Err(ArgError::NeedsConsole("--on".to_string()));
    }
    if options.console_keys && !options.window {
        return Err(ArgError::NeedsWindow("--console-keys".to_string()));
    }
    Ok(())
}

fn check_trace_from(options: &RunOptions) -> Result<(), ArgError> {
    if options.trace_from.is_some() && options.trace.is_none() && !options.trace_mmio {
        return Err(ArgError::TraceFromNeedsTrace);
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

    // An echoed argument is a name a person reads, so `Display` says it through `shown` once.
    // Before that was so it went to the terminal raw: measured as two escape bytes and a broken
    // line with the real binary (`a_bad_argument_is_echoed_escaped_and_on_one_line` in
    // `tests/usage.rs`). Every arm that echoes one is driven with a hostile argument in
    // `an_echoed_argument_puts_no_control_character_on_a_line` below; this keeps its two edges: an
    // argument of one backslash, which is doubled once and not twice, and one that needs no escape,
    // which is echoed as it was typed.
    #[test]
    fn an_echoed_argument_is_escaped_once() {
        let first_line = |err: ArgError| err.to_string().lines().next().unwrap().to_string();
        // A backslash is doubled once, never twice: `parse` hands an arm what argv held, so no
        // argument reaches these arms already escaped.
        assert_eq!(
            first_line(ArgError::Unknown(r"back\slash".into())),
            r"cmm2-emulator: unknown argument 'back\\slash'"
        );
        // A name that needs nothing is echoed exactly as it was typed.
        assert_eq!(
            first_line(ArgError::Unknown("--bogus".into())),
            "cmm2-emulator: unknown argument '--bogus'"
        );
    }

    // The fence over `ArgError`, the twin of the one over `RunError` in `error/tests.rs` and kept
    // a copy rather than generalised, so that breaking either one on purpose is a small edit. It
    // has the same three nets: a sample of every variant with a count pin, a classifier that is a
    // match with no wildcard, and a hostile drive of every arm that says what a person typed. A
    // new variant is a compile error in `says` until it is classified, and a red count until it has
    // a sample in `every_variant`.
    use crate::test_support::{HOSTILE_NAME, HOSTILE_SHOWN};

    /// One sample of every variant of `ArgError`, and no variant may be missing from it.
    const ARG_ERRORS: usize = 16;
    /// The arms that echo an argument as it was typed.
    const ECHOES_AN_ARGUMENT: usize = 5;
    /// The arms that say an option's name, which is a word this file wrote.
    const NAMES_AN_OPTION: usize = 5;
    /// The arms that say nothing a caller can affect.
    const SAYS_NOTHING: usize = 6;

    /// What an arm of `ArgError` does with the text it carries.
    #[derive(Debug, PartialEq)]
    enum Says {
        /// Says an argument as it was typed, which is what `Display` escapes once through `shown`.
        AnArgument,
        /// Says an option's name, a word this file wrote, and says it raw. `BoardOnly`,
        /// `NeedsConsole`, `NeedsWindow` and `EmptyName` are handed a literal where they are
        /// made (`EmptyName` through `file_name`, whose five callers all pass one). `Repeated`
        /// is handed `arg`, and only the guard on its `match` arm makes that `"--burn"`. Safe
        /// while both hold; a flagged item, not one this fence can secure.
        AnOptionName,
        /// Says nothing a caller can affect.
        Nothing,
    }

    /// Which of the three an arm is, by a match with no wildcard: a variant added to `ArgError` is
    /// a compile error here until it is classified.
    fn says(err: &ArgError) -> Says {
        match err {
            ArgError::Unknown(_)
            | ArgError::BadCount(_)
            | ArgError::BadSeed(_)
            | ArgError::BadAddress(_)
            | ArgError::BadKeys(_) => Says::AnArgument,
            ArgError::BoardOnly(_)
            | ArgError::NeedsConsole(_)
            | ArgError::NeedsWindow(_)
            | ArgError::EmptyName(_)
            | ArgError::Repeated(_) => Says::AnOptionName,
            ArgError::Usage
            | ArgError::BurnAndFirmware
            | ArgError::ConsoleMmioNeedsFile
            | ArgError::TraceFromNeedsTrace
            | ArgError::SdAndNoSd
            | ArgError::NothingToRun => Says::Nothing,
        }
    }

    /// One sample of every variant, with `HOSTILE_NAME` in every slot that holds what a person
    /// typed. The slots that hold an option's name hold the literal their call site passes, so
    /// that a sample of an arm that says no name cannot carry one by accident.
    fn every_variant() -> Vec<(&'static str, ArgError)> {
        let typed = HOSTILE_NAME.to_string();
        vec![
            ("usage", ArgError::Usage),
            ("unknown", ArgError::Unknown(typed.clone())),
            ("bad count", ArgError::BadCount(typed.clone())),
            ("bad seed", ArgError::BadSeed(typed.clone())),
            ("bad address", ArgError::BadAddress(typed.clone())),
            ("board only", ArgError::BoardOnly("--no-sd".to_string())),
            ("burn and firmware", ArgError::BurnAndFirmware),
            ("needs console", ArgError::NeedsConsole("--on".to_string())),
            (
                "needs window",
                ArgError::NeedsWindow("--console-keys".to_string()),
            ),
            ("console mmio needs file", ArgError::ConsoleMmioNeedsFile),
            ("trace from needs trace", ArgError::TraceFromNeedsTrace),
            ("bad keys", ArgError::BadKeys(typed.clone())),
            ("empty name", ArgError::EmptyName("--flash".to_string())),
            ("sd and no sd", ArgError::SdAndNoSd),
            ("repeated", ArgError::Repeated("--burn".to_string())),
            ("nothing to run", ArgError::NothingToRun),
        ]
    }

    /// What the process puts on standard error for `err`, which is the whole of `Display` said with
    /// `eprintln` and so ending in a line feed of its own. That is what makes an arm that says a
    /// message two lines: the message's, and the usage line's, with the line feed that ends the
    /// last. `Display` alone ends the message line and puts none after the usage line.
    fn as_printed(err: &ArgError) -> String {
        format!("{err}\n")
    }

    #[test]
    fn every_variant_of_an_argument_error_is_classified_and_counted() {
        let all = every_variant();
        assert_eq!(
            all.len(),
            ARG_ERRORS,
            "one sample for each variant of ArgError, and this count with it"
        );
        let counted = |kind: Says| all.iter().filter(|(_, err)| says(err) == kind).count();
        assert_eq!(
            counted(Says::AnArgument),
            ECHOES_AN_ARGUMENT,
            "the arms that echo an argument, and this count with them"
        );
        assert_eq!(
            counted(Says::AnOptionName),
            NAMES_AN_OPTION,
            "the arms that say an option's name, and this count with them"
        );
        assert_eq!(
            counted(Says::Nothing),
            SAYS_NOTHING,
            "the arms that say nothing of a caller's, and this count with them"
        );
        // A sample of an arm that says nothing, and one of an arm that says an option's name,
        // carries no name a caller typed, so the drive below cannot mistake one for an argument.
        for (what, err) in &all {
            let said = err.to_string();
            match says(err) {
                Says::Nothing => assert!(
                    !said.contains(HOSTILE_NAME) && !said.contains(HOSTILE_SHOWN),
                    "{what} is said to say nothing and says a name: {said:?}"
                ),
                Says::AnOptionName => {
                    assert!(
                        !said.contains(HOSTILE_NAME) && !said.contains(HOSTILE_SHOWN),
                        "{what} is said to name an option and says a name a caller typed: {said:?}"
                    )
                }
                Says::AnArgument => {}
            }
        }
    }

    /// The message line each echoing arm says of `HOSTILE_NAME`, written out here by hand and not
    /// built with `shown`: an escaping helper that agreed with itself would pass otherwise. The
    /// wildcard returns `None` for an arm that echoes nothing, and the test below fails an echoing
    /// arm that answers `None`, so a new one cannot be driven without an expectation.
    fn expected_line(err: &ArgError) -> Option<&'static str> {
        Some(match err {
            ArgError::Unknown(_) => r"cmm2-emulator: unknown argument 'evil\n\x1B[31mRED\x1B[0m'",
            ArgError::BadCount(_) => r"cmm2-emulator: bad count 'evil\n\x1B[31mRED\x1B[0m'",
            ArgError::BadSeed(_) => r"cmm2-emulator: bad seed 'evil\n\x1B[31mRED\x1B[0m'",
            ArgError::BadAddress(_) => r"cmm2-emulator: bad address 'evil\n\x1B[31mRED\x1B[0m'",
            ArgError::BadKeys(_) => r"cmm2-emulator: bad keys 'evil\n\x1B[31mRED\x1B[0m'",
            _ => return None,
        })
    }

    // The hostile drive: an argument carrying a colour change and a line feed, said by every arm
    // that echoes one. What it must never do is put a control character on a line, and what it must
    // say is the name escaped once, so that the reader can still see which argument was refused.
    #[test]
    fn an_echoed_argument_puts_no_control_character_on_a_line() {
        for (what, err) in every_variant() {
            // The classifier and the hand-written expectations cover the same arms, so neither one
            // can quietly lose an arm the other keeps.
            assert_eq!(
                says(&err) == Says::AnArgument,
                expected_line(&err).is_some(),
                "{what} is classified as echoing an argument if and only if it has an expectation"
            );
            if says(&err) != Says::AnArgument {
                continue;
            }
            let Some(line) = expected_line(&err) else {
                unreachable!("the assertion above holds for every arm that echoes")
            };
            let said = err.to_string();
            assert_eq!(said.lines().next(), Some(line), "{what}: {said:?}");
            // One line feed ends the message line, and the usage line after it has none of its
            // own; `as_printed` adds the second, and the real process is driven by
            // `tests/argfence.rs`.
            assert_eq!(said.matches('\n').count(), 1, "{what}: {said:?}");
            assert_eq!(
                as_printed(&err).matches('\n').count(),
                2,
                "{what}: {said:?}"
            );
            for rendered in said.lines() {
                assert!(
                    !rendered.chars().any(char::is_control),
                    "{what} put a control character on a line: {said:?}"
                );
            }
            // The name is still there to read, as one name: the escapes are visible text.
            assert!(said.contains(HOSTILE_SHOWN), "{what}: {said:?}");
        }
    }

    // A name inside the quotes a message puts round it must not read as ended early: a `'` in it is
    // doubled, so the last quote on the line is the one that ends the name. Before that was so,
    // `-x' --burn evil` was measured on the real binary as `unknown argument '-x' --burn evil'`,
    // where a reader sees the name end at `-x` and the rest as the message's own words. Every
    // expectation is written out here by hand, and every arm that echoes is covered by name, so a
    // new one fails the list rather than going untested.
    #[test]
    fn an_echoed_argument_with_a_quote_in_it_is_quoted_so_that_it_cannot_end_early() {
        let echoing: Vec<&str> = every_variant()
            .into_iter()
            .filter(|(_, err)| says(err) == Says::AnArgument)
            .map(|(what, _)| what)
            .collect();
        assert_eq!(
            echoing,
            [
                "unknown",
                "bad count",
                "bad seed",
                "bad address",
                "bad keys"
            ],
            "a case below for each arm that echoes an argument, and this list with it"
        );
        let first_line = |err: ArgError| err.to_string().lines().next().unwrap().to_string();
        // The measured case.
        assert_eq!(
            first_line(ArgError::Unknown("-x' --burn evil".into())),
            "cmm2-emulator: unknown argument '-x'' --burn evil'"
        );
        // One case for each of the other four arms. A name that ends in a quote is followed by
        // three of them: the doubled one, and the one the message puts round the name.
        assert_eq!(
            first_line(ArgError::BadCount("1'000".into())),
            "cmm2-emulator: bad count '1''000'"
        );
        assert_eq!(
            first_line(ArgError::BadSeed("''".into())),
            "cmm2-emulator: bad seed ''''''"
        );
        assert_eq!(
            first_line(ArgError::BadAddress("0x1'".into())),
            "cmm2-emulator: bad address '0x1'''"
        );
        assert_eq!(
            first_line(ArgError::BadKeys("it's".into())),
            "cmm2-emulator: bad keys 'it''s'"
        );
        // A control character and a quote together: escaped once, then doubled once, in that order.
        let both = ArgError::BadAddress("evil\n\u{1b}[31m'".into());
        let said = both.to_string();
        assert_eq!(
            said.lines().next().unwrap(),
            r"cmm2-emulator: bad address 'evil\n\x1B[31m'''"
        );
        assert_eq!(said.matches('\n').count(), 1, "{said:?}");
        for line in said.lines() {
            assert!(!line.chars().any(char::is_control), "{said:?}");
        }
        // A name with no quote in it is not changed at all.
        assert_eq!(
            first_line(ArgError::Unknown("--bogus".into())),
            "cmm2-emulator: unknown argument '--bogus'"
        );
    }

    #[test]
    fn run_options_come_in_any_order() {
        let want = Command::Run {
            start: Start::File("f.bin".into()),
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
            start: Start::File("f.bin".into()),
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
    fn window_runs_without_a_limit() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--window"]),
            Ok(Command::Run {
                start: Start::File("f".into()),
                options: RunOptions {
                    window: true,
                    ..RunOptions::default()
                }
            })
        );
        assert!(!run_options(&["--run", "9"]).window);
    }

    #[test]
    fn window_goes_with_the_console_and_a_limit() {
        let options = run_options(&["--window", "--console", "--run", "9"]);
        assert!(options.window && options.console);
        assert_eq!(options.run, Some(9));
    }

    #[test]
    fn the_window_sends_usb_keys_unless_console_keys_is_given() {
        assert!(!run_options(&["--window"]).console_keys);
        let options = run_options(&["--window", "--console-keys"]);
        assert!(options.window && options.console_keys);
    }

    #[test]
    fn console_keys_needs_the_window() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--console-keys"]),
            Err(ArgError::NeedsWindow("--console-keys".into()))
        );
        assert_eq!(
            parse_args(&["--firmware", "f", "--run", "9", "--console-keys"]),
            Err(ArgError::NeedsWindow("--console-keys".into()))
        );
    }

    #[test]
    fn rng_seed_takes_a_decimal_or_hex_number() {
        assert_eq!(
            run_options(&["--console", "--rng-seed", "42"]).rng_seed,
            Some(42)
        );
        assert_eq!(
            run_options(&["--console", "--rng-seed", "0xFFFFFFFFFFFFFFFF"]).rng_seed,
            Some(u64::MAX)
        );
        assert_eq!(run_options(&["--console"]).rng_seed, None);
    }

    #[test]
    fn rng_seed_must_be_a_number() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--rng-seed", "x"]),
            Err(ArgError::BadSeed("x".into()))
        );
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--rng-seed"]),
            Err(ArgError::Usage)
        );
    }

    #[test]
    fn rng_seed_alone_has_nothing_to_run() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--rng-seed", "1"]),
            Err(ArgError::NothingToRun)
        );
    }

    #[test]
    fn unpaced_turns_the_pacing_off() {
        assert!(run_options(&["--console", "--unpaced"]).unpaced);
        assert!(!run_options(&["--console"]).unpaced);
    }

    #[test]
    fn unpaced_alone_has_nothing_to_run() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--unpaced"]),
            Err(ArgError::NothingToRun)
        );
    }

    #[test]
    fn screenshot_takes_a_file() {
        let options = run_options(&["--run", "9", "--screenshot", "s.ppm"]);
        assert_eq!(options.screenshot.as_deref(), Some("s.ppm"));
        assert_eq!(run_options(&["--run", "9"]).screenshot, None);
        assert_eq!(
            parse_args(&["--firmware", "f", "--run", "9", "--screenshot"]),
            Err(ArgError::Usage)
        );
    }

    #[test]
    fn screenshot_alone_has_nothing_to_run() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--screenshot", "s.ppm"]),
            Err(ArgError::NothingToRun)
        );
    }

    #[test]
    fn flash_takes_a_file() {
        let options = run_options(&["--run", "9", "--flash", "f.flash"]);
        assert_eq!(options.flash.as_deref(), Some("f.flash"));
        assert_eq!(run_options(&["--run", "9"]).flash, None);
        assert_eq!(
            parse_args(&["--firmware", "f", "--run", "9", "--flash"]),
            Err(ArgError::Usage)
        );
    }

    #[test]
    fn flash_alone_has_nothing_to_run() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--flash", "f.flash"]),
            Err(ArgError::NothingToRun)
        );
    }

    #[test]
    fn sd_takes_a_file() {
        let options = run_options(&["--run", "9", "--sd", "card.img"]);
        assert_eq!(options.sd.as_deref(), Some("card.img"));
        assert_eq!(run_options(&["--run", "9"]).sd, None);
        assert_eq!(
            parse_args(&["--firmware", "f", "--run", "9", "--sd"]),
            Err(ArgError::Usage)
        );
    }

    #[test]
    fn an_empty_file_name_is_refused_for_every_file_option() {
        for option in ["--flash", "--sd", "--trace", "--screenshot"] {
            assert_eq!(
                parse_args(&["--firmware", "f", "--run", "9", option, ""]),
                Err(ArgError::EmptyName(option.into())),
                "{option}"
            );
        }
        assert_eq!(
            ArgError::EmptyName("--flash".into())
                .to_string()
                .lines()
                .next(),
            Some("cmm2-emulator: --flash needs a file name, not an empty one")
        );
    }

    #[test]
    fn sd_alone_has_nothing_to_run() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--sd", "card.img"]),
            Err(ArgError::NothingToRun)
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
    fn console_can_write_the_trace_to_a_file() {
        let options = run_options(&["--console", "--trace", "t"]);
        assert!(options.console);
        assert_eq!(options.trace.as_deref(), Some("t"));
    }

    #[test]
    fn console_traces_the_peripherals_only_into_a_file() {
        let options = run_options(&["--console", "--trace", "t", "--trace-mmio"]);
        assert!(options.trace_mmio);
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--trace-mmio"]),
            Err(ArgError::ConsoleMmioNeedsFile)
        );
    }

    #[test]
    fn trace_from_is_decimal_or_hex() {
        assert_eq!(
            run_options(&["--run", "9", "--trace", "t", "--trace-from", "230000000"]).trace_from,
            Some(230_000_000)
        );
        assert_eq!(
            run_options(&["--run", "9", "--trace-mmio", "--trace-from", "0x10"]).trace_from,
            Some(16)
        );
        assert_eq!(
            run_options(&["--run", "9", "--trace", "t"]).trace_from,
            None
        );
    }

    #[test]
    fn trace_from_needs_a_valid_count() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--trace-from"]),
            Err(ArgError::Usage)
        );
        assert_eq!(
            parse_args(&["--firmware", "f", "--trace-from", "x"]),
            Err(ArgError::BadCount("x".into()))
        );
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

    fn run_options(args: &[&str]) -> RunOptions {
        let mut all = vec!["--firmware", "f"];
        all.extend_from_slice(args);
        match parse_args(&all) {
            Ok(Command::Run { options, .. }) => options,
            other => panic!("not a run: {other:?}"),
        }
    }

    fn board_options(args: &[&str]) -> RunOptions {
        match parse_args(args) {
            Ok(Command::Run {
                start: Start::Board,
                options,
            }) => options,
            other => panic!("not a board run: {other:?}"),
        }
    }

    #[test]
    fn trace_from_needs_a_trace() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--run", "9", "--trace-from", "5"]),
            Err(ArgError::TraceFromNeedsTrace)
        );
        assert_eq!(
            parse_args(&["--trace-from", "5"]),
            Err(ArgError::TraceFromNeedsTrace)
        );
    }

    #[test]
    fn no_arguments_start_the_board_with_its_window() {
        let options = board_options(&[]);
        assert_eq!(
            options,
            RunOptions {
                window: true,
                window_by_default: true,
                ..RunOptions::default()
            }
        );
    }

    #[test]
    fn only_a_window_nobody_asked_for_is_by_default() {
        assert!(board_options(&[]).window_by_default);
        assert!(board_options(&["--unpaced"]).window_by_default);
        for args in [&["--window"][..], &["--window", "--unpaced"]] {
            let options = board_options(args);
            assert!(options.window && !options.window_by_default, "{args:?}");
        }
        for args in [
            &["--console"][..],
            &["--run", "9"],
            &["--console", "--window"],
        ] {
            assert!(!board_options(args).window_by_default, "{args:?}");
        }
        assert!(!run_options(&["--window"]).window_by_default);
    }

    #[test]
    fn firmware_alone_prints_the_vectors_as_it_always_did() {
        assert_eq!(
            parse_args(&["--firmware", "f.bin"]),
            Ok(Command::Vectors("f.bin".into()))
        );
    }

    #[test]
    fn a_scratch_run_never_defaults_to_the_window() {
        assert_eq!(
            parse_args(&["--firmware", "f", "--sd", "c"]),
            Err(ArgError::NothingToRun)
        );
        assert!(!run_options(&["--console"]).window);
    }

    #[test]
    fn the_console_or_a_limit_takes_the_window_off_the_boards_default() {
        for args in [
            &["--console"][..],
            &["--run", "9"],
            &["--stop-at", "0x08000100"],
        ] {
            assert!(!board_options(args).window, "{args:?}");
        }
        assert!(board_options(&["--trace", "t"]).window);
        assert!(board_options(&["--unpaced"]).window);
        assert!(board_options(&["--console", "--window"]).window);
    }

    #[test]
    fn without_firmware_every_option_is_for_the_board() {
        let options = board_options(&[
            "--run",
            "9",
            "--flash",
            "x",
            "--sd",
            "c",
            "--rng-seed",
            "1",
            "--trace",
            "t",
        ]);
        assert_eq!(options.run, Some(9));
        assert_eq!(options.flash.as_deref(), Some("x"));
        assert_eq!(options.sd.as_deref(), Some("c"));
        assert_eq!(board_options(&["--console", "--on", "a", "b"]).on.len(), 1);
        assert_eq!(
            parse_args(&["--on", "a", "b"]),
            Err(ArgError::NeedsConsole("--on".into()))
        );
        assert!(board_options(&["--console-keys"]).console_keys);
    }

    #[test]
    fn burn_takes_a_file_and_goes_with_the_other_board_options() {
        let options = board_options(&["--burn", "f.bin", "--console", "--run", "9", "--sd", "c"]);
        assert_eq!(options.burn.as_deref(), Some("f.bin"));
        assert!(board_options(&["--burn", "f.bin"]).window);
        assert_eq!(board_options(&[]).burn, None);
        assert_eq!(parse_args(&["--burn"]), Err(ArgError::Usage));
        assert_eq!(
            parse_args(&["--burn", ""]),
            Err(ArgError::EmptyName("--burn".into()))
        );
    }

    #[test]
    fn burn_and_firmware_together_are_refused_in_either_order() {
        for args in [
            &["--burn", "a", "--firmware", "b", "--run", "9"][..],
            &["--firmware", "b", "--burn", "a"],
        ] {
            assert_eq!(parse_args(args), Err(ArgError::BurnAndFirmware), "{args:?}");
        }
    }

    #[test]
    fn firmware_with_any_options_is_a_scratch_run() {
        for args in [
            &["--console"][..],
            &["--window"],
            &["--run", "9"],
            &["--console", "--on", "a", "b"],
            &["--unpaced", "--console"],
            &["--flash", "x", "--console"],
            &["--sd", "c", "--run", "9"],
            &["--rng-seed", "1", "--console"],
            &["--console", "--trace", "t"],
        ] {
            let mut all = vec!["--firmware", "f"];
            all.extend_from_slice(args);
            assert!(
                matches!(
                    parse_args(&all),
                    Ok(Command::Run {
                        start: Start::File(_),
                        ..
                    })
                ),
                "{args:?}"
            );
        }
    }

    #[test]
    fn no_sd_is_for_the_board() {
        assert!(board_options(&["--no-sd"]).no_sd);
        assert!(!board_options(&["--console"]).no_sd);
        assert_eq!(
            parse_args(&["--firmware", "f", "--console", "--no-sd"]),
            Err(ArgError::BoardOnly("--no-sd".into()))
        );
    }

    #[test]
    fn sd_and_no_sd_together_are_refused() {
        assert_eq!(
            parse_args(&["--sd", "c", "--no-sd"]),
            Err(ArgError::SdAndNoSd)
        );
        assert_eq!(
            parse_args(&["--no-sd", "--sd", "c"]),
            Err(ArgError::SdAndNoSd)
        );
    }

    #[test]
    fn a_second_firmware_or_burn_is_refused_and_an_empty_one_too() {
        assert_eq!(
            parse_args(&["--firmware", "a", "--firmware", "b"]),
            Err(ArgError::Unknown("--firmware".into()))
        );
        assert_eq!(
            parse_args(&["--burn", "a", "--burn", "b"]),
            Err(ArgError::Repeated("--burn".into()))
        );
        // An empty firmware name is loaded, and fails there, as it did.
        assert_eq!(
            parse_args(&["--firmware", ""]),
            Ok(Command::Vectors(String::new()))
        );
        assert_eq!(
            parse_args(&["--burn", ""]),
            Err(ArgError::EmptyName("--burn".into()))
        );
    }
}
