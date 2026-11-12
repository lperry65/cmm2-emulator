mod args;

use std::cell::RefCell;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::process::ExitCode;
use std::rc::Rc;

use args::{Command, RunOptions};
use cmm2_core::{
    AccessKind, Console, Cpu, CpuError, Firmware, FirmwareError, Irqs, MemoryError,
    PeripheralAccess, Stop, Svd, cmm2_memory,
};

/// How many unmapped accesses the summary lists; the count covers all of them.
const UNMAPPED_SHOWN: usize = 5;

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
    Trace { path: String, source: io::Error },
    Write(io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Firmware(err) => err.fmt(f),
            RunError::Memory(err) => err.fmt(f),
            RunError::Cpu(err) => err.fmt(f),
            RunError::Trace { path, source } => write!(f, "{path}: {source}"),
            RunError::Write(err) => write!(f, "writing the trace: {err}"),
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

/// Returns whether the run ended without a fault.
fn run_firmware(path: &str, options: &RunOptions) -> Result<bool, RunError> {
    let firmware = Firmware::load(path)?;
    let irqs = Irqs::new();
    let mut memory = cmm2_memory(&irqs);
    let trace = trace_out(options)?;
    if let (Some(trace), true) = (&trace, options.trace_mmio) {
        let sink = trace.clone();
        memory.set_peripheral_trace(Some(Box::new(move |access| {
            sink.borrow_mut()
                .line(format_args!("{}", access_line(access)));
        })));
    }
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs)?;

    let limit = options.run.unwrap_or(u64::MAX);
    let stop = match (&options.trace, &trace) {
        (Some(_), Some(sink)) => cpu.run_traced(limit, options.stop_at, |pc, text| {
            sink.borrow_mut().line(format_args!("{pc:#010X}  {text}"));
        }),
        _ => cpu.run_until(limit, options.stop_at),
    };
    if let Some(trace) = trace {
        trace.borrow_mut().finish().map_err(RunError::Write)?;
    }

    let ok = !matches!(stop, Stop::Fault(_));
    print_summary(&mut cpu, &stop);
    print_console(&console);
    Ok(ok)
}

/// What the guest sent to its console, until there is an interactive one.
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
