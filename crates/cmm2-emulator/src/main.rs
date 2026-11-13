mod args;
mod pacing;
mod script;
mod terminal;
mod window;

use std::cell::RefCell;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use args::{Command, RunOptions};
use cmm2_core::{
    AccessKind, CORE_HZ, Console, Cpu, CpuError, Firmware, FirmwareError, FrameError, Irqs,
    MemoryError, PeripheralAccess, Stop, Svd, cmm2_memory,
};
use pacing::{Pacer, RealTime};
use script::Script;
use terminal::RawMode;
use window::{Window, WindowError};

/// How many unmapped accesses the summary lists; the count covers all of them.
const UNMAPPED_SHOWN: usize = 5;

/// How many instructions run between looks at the console in `--console` mode.
const SLICE: u64 = 50_000;

/// Ctrl-] ends a `--console` run; the terminal is in raw mode, so Ctrl-C goes to the guest.
const QUIT_KEY: u8 = 0x1D;

fn main() -> ExitCode {
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
        Command::Run { firmware, options } if options.console || options.window => {
            finish(run_live(&firmware, options))
        }
        Command::Run { firmware, options } => finish(run_firmware(&firmware, &options)),
    }
}

/// Exit 0 when the run ended normally, 1 on a fault or any error.
fn finish(result: Result<bool, RunError>) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            eprintln!("cmm2-emulator: {err}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug)]
enum RunError {
    Firmware(FirmwareError),
    Memory(MemoryError),
    Cpu(CpuError),
    Trace {
        path: String,
        source: io::Error,
    },
    Write(io::Error),
    Console(io::Error),
    Window(WindowError),
    /// There is no screen to take a picture of.
    Frame(FrameError),
    Screenshot {
        path: String,
        source: io::Error,
    },
    /// The guest faulted; the text is the core's description.
    Fault(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Firmware(err) => err.fmt(f),
            RunError::Memory(err) => err.fmt(f),
            RunError::Cpu(err) => err.fmt(f),
            RunError::Trace { path, source } => write!(f, "{path}: {source}"),
            RunError::Write(err) => write!(f, "writing the trace: {err}"),
            RunError::Console(err) => write!(f, "writing the console output: {err}"),
            RunError::Window(err) => err.fmt(f),
            RunError::Frame(err) => write!(f, "no screenshot: {err}"),
            RunError::Screenshot { path, source } => write!(f, "{path}: {source}"),
            RunError::Fault(description) => f.write_str(description),
        }
    }
}

impl std::error::Error for RunError {}

impl From<FirmwareError> for RunError {
    fn from(err: FirmwareError) -> RunError {
        RunError::Firmware(err)
    }
}

impl From<MemoryError> for RunError {
    fn from(err: MemoryError) -> RunError {
        RunError::Memory(err)
    }
}

impl From<CpuError> for RunError {
    fn from(err: CpuError) -> RunError {
        RunError::Cpu(err)
    }
}

fn show_vectors(path: &str) -> Result<bool, RunError> {
    let firmware = Firmware::load(path)?;
    let vectors = firmware.vector_table();
    println!("initial SP {:#010X}", vectors.initial_sp);
    println!("reset handler {:#010X}", vectors.reset_handler);
    Ok(true)
}

/// Where the trace goes. The instruction lines and the peripheral lines share one, so they
/// stay in order. The first write error is kept for the end, as the callbacks cannot return it.
struct TraceOut {
    out: BufWriter<Box<dyn Write>>,
    error: Option<io::Error>,
}

impl TraceOut {
    fn line(&mut self, line: fmt::Arguments) {
        if self.error.is_none()
            && let Err(err) = writeln!(self.out, "{line}")
        {
            self.error = Some(err);
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        match self.error.take() {
            Some(err) => Err(err),
            None => self.out.flush(),
        }
    }
}

fn trace_out(options: &RunOptions) -> Result<Option<Rc<RefCell<TraceOut>>>, RunError> {
    let out: Box<dyn Write> = match &options.trace {
        Some(path) => Box::new(File::create(path).map_err(|source| RunError::Trace {
            path: path.clone(),
            source,
        })?),
        None if options.trace_mmio => Box::new(io::stdout()),
        None => return Ok(None),
    };
    Ok(Some(Rc::new(RefCell::new(TraceOut {
        out: BufWriter::new(out),
        error: None,
    }))))
}

/// A register name for the trace, with the byte offset when the access is not at its start.
fn register_name(addr: u32) -> String {
    match Svd::stm32h743().lookup(addr) {
        Some(found) if found.offset != 0 => format!("{} +{}", found.name, found.offset),
        Some(found) => found.name.to_string(),
        None => "?".to_string(),
    }
}

fn access_line(access: &PeripheralAccess) -> String {
    format!(
        "    {} {} {:#010X} size {} = {:#010X}",
        if access.write { 'W' } else { 'R' },
        register_name(access.addr),
        access.addr,
        access.size,
        access.value
    )
}

struct Machine {
    cpu: Cpu,
    console: Console,
    trace: Option<Rc<RefCell<TraceOut>>>,
}

fn build_machine(path: &str, options: &RunOptions) -> Result<Machine, RunError> {
    let firmware = Firmware::load(path)?;
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let trace = trace_out(options)?;
    let console = memory.console();
    let cpu = Cpu::new(&firmware, memory, irqs)?;
    Ok(Machine {
        cpu,
        console,
        trace,
    })
}

/// Returns whether the run ended without a fault.
fn run_firmware(path: &str, options: &RunOptions) -> Result<bool, RunError> {
    let Machine {
        mut cpu,
        console,
        trace,
    } = build_machine(path, options)?;

    let limit = options.run.unwrap_or(u64::MAX);
    let stop = run_slice(&mut cpu, options, trace.as_ref(), limit);
    report_resets(&cpu, 0);
    finish_trace(trace)?;

    let ok = !matches!(stop, Stop::Fault(_));
    print_summary(&mut cpu, &stop);
    print_console(&console);
    write_screenshot(&mut cpu, options)?;
    Ok(ok)
}

/// Runs with the console on the host terminal and the screen in a window, as asked, until the
/// quit key, the window is closed, the limit, `--stop-at` or a fault. The machine is held to
/// its own speed unless `--unpaced`. The summary is left out, as the screen belongs to the
/// guest. Without `--console` the guest's output is printed at the end.
fn run_live(path: &str, mut options: RunOptions) -> Result<bool, RunError> {
    let Machine {
        mut cpu,
        console,
        trace,
    } = build_machine(path, &options)?;
    let mut window = options
        .window
        .then(Window::open)
        .transpose()
        .map_err(RunError::Window)?;
    let _raw = options.console.then(RawMode::enable);
    let keys = options.console.then(read_stdin);
    let mut script = Script::new(std::mem::take(&mut options.on));
    let limit = options.run.unwrap_or(u64::MAX);
    let mut stdout = io::stdout();
    let mut resets = 0;
    let clock = cpu.memory().clock();
    let mut pacer = (!options.unpaced).then(|| Pacer::new(RealTime::start(), CORE_HZ));

    let result = loop {
        let budget = limit.saturating_sub(cpu.instruction_count()).min(SLICE);
        let stop = run_slice(&mut cpu, &options, trace.as_ref(), budget);
        if keys.is_some()
            && let Err(err) = show_output(&console, &mut script, &mut stdout)
        {
            break Err(RunError::Console(err));
        }
        resets = report_resets(&cpu, resets);
        if let Some(window) = &mut window {
            match window.update(|| cpu.memory().frame()) {
                Ok(true) => {}
                Ok(false) => break Ok(true),
                Err(err) => break Err(RunError::Window(err)),
            }
        }
        match stop {
            Stop::Fault(fault) => break Err(RunError::Fault(fault.description)),
            Stop::Address(_) => break Ok(true),
            Stop::InstructionLimit if cpu.instruction_count() >= limit => break Ok(true),
            Stop::InstructionLimit => {}
        }
        if let Some(keys) = &keys
            && !send_keys(&console, keys)
        {
            break Ok(true);
        }
        if let Some(pacer) = &mut pacer {
            pacer.pace(clock.now());
        }
    };
    finish_trace(trace)?;
    let ok = result?;
    if keys.is_none() {
        print_console(&console);
    }
    write_screenshot(&mut cpu, &options)?;
    Ok(ok)
}

/// Writes the screen to `--screenshot`, if it was given.
fn write_screenshot(cpu: &mut Cpu, options: &RunOptions) -> Result<(), RunError> {
    let Some(path) = &options.screenshot else {
        return Ok(());
    };
    let frame = cpu.memory().frame().map_err(RunError::Frame)?;
    std::fs::write(path, frame.to_ppm()).map_err(|source| RunError::Screenshot {
        path: path.clone(),
        source,
    })
}

/// Says on stderr, outside the guest's console stream, that the guest reset the machine, once
/// for each reset after the first `seen`. Returns how many there have been.
fn report_resets(cpu: &Cpu, seen: u32) -> u32 {
    for _ in seen..cpu.resets() {
        eprintln!("cmm2-emulator: system reset");
    }
    cpu.resets()
}

/// Runs up to `budget` instructions. The traces start at `--trace-from`; until then this is a
/// plain run, so a slice that crosses that point is split in two. The peripheral trace is only
/// hooked in at that point, so it cannot write earlier.
fn run_slice(
    cpu: &mut Cpu,
    options: &RunOptions,
    trace: Option<&Rc<RefCell<TraceOut>>>,
    budget: u64,
) -> Stop {
    let Some(sink) = trace else {
        return cpu.run_until(budget, options.stop_at);
    };
    let untraced = options
        .trace_from
        .unwrap_or(0)
        .saturating_sub(cpu.instruction_count())
        .min(budget);
    if untraced > 0 {
        let stop = cpu.run_until(untraced, options.stop_at);
        if stop != Stop::InstructionLimit || untraced == budget {
            return stop;
        }
    }
    let budget = budget - untraced;
    if options.trace_mmio {
        let sink = sink.clone();
        cpu.memory()
            .set_peripheral_trace(Some(Box::new(move |access| {
                sink.borrow_mut()
                    .line(format_args!("{}", access_line(access)));
            })));
    }
    if options.trace.is_none() {
        return cpu.run_until(budget, options.stop_at);
    }
    cpu.run_traced(budget, options.stop_at, |pc, text| {
        sink.borrow_mut().line(format_args!("{pc:#010X}  {text}"));
    })
}

fn finish_trace(trace: Option<Rc<RefCell<TraceOut>>>) -> Result<(), RunError> {
    match trace {
        Some(trace) => trace.borrow_mut().finish().map_err(RunError::Write),
        None => Ok(()),
    }
}

/// Writes what the guest sent to the terminal and types the keys the script has for it.
fn show_output(console: &Console, script: &mut Script, out: &mut impl Write) -> io::Result<()> {
    let output = console.take_output();
    if output.is_empty() {
        return Ok(());
    }
    out.write_all(&output)?;
    out.flush()?;
    console.push_input(&script.feed(&output));
    Ok(())
}

/// Passes the typed keys on to the guest. Returns false once the quit key has been typed.
fn send_keys(console: &Console, keys: &Receiver<Vec<u8>>) -> bool {
    while let Ok(bytes) = keys.try_recv() {
        let (typed, quit) = until_quit(&bytes);
        console.push_input(typed);
        if quit {
            return false;
        }
    }
    true
}

/// The keys before the quit key, and whether it was among them.
fn until_quit(bytes: &[u8]) -> (&[u8], bool) {
    match bytes.iter().position(|&byte| byte == QUIT_KEY) {
        Some(at) => (&bytes[..at], true),
        None => (bytes, false),
    }
}

/// Reads stdin on a thread, as a read blocks. At the end of stdin the thread stops and the
/// emulator carries on.
fn read_stdin() -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buffer = [0; 256];
        while let Ok(count @ 1..) = stdin.read(&mut buffer) {
            if sender.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    receiver
}

/// What the guest sent to its console, after a run with no terminal attached.
fn print_console(console: &Console) {
    let output = console.take_output();
    if output.is_empty() {
        return;
    }
    println!("console output ({} bytes):", output.len());
    let text = String::from_utf8_lossy(&output);
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

fn print_summary(cpu: &mut Cpu, stop: &Stop) {
    match stop {
        Stop::InstructionLimit => println!("stopped: instruction limit"),
        Stop::Address(addr) => println!("stopped: address {addr:#010X}"),
        Stop::Fault(fault) => println!("stopped: {}", fault.description),
    }
    println!("instructions {}", cpu.instruction_count());
    println!("pc {:#010X}", cpu.pc());
    let unmapped = cpu.memory().take_unmapped();
    if unmapped.is_empty() {
        return;
    }
    println!("unmapped accesses {}", unmapped.len());
    for access in unmapped.iter().take(UNMAPPED_SHOWN) {
        let kind = match access.kind {
            AccessKind::Read => "read",
            AccessKind::Write(_) => "write",
        };
        println!(
            "  {:#010X} {kind} {:#010X} size {}",
            access.pc, access.addr, access.size
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quit_key_and_what_follows_it_are_not_typed() {
        assert_eq!(until_quit(b"ab"), (&b"ab"[..], false));
        assert_eq!(until_quit(b"a\x1Db"), (&b"a"[..], true));
        assert_eq!(until_quit(b"\x1D"), (&b""[..], true));
    }
}
