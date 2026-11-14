mod args;
mod keys;
mod local_clock;
mod pacing;
mod script;
mod signals;
mod terminal;
mod usb_keys;
mod window;

use std::cell::RefCell;
use std::ffi::CString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use args::{Command, RunOptions};
use cmm2_core::{
    AccessKind, CORE_HZ, Console, Cpu, CpuError, Firmware, FirmwareError, FrameError, HidKeyboard,
    ImageSizeError, Irqs, KeyReport, MIN_IMAGE_SIZE, MemoryError, PeripheralAccess,
    PersistentError, SdSlot, Stop, Svd, cmm2_memory,
};
use local_clock::LocalClock;
use pacing::{Pacer, RealTime};
use script::Script;
use terminal::RawMode;
use usb_keys::UsbKeys;
use window::{Window, WindowError};

/// How many unmapped accesses the summary lists; the count covers all of them.
const UNMAPPED_SHOWN: usize = 5;

/// How many instructions run between looks at the console in `--console` mode.
const SLICE: u64 = 50_000;

/// Ctrl-] ends a `--console` run; the terminal is in raw mode, so Ctrl-C goes to the guest.
const QUIT_KEY: u8 = 0x1D;

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

/// The messages of panics that are held back while a run is going, oldest first. A run that
/// panics is saved after the message would have been written, so the message waits for the save.
/// Only so many are kept, the first ones, as the first panic is the one that matters.
static HELD_PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
const HELD_PANICS_MAX: usize = 8;

/// A run is going, and what it does in `run_and_save` is saved if it panics.
static HOLDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Keeps a panic's message for after the save.
fn hold_panic(text: String) {
    let mut held = HELD_PANICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.len() < HELD_PANICS_MAX {
        held.push(text);
    }
}

/// The message of a panic, in the words of the default one but without the thread's id and the
/// note on how to get a backtrace.
fn describe_panic(
    thread: Option<&str>,
    location: Option<(&str, u32, u32)>,
    payload: &(dyn std::any::Any + Send),
) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("Box<dyn Any>");
    let thread = thread.unwrap_or("<unnamed>");
    match location {
        Some((file, line, column)) => {
            format!("thread '{thread}' panicked at {file}:{line}:{column}:\n{message}\n")
        }
        None => format!("thread '{thread}' panicked:\n{message}\n"),
    }
}

/// The main thread, which is the one that puts the terminal back (as it does on the way out of
/// a run); a panic on another thread (stdin's) leaves the terminal as the run has it.
static MAIN_THREAD: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);

fn is_main_thread() -> bool {
    MAIN_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_none_or(|main| main == std::thread::current().id())
}

thread_local! {
    /// This thread has had a panic held back, and has not yet caught it. A second one now is a
    /// panic while unwinding, which aborts the process. It is cleared in `run_and_save` only,
    /// where the panic is caught: a panic caught by code inside a run would leave it set, and
    /// the next ordinary panic on the thread would be taken for a double one. There is no such
    /// code today (nothing in a run catches panics), and if there were it would have to clear
    /// this too.
    static PANIC_SEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The text a held panic gets: its message, and a backtrace if the environment asks for one as
/// the default hook does (RUST_LIB_BACKTRACE, then RUST_BACKTRACE; `full` for the long form).
fn panic_text(info: &std::panic::PanicHookInfo<'_>) -> String {
    let location = info
        .location()
        .map(|place| (place.file(), place.line(), place.column()));
    let mut text = describe_panic(std::thread::current().name(), location, info.payload());
    let backtrace = std::backtrace::Backtrace::capture();
    if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
        let full = std::env::var("RUST_BACKTRACE").is_ok_and(|value| value == "full");
        text.push_str("stack backtrace:\n");
        text.push_str(&if full {
            format!("{backtrace:#}")
        } else {
            format!("{backtrace}")
        });
        text.push('\n');
    }
    text
}

/// Makes a panic put the terminal back before its message, as the main thread does on the way
/// out (not a panic on another thread, as the run goes on), and not let the message stop the
/// save that follows it. The default hook writes to standard error and waits if nobody is
/// reading it (a full pipe takes whatever the reader has room for, and a message with a
/// backtrace is long, so there is no telling beforehand that it will fit), and the save is after
/// it. So while a run is going (`HOLDING`) the message is kept (`hold_panic`) and said once the
/// files are written (`run_and_save`), through the writer that a signal can end. At other times
/// the default hook says it as ever.
///
/// A panic while another is being unwound (a `Drop` that panics) is an abort that nothing
/// catches, and nothing is saved then. What is held is written at once, plainly, and then the
/// new message, as the process is about to end and would lose them. A second signal during the
/// save after a panic also loses what is held: the handler cannot take a lock or allocate.
fn install_panic_hook() {
    set_main_thread();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if is_main_thread() {
            terminal::restore();
        }
        if !HOLDING.load(std::sync::atomic::Ordering::Acquire) {
            default(info);
            return;
        }
        let text = panic_text(info);
        if PANIC_SEEN.with(|seen| seen.replace(true)) {
            let mut stderr = io::stderr();
            let held = std::mem::take(
                &mut *HELD_PANICS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            for held in held.iter().chain(std::iter::once(&text)) {
                let _ = stderr.write_all(held.as_bytes());
            }
        } else {
            hold_panic(text);
        }
    }));
}

/// The calling thread is the main one, for the hook.
fn set_main_thread() {
    *MAIN_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::thread::current().id());
}

/// Says the panic messages that were held back, oldest first.
fn say_late_panic(out: &mut impl Write) {
    let late = std::mem::take(
        &mut *HELD_PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for text in late {
        let _ = out.write_all(text.as_bytes());
    }
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
            say(err);
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
    /// The `--flash` file could not be read or written.
    Flash {
        path: String,
        source: io::Error,
    },
    /// The `--flash` file is not the size of the saved state.
    FlashSize {
        path: String,
        source: PersistentError,
    },
    /// The `--sd` file could not be read or written.
    Sd {
        path: String,
        source: io::Error,
    },
    /// The `--sd` file is not a whole number of blocks.
    SdSize {
        path: String,
        source: ImageSizeError,
    },
    /// The `--sd` file is too small for a card whose CSD can say its size.
    SdTooSmall {
        path: String,
        size: usize,
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
            RunError::Flash { path, source } => write!(f, "{path}: {source}"),
            RunError::FlashSize { path, source } => write!(f, "{path}: {source}"),
            RunError::Sd { path, source } => write!(f, "{path}: {source}"),
            RunError::SdSize { path, source } => write!(f, "{path}: {source}"),
            RunError::SdTooSmall { path, size } => write!(
                f,
                "{path}: {size} bytes is smaller than the {MIN_IMAGE_SIZE} bytes (512 KB) of the smallest card"
            ),
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

    // A write that a signal gave up on is the run ending, not a trace that failed.
    fn finish(&mut self) -> io::Result<()> {
        match self.error.take() {
            Some(err) => Err(err),
            None => self.out.flush(),
        }
        .or_else(|err| {
            if is_signal_end(&err) {
                Ok(())
            } else {
                Err(err)
            }
        })
    }
}

fn trace_out(options: &RunOptions) -> Result<Option<Rc<RefCell<TraceOut>>>, RunError> {
    let out: Box<dyn Write> = match &options.trace {
        Some(path) => Box::new(Output::file(File::create(path).map_err(|source| {
            RunError::Trace {
                path: path.clone(),
                source,
            }
        })?)),
        None if options.trace_mmio => Box::new(Output::stdout()),
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
    sd: SdSlot,
    trace: Option<Rc<RefCell<TraceOut>>>,
}

fn build_machine(path: &str, options: &RunOptions) -> Result<Machine, RunError> {
    let firmware = Firmware::load(path)?;
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let trace = trace_out(options)?;
    let console = memory.console();
    let sd = memory.sd_slot();
    // The board's DS3231 is always fitted, and keeps the host's time.
    memory.ds3231().set_host_clock(Box::new(LocalClock));
    let mut cpu = Cpu::new(&firmware, memory, irqs)?;
    if let Some(path) = &options.flash {
        load_flash(&mut cpu, path)?;
    }
    if let Some(path) = &options.sd {
        load_sd(&sd, path)?;
    }
    Ok(Machine {
        cpu,
        console,
        sd,
        trace,
    })
}

/// Loads the saved flash and clock from the `--flash` file. A file that is not there yet is the first run:
/// the flash stays as the firmware leaves it. The options have to be saved at the end, so a
/// file that could not be is refused now and not after a long run.
fn load_flash(cpu: &mut Cpu, path: &str) -> Result<(), RunError> {
    let flash_error = |source| RunError::Flash {
        path: path.to_string(),
        source,
    };
    check_regular(path).map_err(flash_error)?;
    if let Some(why) = why_not_saveable(path) {
        return Err(flash_error(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{why}, so the run could not save its flash and clock"),
        )));
    }
    remove_stale_temps(path);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(flash_error(source)),
    };
    cpu.load_persistent(&bytes)
        .map_err(|source| RunError::FlashSize {
            path: path.to_string(),
            source,
        })
}

/// Whether a path cannot be that of a file: it is empty, or ends in a slash, or in . or .., which
/// is a directory even if there is nothing there yet to say so.
fn not_a_file_name(path: &Path) -> bool {
    let name = path.as_os_str().as_bytes();
    let last = name.rsplit(|&byte| byte == b'/').next().unwrap_or(name);
    name.is_empty() || name.ends_with(b"/") || last == b"." || last == b".."
}

/// Refuses what the file named is if it is there and not a regular file: a device (a whole
/// card, with `sudo`, would be read through and then have a file renamed over its node), a
/// FIFO (the read would never end), a directory. This looks before anything is opened. A file
/// that is not there is for the read, or the first save, to say.
fn check_regular(path: &str) -> io::Result<()> {
    if not_a_file_name(Path::new(path)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file name (it is empty, or ends in a slash, or in . or ..)",
        ));
    }
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        )),
        // It is there and it has to be read, so that is said in words and not as a bare errno.
        Ok(_) if access(Path::new(path), libc::R_OK).is_err() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the file cannot be read",
        )),
        _ => Ok(()),
    }
}

/// Whether the calling user can use `path` as `mode` (R_OK, W_OK, X_OK), as faccessat(2) says
/// with AT_EACCESS, so by the effective user (which is the one that will write), and which
/// looks at the file system too: a read-only one is EROFS.
fn access(path: &Path, mode: libc::c_int) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: the pointer is to a NUL-terminated string that lives through the call.
    if unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The attribute bits (FS_IMMUTABLE_FL and FS_APPEND_FL of linux/fs.h) that make the rename of
/// a new file over this one fail with EPERM, as the mode bits do not show.
const FS_IMMUTABLE_OR_APPEND: libc::c_long = 0x10 | 0x20;

/// Whether the file system attribute flags of the file forbid replacing it.
fn is_immutable(file: &File) -> bool {
    use std::os::fd::AsRawFd;
    let mut flags: libc::c_long = 0;
    // SAFETY: the ioctl fills one c_long through the pointer; a file system that does not know
    // it answers with an error and leaves it 0.
    unsafe { libc::ioctl(file.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) };
    flags & FS_IMMUTABLE_OR_APPEND != 0
}

/// In a directory with the sticky bit (/tmp), only the owner of the file, the owner of the
/// directory or a user with CAP_FOWNER can rename a file away, which is what replacing it is.
fn sticky_refuses(
    dir_mode: u32,
    dir_owner: u32,
    file_owner: u32,
    user: u32,
    cap_fowner: bool,
) -> bool {
    dir_mode & libc::S_ISVTX != 0 && !cap_fowner && file_owner != user && dir_owner != user
}

/// What is wrong with a path that a save goes through, when the OS said no, in words. `denied`
/// is what EACCES means for what was being asked: that a directory cannot be searched, or that
/// one cannot be written to.
fn why_path_failed(err: &io::Error, path: &Path, denied: &str) -> String {
    match err.raw_os_error() {
        Some(libc::EROFS) => "its file system is read-only".to_string(),
        Some(libc::EACCES | libc::EPERM) => denied.to_string(),
        Some(libc::ENOENT) => format!("its directory {} does not exist", path.display()),
        Some(libc::ENOTDIR) => format!("{} is not a directory", path.display()),
        Some(libc::ELOOP) => "its path is a loop of symbolic links".to_string(),
        Some(libc::ENAMETOOLONG) => {
            "a name on its path is longer than the file system allows".to_string()
        }
        _ => format!("its directory {} cannot be used ({err})", path.display()),
    }
}

const NOT_SEARCHABLE: &str = "a directory on its path cannot be searched";
const NOT_WRITABLE: &str = "its directory is not writable";

/// Whether the effective capabilities hold CAP_FOWNER (bit 3), which is what lets a user rename
/// away another user's file in a sticky directory (as the kernel has it, and not just the
/// user being root: a root without it, and a user with it, are both possible). From the
/// CapEff line of /proc/self/status; where that cannot be read it is the effective user being root.
fn has_cap_fowner() -> bool {
    const CAP_FOWNER: u32 = 3;
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:"))?;
            u64::from_str_radix(line.trim(), 16).ok()
        })
        .map_or_else(
            // SAFETY: geteuid has no arguments and cannot fail.
            || unsafe { libc::geteuid() } == 0,
            |caps| caps >> CAP_FOWNER & 1 != 0,
        )
}

/// Why `write_atomically` could not save `path`, if it could not, as far as can be seen at the
/// start. It makes a temporary file in the directory of the file it ends up writing (a link is
/// followed), and renames it over the file. So that directory has to be there and writable, and
/// not on a read-only file system; the file has to be one that can be replaced (not immutable
/// or append-only, and not another user's in a sticky directory). The file's own permissions do
/// not matter to that. What cannot be seen from here (a full disk, say) is left to the save.
fn why_not_saveable(path: &str) -> Option<String> {
    let target = match resolve(Path::new(path)) {
        Ok(target) => target,
        Err(err) => return Some(why_path_failed(&err, Path::new(path), NOT_SEARCHABLE)),
    };
    // A link can point at a name that is not one (`ln -s newfile/ link`), which the name given
    // does not show.
    if not_a_file_name(&target) {
        return Some(format!(
            "it leads to {}, which cannot be the name of a file",
            target.display()
        ));
    }
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let dir_meta = match std::fs::metadata(dir) {
        Ok(meta) if !meta.is_dir() => return Some(format!("{} is not a directory", dir.display())),
        Ok(meta) => meta,
        Err(err) => return Some(why_path_failed(&err, dir, NOT_SEARCHABLE)),
    };
    // Searching the directory and writing to it are different rights, and are said apart.
    if let Err(err) = access(dir, libc::X_OK) {
        return Some(why_path_failed(
            &err,
            dir,
            "its directory cannot be searched",
        ));
    }
    if let Err(err) = access(dir, libc::W_OK) {
        return Some(why_path_failed(&err, dir, NOT_WRITABLE));
    }
    // A directory that is append-only lets a file be made in it but not renamed over another.
    // Its flags are asked of a descriptor, which takes the right to read it, so a directory
    // that cannot be read is one whose flags cannot be seen, and that is said.
    match File::open(dir) {
        Ok(dir) if is_immutable(&dir) => {
            return Some("its directory is append-only or immutable".to_string());
        }
        Ok(_) => {}
        Err(err) => say(format!(
            "{path}: its directory cannot be read ({err}), so whether it is append-only is not known"
        )),
    }
    // What is known of the file without opening it comes first, as a file that cannot be read
    // (which `check_regular` has already said) has its attributes too.
    let file_meta = std::fs::metadata(&target).ok()?;
    if File::open(&target).is_ok_and(|file| is_immutable(&file)) {
        return Some("the file is immutable or append-only".to_string());
    }
    // SAFETY: geteuid has no arguments and cannot fail.
    let user = unsafe { libc::geteuid() };
    sticky_refuses(
        dir_meta.mode(),
        dir_meta.uid(),
        file_meta.uid(),
        user,
        has_cap_fowner(),
    )
    .then(|| "its directory is sticky and the file is another user's".to_string())
}

/// Why the card of the `--sd` file is write-protected, if it is: the file cannot be written
/// back (as for `--flash`), or it is read-only, which is the same switch on a real card.
fn why_write_protected(path: &str) -> Option<String> {
    why_not_saveable(path).or_else(|| {
        access(Path::new(path), libc::W_OK)
            .is_err()
            .then(|| "the file is read-only".to_string())
    })
}

/// Puts the card image from the `--sd` file in the socket. The file has to exist: a card is
/// never made out of nothing. A card that could not be written back is one with its
/// write-protect switch on: the firmware refuses to write to it, and it is never written back.
fn load_sd(sd: &SdSlot, path: &str) -> Result<(), RunError> {
    let sd_error = |source| RunError::Sd {
        path: path.to_string(),
        source,
    };
    check_regular(path).map_err(sd_error)?;
    let image = std::fs::read(path).map_err(sd_error)?;
    let size = image.len();
    sd.insert(image).map_err(|source| RunError::SdSize {
        path: path.to_string(),
        source,
    })?;
    if size < MIN_IMAGE_SIZE {
        return Err(RunError::SdTooSmall {
            path: path.to_string(),
            size,
        });
    }
    remove_stale_temps(path);
    if let Some(why) = why_write_protected(path) {
        sd.set_write_protected(true);
        say(format!("{path}: the card is write-protected because {why}"));
    }
    Ok(())
}

/// The file a symlink chain ends at, which need not exist yet: a link is followed even when
/// what it points to is not there, so that the first save creates the target and leaves the link
/// as it is. A loop is an error (ELOOP).
fn resolve(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match std::fs::read_link(&path) {
            Ok(link) => path = path.parent().unwrap_or(Path::new("")).join(link),
            // Not a link, or not there at all (or under something that is not a directory).
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(libc::EINVAL | libc::ENOENT | libc::ENOTDIR)
                ) =>
            {
                return Ok(path);
            }
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::from_raw_os_error(libc::ELOOP))
}

/// How many links to follow before it is a loop, as Linux has it.
const MAX_LINKS: usize = 40;

/// Writes `bytes` to `path` so that a crash leaves the old file and not half of the new one: to
/// a temporary file in the same directory, which is synced, and then renamed over the file, and
/// the directory is synced after. A symlink is followed, even a dangling one, so the file it
/// points to is the one written and the link stays. The new file has the old one's permissions
/// (not its owner, which only a privileged process can keep, and a hard link to the old file
/// does not see the new contents). The temporary file is made beside the file, with a short name
/// of its own (the process's id and the time), so that it fits whatever the length of the
/// file's name, and so that two emulators on one file, or a temporary file that an earlier
/// crash left under a repeated process id (in a container), do not get in the way.
fn write_atomically(path: &str, bytes: &[u8]) -> io::Result<()> {
    let target = resolve(Path::new(path))?;
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let temp = dir.join(format!(".cmm2.{}.{nanos}.tmp", std::process::id()));

    let mut file = create_locked(&temp)?;
    let written =
        write_synced(&mut file, &target, bytes).and_then(|()| std::fs::rename(&temp, &target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written?;
    // The file is in its place by now, so a directory that cannot be synced (one that cannot be
    // read, or a file system that has no such thing) is something to say and not a failed save.
    if let Err(err) = File::open(dir).and_then(|dir| dir.sync_all()) {
        say(format!(
            "{path}: written, but its directory could not be synced ({err})"
        ));
    }
    Ok(())
}

/// Takes an advisory lock on the open file, without waiting: whether it was got. The lock goes
/// with the open file description, so it is gone when the file is closed or the process is, however
/// that happens, and another open of the same file (by another process, or here) cannot have it
/// while it is held.
fn try_lock(file: &File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: flock only takes a descriptor that is open, as this is.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Makes the temporary file, which must not be there, and locks it at once, for as long as it is
/// open, so that `remove_stale_temps` can tell it from one that nobody is writing.
fn create_locked(temp: &Path) -> io::Result<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(temp)?;
    // A file system that has no locks (some network ones) says no, and then the file is simply
    // not protected from the cleanup, which keeps to files it can lock.
    try_lock(&file);
    Ok(file)
}

/// How old a temporary file has to be (by its time of last change) to be removed. A new one may
/// be between being made and being locked, which is a matter of microseconds; a minute is far
/// longer than that.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Takes `.cmm2.<pid>.<nanos>.tmp` out of the directory of the file `path` leads to when nobody
/// has it. A save that was cut short (a SIGKILL, an abort, the power, a second Ctrl-C in the
/// middle of a slow one) leaves its temporary file, which is as big as the card. A save that is
/// going on holds an advisory lock on its file (`create_locked`), which is gone when the process
/// is, so a file that can be locked here and has not been changed for a minute belongs to no one.
/// That does not depend on what the pid in the name means here (another host or namespace, the
/// same directory seen by both), or on the clock. A file system with no locks (some network
/// ones, where locking can also fail or mislead) makes the lock fail, and then nothing is
/// removed: it is kept when it cannot be told. Errors are not reported: this is housekeeping.
fn remove_stale_temps(path: &str) {
    let Ok(target) = resolve(Path::new(path)) else {
        return;
    };
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_temp_name) {
            continue;
        }
        // What the directory says about the entry itself, not what a link in its place leads
        // to: a FIFO or a link to a device is not ours to open.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|changed| changed.elapsed().ok())
            .is_some_and(|age| age >= STALE_AFTER);
        // The file is kept open, and so locked, until it has been removed, so that a save that
        // starts meanwhile cannot be using it (it makes its own, new, file).
        let Some(file) = old.then(|| open_to_lock(&entry.path())).flatten() else {
            continue;
        };
        if try_lock(&file) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Opens a file that is to be locked and not read: not following a link, not waiting for a
/// FIFO's other end, and only if it turns out to be a regular file. It is opened for reading,
/// or if that is not allowed (a left-over file whose mode was copied from a write-only one) for
/// writing; a file with neither right cannot be locked, and is left (README says how to clear
/// it by hand).
fn open_to_lock(path: &Path) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY;
    let file = File::options()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .or_else(|_| File::options().write(true).custom_flags(flags).open(path))
        .ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// Whether a name is that of a temporary file `write_atomically` makes: `.cmm2.<pid>.<nanos>.tmp`,
/// with a process id above 0 and both numbers in digits and nothing else (not a sign).
fn is_temp_name(name: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    name.strip_prefix(".cmm2.")
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(pid, made)| {
            digits(pid) && digits(made) && pid.parse::<libc::pid_t>().is_ok_and(|pid| pid > 0)
        })
}

/// Fills `file` with `bytes` and the permissions `target` has, if it exists, and syncs it.
fn write_synced(file: &mut File, target: &Path, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    if let Ok(old) = std::fs::metadata(target) {
        file.set_permissions(old.permissions())?;
    }
    file.sync_all()
}

/// Writes the flash and clock to the `--flash` file, if it was given.
fn save_flash(cpu: &mut Cpu, options: &RunOptions) -> Result<(), RunError> {
    let Some(path) = &options.flash else {
        return Ok(());
    };
    write_atomically(path, &cpu.persistent()).map_err(|source| RunError::Flash {
        path: path.clone(),
        source,
    })
}

/// Writes the card in the socket back to the `--sd` file, if it was given and the guest changed
/// the card. A card that was only read leaves the file alone, so whatever else touched it
/// meanwhile is not overwritten.
fn save_sd(sd: &SdSlot, options: &RunOptions) -> Result<(), RunError> {
    let Some(path) = &options.sd else {
        return Ok(());
    };
    // A card with its write-protect switch on is never written back, whatever the guest did.
    if !sd.modified() || sd.write_protected() {
        return Ok(());
    }
    let Some(image) = sd.eject() else {
        return Ok(());
    };
    write_atomically(path, &image).map_err(|source| RunError::Sd {
        path: path.clone(),
        source,
    })
}

/// Writes back everything the run changed that a file keeps. Both are tried, and the first
/// error is the one returned.
fn save_files(cpu: &mut Cpu, sd: &SdSlot, options: &RunOptions) -> Result<(), RunError> {
    let flash = save_flash(cpu, options);
    flash.and(save_sd(sd, options))
}

/// Runs `run`, and writes the files back whichever way it ends. A panic in it is saved from too
/// and then carries on as it was, so the guest's session is not lost to a bug in the emulator.
/// What this cannot cover is the process being killed outright (SIGKILL), or the power going.
fn run_and_save<T>(
    cpu: &mut Cpu,
    sd: &SdSlot,
    options: &RunOptions,
    run: impl FnOnce(&mut Cpu) -> T,
) -> (T, Result<(), RunError>) {
    run_and_save_saying(cpu, sd, options, run, &mut Output::stderr())
}

/// `run_and_save`, with where a panic's message that was held back is said.
fn run_and_save_saying<T>(
    cpu: &mut Cpu,
    sd: &SdSlot,
    options: &RunOptions,
    run: impl FnOnce(&mut Cpu) -> T,
    late: &mut impl Write,
) -> (T, Result<(), RunError>) {
    // A panic's message is held until the files are written; see `install_panic_hook`.
    HOLDING.store(true, std::sync::atomic::Ordering::Release);
    let ran = catch_unwind(AssertUnwindSafe(|| run(cpu)));
    HOLDING.store(false, std::sync::atomic::Ordering::Release);
    PANIC_SEEN.with(|seen| seen.set(false));
    match ran {
        Ok(result) => (result, save_files(cpu, sd, options)),
        Err(panic) => {
            if let Err(err) = save_files(cpu, sd, options) {
                say(err);
            }
            // The files are written, and now the message, which could have waited for a reader.
            say_late_panic(late);
            resume_unwind(panic)
        }
    }
}

/// Returns whether the run ended without a fault. The run goes in slices so that a signal ends
/// it too, as `run_live` does.
fn run_firmware(path: &str, options: &RunOptions) -> Result<bool, RunError> {
    let Machine {
        mut cpu,
        console,
        sd,
        trace,
    } = build_machine(path, options)?;

    let limit = options.run.unwrap_or(u64::MAX);
    signals::begin_run();
    let (stop, saved) = run_and_save(&mut cpu, &sd, options, |cpu| {
        loop {
            let budget = limit.saturating_sub(cpu.instruction_count()).min(SLICE);
            let stop = run_slice(cpu, options, trace.as_ref(), budget);
            if stop != Stop::InstructionLimit
                || cpu.instruction_count() >= limit
                || signals::caught()
            {
                break stop;
            }
        }
    });
    report_resets(&cpu, 0);
    let traced = finish_trace(trace);

    // A fault is in the summary, which comes first; the errors after it are all said.
    let ok = !matches!(stop, Stop::Fault(_));
    let mut out = BufWriter::new(Output::stdout());
    let printed = print_summary(&mut cpu, &stop, signals::caught(), &mut out)
        .and_then(|()| print_console(&console, &mut out))
        .and_then(|()| out.flush());
    let shot = write_screenshot(&mut cpu, options);
    settle(
        Ok(ok),
        [traced, output_result(printed).and(shot), saved],
        &mut Output::stderr(),
    )
}

/// Runs with the console on the host terminal and the screen and keyboard in a window, as asked,
/// until the quit key, the window is closed, the limit, `--stop-at`, a fault or a signal
/// (SIGINT, SIGTERM or SIGHUP). The machine is held to its own speed unless `--unpaced`. The
/// summary is left out, as the screen belongs to the guest. The window's keys are a USB
/// keyboard unless `--console-keys`. Without `--console` the guest's output is printed at the
/// end. The `--flash` and `--sd` files are written whichever way the run ends, and when the
/// emulator panics.
fn run_live(path: &str, mut options: RunOptions) -> Result<bool, RunError> {
    let Machine {
        mut cpu,
        console,
        sd,
        trace,
    } = build_machine(path, &options)?;
    // The window's keys are the USB keyboard's, unless it is asked to type into the console.
    let usb_keys = (options.window && !options.console_keys).then(|| plug_in_keyboard(&cpu));
    let mut window = options
        .window
        .then(|| signals::with_blocked(|| Window::open(usb_keys)))
        .transpose()
        .map_err(RunError::Window)?;
    let _raw = options.console.then(RawMode::enable);
    let keys = options.console.then(|| signals::with_blocked(read_stdin));
    let mut script = Script::new(std::mem::take(&mut options.on));
    let limit = options.run.unwrap_or(u64::MAX);
    let mut stdout = Output::stdout();
    let mut resets = 0;
    let clock = cpu.memory().clock();
    let mut pacer = (!options.unpaced).then(|| Pacer::new(RealTime::start(), CORE_HZ));

    // Every thread that is to have the signals blocked has been made, so the main one takes them.
    signals::begin_run();
    let (result, saved) = run_and_save(&mut cpu, &sd, &options, |cpu| {
        loop {
            if signals::caught() {
                break Ok(true);
            }
            let budget = limit.saturating_sub(cpu.instruction_count()).min(SLICE);
            let stop = run_slice(cpu, &options, trace.as_ref(), budget);
            if keys.is_some()
                && let Err(err) = show_output(&console, &mut script, &mut stdout)
            {
                // A write that a signal cut short is the signal ending the run.
                break if is_signal_end(&err) {
                    Ok(true)
                } else {
                    Err(RunError::Console(err))
                };
            }
            resets = report_resets(cpu, resets);
            if let Some(window) = &mut window {
                match window.update(|bytes| console.push_input(bytes), || cpu.memory().frame()) {
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
        }
    });
    // The console and the screen are only for a run that did not fail.
    let shown = if result.is_ok() {
        let mut out = BufWriter::new(Output::stdout());
        let printed = if keys.is_none() {
            print_console(&console, &mut out).and_then(|()| out.flush())
        } else {
            Ok(())
        };
        let shot = write_screenshot(&mut cpu, &options);
        output_result(printed).and(shot)
    } else {
        Ok(())
    };
    settle(
        result,
        [finish_trace(trace), shown, saved],
        &mut Output::stderr(),
    )
}

/// The outcome of a run, given its other errors in the order they happened (the trace, the
/// screenshot, the save). When more than one failed, every one but the last is said to `say`
/// (standard error, for the caller) and the last is returned for the caller to say, so that none
/// hides another, and the run's own comes first.
fn settle(
    run: Result<bool, RunError>,
    rest: [Result<(), RunError>; 3],
    say: &mut impl Write,
) -> Result<bool, RunError> {
    let mut errors: Vec<RunError> = rest.into_iter().filter_map(Result::err).collect();
    match run {
        Ok(ok) if errors.is_empty() => return Ok(ok),
        Ok(_) => {}
        Err(run) => errors.insert(0, run),
    }
    let last = errors.pop().expect("something failed");
    for err in errors {
        let _ = writeln!(say, "cmm2-emulator: {err}");
    }
    Err(last)
}

/// Plugs a USB keyboard into the port, and returns the keys that drive it. The core attaches
/// nothing itself, so that a run with no window sees no keyboard.
fn plug_in_keyboard(cpu: &Cpu) -> UsbKeys {
    let report = KeyReport::new();
    cpu.memory()
        .usb_port()
        .attach(HidKeyboard::new(report.clone()));
    UsbKeys::new(report)
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
        say("system reset");
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
    console.push_input(&script.feed(&output));
    Ok(())
}

/// A signal came while the output was waiting to be written, as an error that is the signal
/// ending the run and not a failure of the output.
#[derive(Debug)]
struct SignalEnd;

impl fmt::Display for SignalEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("interrupted by a signal")
    }
}

impl std::error::Error for SignalEnd {}

fn is_signal_end(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<SignalEnd>())
}

/// Says a line on standard error, with the program's name in front. It goes through the same
/// writer as the rest of the output of a run, so that a standard error that nobody reads does
/// not stop a signal from ending the run (and a line that a signal gave up on is dropped).
fn say(line: impl fmt::Display) {
    let line = format!("cmm2-emulator: {line}\n");
    let _ = Output::stderr().write_all(line.as_bytes());
}

/// The most that is written in one go to a descriptor that could not be reopened (see
/// `Output::of`), by the kind of file it is, so that a write after `poll` has said it can take
/// one does not wait for room. A pipe takes up to PIPE_BUF bytes at once if it has room for one
/// at all. For a terminal `poll` only says that fewer than 256 characters are waiting in it and
/// that it has room for at least one more (as Linux has it, from memory, not checked), which
/// does not say how many: so one byte is written at a time. Even that can wait if the byte
/// becomes two (a newline is turned into CR LF) and there was room for one only. For anything
/// else, which is also given a small chunk, `poll` is a guess too.
fn chunk_for(fd: libc::c_int) -> usize {
    // SAFETY: isatty only looks at the descriptor.
    if unsafe { libc::isatty(fd) } == 1 {
        return 1;
    }
    match file_type(fd) {
        libc::S_IFIFO => libc::PIPE_BUF,
        _ => 256,
    }
}

/// Whether opening the file `fd` is open on again, by its path in /proc, gives another way to
/// the same thing and does nothing else. That is so for a pipe or FIFO, and for a terminal's
/// slave side. It is not for the master side of a pseudo-terminal, whose path is /dev/ptmx and
/// opening that makes a new pair (TIOCGPTN is the question that only a master answers), and
/// not for any other device, which may do something when it is opened (/dev/net/tun, a
/// watchdog), or a socket, which cannot be opened.
fn can_be_reopened(fd: libc::c_int) -> bool {
    match file_type(fd) {
        libc::S_IFIFO => true,
        libc::S_IFCHR => {
            let mut number: libc::c_uint = 0;
            // SAFETY: TIOCGPTN fills one c_uint through the pointer, and fails on anything that
            // is not a pseudo-terminal master.
            let master = unsafe { libc::ioctl(fd, libc::TIOCGPTN, &mut number) } == 0;
            // SAFETY: isatty only looks at the descriptor.
            !master && unsafe { libc::isatty(fd) } == 1
        }
        _ => false,
    }
}

/// Whether `fd` is one of the memory character devices that take any write at once and need no
/// waiting for: /dev/null (1, 3), /dev/zero (1, 5), /dev/full (1, 7) and the random ones (1, 8
/// and 9). Not the others of major 1, which are different: /dev/mem and /dev/port are the
/// machine's, and /dev/kmsg refuses a write of more than about 1 KB. A terminal is not one.
fn never_blocks(fd: libc::c_int) -> bool {
    // SAFETY: fstat fills a plain stat; isatty only looks at the descriptor.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut stat) == 0
            && stat.st_mode & libc::S_IFMT == libc::S_IFCHR
            && memory_device_never_blocks(libc::major(stat.st_rdev), libc::minor(stat.st_rdev))
            && libc::isatty(fd) != 1
    }
}

/// The device numbers of the memory devices that take any write at once (see `never_blocks`).
fn memory_device_never_blocks(major: u32, minor: u32) -> bool {
    major == 1 && matches!(minor, 3 | 5 | 7 | 8 | 9)
}

/// The type bits of the mode of the file `fd` is open on (S_IFREG and so on).
fn file_type(fd: libc::c_int) -> libc::mode_t {
    // SAFETY: fstat fills a plain stat.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut stat);
        stat.st_mode & libc::S_IFMT
    }
}

/// A file or standard output or error as the descriptor is, with no buffer of ours, that does not
/// wait in a write for a reader that is not reading, so that a signal can end the run whatever
/// the output is blocked on. The descriptor is reopened through /proc/self/fd, with O_NONBLOCK,
/// which for a terminal, a pipe or a FIFO is an open file description of its own: non-blocking
/// here without making stdin and stderr, which share the original, non-blocking too. Writes are
/// large (64 KB) and never wait. When the descriptor takes no more (EAGAIN) it waits in `poll`
/// for the descriptor to take a write, or for a signal, whichever is first (the signal handler
/// writes to the pipe that `signals::wake_fd` is the end of), so there is no instant in which a
/// signal can come and be missed: either the flag was set before the wait, and the wake pipe is
/// readable, or the signal comes during the wait and wakes it. After a signal what the
/// descriptor can take is still written, and what it cannot is given up as `SignalEnd`.
///
/// What cannot be reopened (a socket, a pseudo-terminal master, another device, or where there
/// is no /proc) is written as it is,
/// in the small chunks of `chunk_for` after a `poll`, which leaves a gap for a signal between the
/// `poll` and the `write` that only a second signal gets out of. A regular file never waits, so
/// it is written as it is, in large chunks.
struct Output {
    fd: libc::c_int,
    chunk: usize,
    /// `fd` is the reopened, non-blocking descriptor, and writes need not wait for a `poll`.
    own: bool,
    /// The file `fd` belongs to when it is not one of the process's own, which has to stay open
    /// as long as this writes.
    _file: Option<File>,
}

/// The most written to a descriptor in one go when it never waits.
const BIG_CHUNK: usize = 1 << 16;

impl Output {
    fn stdout() -> Output {
        Output::of(1, None)
    }

    fn stderr() -> Output {
        Output::of(2, None)
    }

    fn file(file: File) -> Output {
        use std::os::fd::AsRawFd;
        Output::of(file.as_raw_fd(), Some(file))
    }

    fn of(fd: libc::c_int, file: Option<File>) -> Output {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        if file_type(fd) == libc::S_IFREG || never_blocks(fd) {
            return Output {
                fd,
                chunk: BIG_CHUNK,
                own: false,
                _file: file,
            };
        }
        // O_NOCTTY, as opening a terminal must not make it the one this process is run from.
        let own = can_be_reopened(fd)
            .then(|| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
                    .open(format!("/proc/self/fd/{fd}"))
                    .ok()
            })
            .flatten();
        match own {
            Some(own) => Output {
                fd: own.as_raw_fd(),
                chunk: BIG_CHUNK,
                own: true,
                _file: Some(own),
            },
            None => Output {
                fd,
                chunk: chunk_for(fd),
                own: false,
                _file: file,
            },
        }
    }

    /// Waits until the descriptor can take a write, which is `Ok`, or until a signal has come
    /// and it cannot, which is `SignalEnd`.
    fn wait(&self) -> io::Result<()> {
        loop {
            let mut waits = [
                libc::pollfd {
                    fd: self.fd,
                    events: libc::POLLOUT,
                    revents: 0,
                },
                libc::pollfd {
                    fd: signals::wake_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: the pointer and count are those of a live array.
            if unsafe { libc::poll(waits.as_mut_ptr(), 2, -1) } < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            // Anything on the descriptor (room, or an error to be had from the write) first.
            return if waits[0].revents == 0 {
                Err(io::Error::other(SignalEnd))
            } else {
                Ok(())
            };
        }
    }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let chunk = &bytes[..bytes.len().min(self.chunk)];
        loop {
            // A descriptor that is not our own may wait in a write, so it is asked first.
            if !self.own {
                self.wait()?;
            }
            // SAFETY: the pointer and length are those of a live slice.
            let wrote = unsafe { libc::write(self.fd, chunk.as_ptr().cast(), chunk.len()) };
            if let Ok(wrote) = usize::try_from(wrote) {
                return Ok(wrote);
            }
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock => self.wait()?,
                _ => return Err(err),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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
fn print_console(console: &Console, out: &mut impl Write) -> io::Result<()> {
    let output = console.take_output();
    if output.is_empty() {
        return Ok(());
    }
    writeln!(out, "console output ({} bytes):", output.len())?;
    let text = String::from_utf8_lossy(&output);
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

fn print_summary(
    cpu: &mut Cpu,
    stop: &Stop,
    interrupted: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    match stop {
        Stop::InstructionLimit if interrupted => writeln!(out, "stopped: interrupted by a signal")?,
        Stop::InstructionLimit => writeln!(out, "stopped: instruction limit")?,
        Stop::Address(addr) => writeln!(out, "stopped: address {addr:#010X}")?,
        Stop::Fault(fault) => writeln!(out, "stopped: {}", fault.description)?,
    }
    writeln!(out, "instructions {}", cpu.instruction_count())?;
    writeln!(out, "pc {:#010X}", cpu.pc())?;
    let unmapped = cpu.memory().take_unmapped();
    if unmapped.is_empty() {
        return Ok(());
    }
    writeln!(out, "unmapped accesses {}", unmapped.len())?;
    for access in unmapped.iter().take(UNMAPPED_SHOWN) {
        let kind = match access.kind {
            AccessKind::Read => "read",
            AccessKind::Write(_) => "write",
        };
        writeln!(
            out,
            "  {:#010X} {kind} {:#010X} size {}",
            access.pc, access.addr, access.size
        )?;
    }
    Ok(())
}

/// What writing to standard output came to: a write that a signal gave up on is the run ending
/// and not an error.
fn output_result(result: io::Result<()>) -> Result<(), RunError> {
    match result {
        Err(err) if !is_signal_end(&err) => Err(RunError::Console(err)),
        _ => Ok(()),
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
    use cmm2_core::PERSISTENT_SIZE;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    #[test]
    fn the_quit_key_and_what_follows_it_are_not_typed() {
        assert_eq!(until_quit(b"ab"), (&b"ab"[..], false));
        assert_eq!(until_quit(b"a\x1Db"), (&b"a"[..], true));
        assert_eq!(until_quit(b"\x1D"), (&b""[..], true));
    }

    // A directory under the temp directory that is this test's own.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cmm2-main-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_is_written_whole_and_replaced_whole() {
        let dir = scratch("atomic-plain");
        let file = dir.join("a.img");
        write_atomically(file.to_str().unwrap(), b"first").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"first");
        write_atomically(file.to_str().unwrap(), b"second, longer").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"second, longer");
        // The temporary file does not stay.
        assert_eq!(files_in(&dir), ["a.img"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_is_written_through_and_stays_a_symlink() {
        let dir = scratch("atomic-link");
        let real = dir.join("real.img");
        let link = dir.join("link.img");
        std::fs::write(&real, b"old").unwrap();
        symlink("real.img", &link).unwrap();

        write_atomically(link.to_str().unwrap(), b"new").unwrap();

        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
        assert_eq!(files_in(&dir), ["link.img", "real.img"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_to_another_directory_is_written_there() {
        let dir = scratch("atomic-link-dir");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("real.img"), b"old").unwrap();
        symlink("elsewhere/real.img", dir.join("link.img")).unwrap();

        write_atomically(dir.join("link.img").to_str().unwrap(), b"new").unwrap();

        assert_eq!(std::fs::read(elsewhere.join("real.img")).unwrap(), b"new");
        // The temporary file was made beside the real file, and is gone from both.
        assert_eq!(files_in(&elsewhere), ["real.img"]);
        assert_eq!(files_in(&dir), ["elsewhere", "link.img"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_permissions_of_the_old_file_are_kept() {
        let dir = scratch("atomic-mode");
        let file = dir.join("a.img");
        std::fs::write(&file, b"old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();

        write_atomically(file.to_str().unwrap(), b"new").unwrap();

        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o640);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_temporary_file_is_never_taken_over_and_a_stale_one_does_not_block_the_save() {
        let dir = scratch("atomic-unique");
        let file = dir.join("a.img");
        // Somebody else's file with the name another process would use, and one that an
        // earlier crash left under this very process id (a container starts at the same one).
        let other = dir.join("a.img.1.1.tmp");
        std::fs::write(&other, b"theirs").unwrap();
        let stale = dir.join(format!("a.img.{}.tmp", std::process::id()));
        std::fs::write(&stale, b"stale").unwrap();

        write_atomically(file.to_str().unwrap(), b"new").unwrap();
        write_atomically(file.to_str().unwrap(), b"newer").unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), b"newer");
        assert_eq!(std::fs::read(&other).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&stale).unwrap(), b"stale");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dangling_symlink_is_written_through_and_the_target_is_made() {
        let dir = scratch("atomic-dangling");
        let link = dir.join("link.img");
        symlink("real.img", &link).unwrap();

        write_atomically(link.to_str().unwrap(), b"first").unwrap();

        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read(dir.join("real.img")).unwrap(), b"first");
        assert_eq!(files_in(&dir), ["link.img", "real.img"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_chain_of_links_to_a_target_that_is_not_there_is_followed_to_its_end() {
        let dir = scratch("atomic-chain");
        std::fs::create_dir(dir.join("sub")).unwrap();
        symlink("sub/second", dir.join("first")).unwrap();
        // Relative to the directory of the link that holds it, which is sub.
        symlink("../elsewhere.img", dir.join("sub/second")).unwrap();

        write_atomically(dir.join("first").to_str().unwrap(), b"data").unwrap();

        assert_eq!(std::fs::read(dir.join("elsewhere.img")).unwrap(), b"data");
        assert!(
            std::fs::symlink_metadata(dir.join("first"))
                .unwrap()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(dir.join("sub/second"))
                .unwrap()
                .is_symlink()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_loop_of_links_is_an_error() {
        let dir = scratch("atomic-loop");
        symlink("b", dir.join("a")).unwrap();
        symlink("a", dir.join("b")).unwrap();

        let err = write_atomically(dir.join("a").to_str().unwrap(), b"x").unwrap_err();

        assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
        assert_eq!(files_in(&dir), ["a", "b"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_file() {
        let dir = scratch("atomic-fail");
        // The rename onto a directory fails, after the temporary file is made.
        let target = dir.join("a");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(target.join("inside")).unwrap();

        assert!(write_atomically(target.to_str().unwrap(), b"x").is_err());

        assert_eq!(files_in(&dir), ["a"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_with_no_directory_is_written_in_the_current_one() {
        let name = format!("cmm2-main-{}-relative.img", std::process::id());
        write_atomically(&name, b"here").unwrap();
        assert_eq!(std::fs::read(&name).unwrap(), b"here");
        std::fs::remove_file(&name).unwrap();
    }

    // The levels a bit-banging guest drives, as the firmware does it: port C pins 10 (clock)
    // and 12 (data in) and port D pin 2 (chip select) as outputs, pin 11 of C the card's data.
    const GPIOC: u32 = 0x5802_0800;
    const GPIOD: u32 = 0x5802_0C00;
    const MODER: u32 = 0x00;
    const BSRR: u32 = 0x18;

    fn wired_card(blocks: usize) -> (SdSlot, cmm2_core::MemoryMap) {
        let mut memory = cmm2_memory(&Irqs::new());
        let sd = memory.sd_slot();
        sd.insert(vec![0x11; blocks * 512]).unwrap();
        memory.write32(GPIOC + MODER, !(0x3F << 20) | 0b01_00_01 << 20);
        memory.write32(GPIOD + MODER, !(3 << 4) | 1 << 4);
        memory.write32(GPIOD + BSRR, 1 << 18); // chip select low
        (sd, memory)
    }

    fn spi_byte(memory: &mut cmm2_core::MemoryMap, byte: u8) {
        for bit in (0..8).rev() {
            let data = if byte >> bit & 1 != 0 {
                1 << 12
            } else {
                1 << 28
            };
            memory.write32(GPIOC + BSRR, data);
            memory.write32(GPIOC + BSRR, 1 << 10);
            memory.write32(GPIOC + BSRR, 1 << 26);
        }
    }

    fn spi_command(memory: &mut cmm2_core::MemoryMap, index: u8, arg: u32) {
        for byte in [0x40 | index]
            .into_iter()
            .chain(arg.to_be_bytes())
            .chain([1])
        {
            spi_byte(memory, byte);
        }
        for _ in 0..3 {
            spi_byte(memory, 0xFF);
        }
    }

    // Brings the card up, and writes block 1 with `fill` through CMD24.
    fn write_block_one(memory: &mut cmm2_core::MemoryMap, fill: u8) {
        spi_command(memory, 0, 0);
        spi_command(memory, 55, 0);
        spi_command(memory, 41, 0x4000_0000);
        spi_command(memory, 24, 1);
        spi_byte(memory, 0xFE);
        for _ in 0..512 + 2 {
            spi_byte(memory, fill);
        }
        spi_byte(memory, 0xFF);
    }

    fn sd_options(path: &Path) -> RunOptions {
        RunOptions {
            sd: Some(path.to_str().unwrap().to_string()),
            ..RunOptions::default()
        }
    }

    #[test]
    fn a_card_the_guest_wrote_to_is_written_back() {
        let dir = scratch("sd-dirty");
        let file = dir.join("card.img");
        std::fs::write(&file, vec![0x11; 2 * 512]).unwrap();
        let (sd, mut memory) = wired_card(2);

        write_block_one(&mut memory, 0x99);
        save_sd(&sd, &sd_options(&file)).unwrap();

        let saved = std::fs::read(&file).unwrap();
        assert_eq!(saved[..512], [0x11; 512]);
        assert_eq!(saved[512..], [0x99; 512]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_card_that_was_not_written_to_leaves_the_file_alone() {
        let dir = scratch("sd-clean");
        let file = dir.join("card.img");
        std::fs::write(&file, vec![0x11; 2 * 512]).unwrap();
        let inode = std::fs::metadata(&file).unwrap().ino();
        let (sd, mut memory) = wired_card(2);
        // Initialised and read from, but never written to.
        spi_command(&mut memory, 0, 0);
        spi_command(&mut memory, 55, 0);
        spi_command(&mut memory, 41, 0x4000_0000);
        spi_command(&mut memory, 17, 0);
        // Somebody else changes the file during the run.
        std::fs::write(&file, vec![0x22; 2 * 512]).unwrap();
        let inode_then = std::fs::metadata(&file).unwrap().ino();

        save_sd(&sd, &sd_options(&file)).unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), vec![0x22; 2 * 512]);
        assert_eq!(std::fs::metadata(&file).unwrap().ino(), inode_then);
        assert_eq!(inode, inode_then);
        assert_eq!(files_in(&dir), ["card.img"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_protected_card_the_guest_wrote_to_anyway_is_not_written_back() {
        let dir = scratch("sd-protect-write");
        let file = dir.join("card.img");
        std::fs::write(&file, vec![0x11; 2 * 512]).unwrap();
        let inode = std::fs::metadata(&file).unwrap().ino();
        let (sd, mut memory) = wired_card(2);
        sd.set_write_protected(true);
        write_block_one(&mut memory, 0x99);
        assert!(sd.modified());

        save_sd(&sd, &sd_options(&file)).unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), vec![0x11; 2 * 512]);
        assert_eq!(std::fs::metadata(&file).unwrap().ino(), inode);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_sd_file_under_512_kb_is_refused() {
        let dir = scratch("sd-small");
        let file = dir.join("card.img");
        std::fs::write(&file, vec![0; MIN_IMAGE_SIZE - 512]).unwrap();

        let err = load_sd(&SdSlot::new(), file.to_str().unwrap()).unwrap_err();

        assert_eq!(
            err.to_string(),
            format!(
                "{}: 523776 bytes is smaller than the 524288 bytes (512 KB) of the smallest card",
                file.display()
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // The tests that panic on purpose and look at `HELD_PANICS` take turns, as it is shared.
    static PANICS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // As root a mode bit stops nothing, so what depends on one is not tried.
    fn root() -> bool {
        // SAFETY: geteuid has no arguments and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    fn mode(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn the_ds3231_is_fitted_and_shows_the_hosts_time() {
        let dir = scratch("ds3231-fitted");
        let firmware = dir.join("firmware.bin");
        let mut image = vec![0; 64];
        image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x0800_0021u32.to_le_bytes());
        std::fs::write(&firmware, image).unwrap();

        let machine = build_machine(firmware.to_str().unwrap(), &RunOptions::default()).unwrap();

        // The chip's own default clock stands at the year 2000, until the host's is attached.
        let shown = machine.cpu.memory().ds3231().time();
        assert_ne!(shown.year, 2000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_ds3231_offset_is_kept_in_the_flash_file_between_runs() {
        let dir = scratch("flash-offset");
        let file = dir.join("board.flash");
        let path = file.to_str().unwrap();
        let options = RunOptions {
            flash: Some(path.to_string()),
            ..RunOptions::default()
        };
        let mut first = idle_cpu();
        first
            .memory()
            .ds3231()
            .set_offset_millis(Some(-4 * 86_400_000 - 5000))
            .unwrap();
        save_flash(&mut first, &options).unwrap();

        let mut second = idle_cpu();
        load_flash(&mut second, path).unwrap();

        assert_eq!(
            second.memory().ds3231().offset_millis(),
            Some(-4 * 86_400_000 - 5000)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_ds3231_never_set_keeps_the_flash_file_the_size_it_was_before_it_was_modelled() {
        let dir = scratch("flash-size-unset");
        let file = dir.join("board.flash");
        let options = RunOptions {
            flash: Some(file.to_str().unwrap().to_string()),
            ..RunOptions::default()
        };
        let mut cpu = idle_cpu();
        save_flash(&mut cpu, &options).unwrap();
        assert_eq!(std::fs::read(&file).unwrap().len(), PERSISTENT_SIZE);
        assert_eq!(PERSISTENT_SIZE, 917_512);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flash_file_with_a_ds3231_offset_that_is_too_far_away_is_refused() {
        let dir = scratch("flash-offset-far");
        let file = dir.join("board.flash");
        let mut cpu = idle_cpu();
        let mut bytes = cpu.persistent();
        bytes.extend(i64::MAX.to_le_bytes());
        bytes.extend([1, 0x88]);
        std::fs::write(&file, bytes).unwrap();

        let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

        assert!(err.to_string().ends_with("more than 200 years"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_read_only_flash_file_is_saved_all_the_same_as_the_save_replaces_the_file() {
        let dir = scratch("flash-readonly");
        let file = dir.join("board.flash");
        let mut cpu = idle_cpu();
        std::fs::write(&file, cpu.persistent()).unwrap();
        mode(&file, 0o444);
        let path = file.to_str().unwrap();

        load_flash(&mut cpu, path).unwrap();
        assert_eq!(why_not_saveable(path), None);
        let options = RunOptions {
            flash: Some(path.to_string()),
            ..RunOptions::default()
        };
        save_flash(&mut cpu, &options).unwrap();

        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o444);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flash_file_in_a_directory_that_cannot_be_written_is_refused_at_the_start() {
        if root() {
            return;
        }
        let dir = scratch("flash-dir-readonly");
        let file = dir.join("board.flash");
        let mut cpu = idle_cpu();
        std::fs::write(&file, cpu.persistent()).unwrap();
        mode(&dir, 0o555);
        let path = file.to_str().unwrap();

        // The file is writable and exists; the rename of a new one over it is what fails. A
        // first run, with no file yet, is refused too.
        let err = load_flash(&mut cpu, path).unwrap_err();
        let first = load_flash(&mut cpu, dir.join("new.flash").to_str().unwrap()).unwrap_err();

        mode(&dir, 0o755);
        assert_eq!(
            err.to_string(),
            format!(
                "{path}: its directory is not writable, so the run could not save its flash and clock"
            )
        );
        assert!(matches!(first, RunError::Flash { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flash_link_is_judged_by_the_directory_of_the_file_it_points_to() {
        if root() {
            return;
        }
        let dir = scratch("flash-link-dir");
        std::fs::create_dir(dir.join("locked")).unwrap();
        symlink("locked/board.flash", dir.join("link.flash")).unwrap();
        mode(&dir.join("locked"), 0o555);

        let why = why_not_saveable(dir.join("link.flash").to_str().unwrap());

        mode(&dir.join("locked"), 0o755);
        assert_eq!(why.as_deref(), Some("its directory is not writable"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_card_whose_file_cannot_be_written_back_is_write_protected_for_each_reason() {
        if root() {
            return;
        }
        let dir = scratch("sd-why");
        let file = dir.join("card.img");
        std::fs::write(&file, vec![0; MIN_IMAGE_SIZE]).unwrap();
        let path = file.to_str().unwrap();
        let protected = |path: &str| {
            let sd = SdSlot::new();
            load_sd(&sd, path).unwrap();
            sd.write_protected()
        };
        assert!(!protected(path));

        mode(&file, 0o444);
        assert_eq!(
            why_write_protected(path).as_deref(),
            Some("the file is read-only")
        );
        assert!(protected(path));

        mode(&file, 0o644);
        mode(&dir, 0o555);
        assert_eq!(
            why_write_protected(path).as_deref(),
            Some("its directory is not writable")
        );
        assert!(protected(path));

        mode(&dir, 0o755);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flash_file_in_a_directory_that_is_not_there_is_refused_at_the_start() {
        let dir = scratch("flash-nodir");
        let file = dir.join("nodir/x.flash");
        let path = file.to_str().unwrap();

        let err = load_flash(&mut idle_cpu(), path).unwrap_err();

        assert_eq!(
            err.to_string(),
            format!(
                "{path}: its directory {}/nodir does not exist, so the run could not save its flash and clock",
                dir.display()
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flash_path_through_a_file_or_a_loop_is_refused_at_the_start() {
        let dir = scratch("flash-badpath");
        std::fs::write(dir.join("plain"), b"x").unwrap();
        symlink("loop", dir.join("loop")).unwrap();

        let through_file = dir.join("plain/x.flash");
        let err = load_flash(&mut idle_cpu(), through_file.to_str().unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{}: {}/plain is not a directory, so the run could not save its flash and clock",
                through_file.display(),
                dir.display()
            )
        );
        let looped = dir.join("loop");
        let err = load_flash(&mut idle_cpu(), looped.to_str().unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{}: its path is a loop of symbolic links, so the run could not save its flash and clock",
                looped.display()
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_is_not_a_regular_file_is_refused_for_the_flash_and_the_card() {
        let dir = scratch("not-regular");
        let fifo = dir.join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo with a NUL-terminated name that lives through the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

        // None of them is opened, or this would never return for the FIFO.
        for path in [fifo.as_path(), dir.as_path(), Path::new("/dev/null")] {
            let path = path.to_str().unwrap();
            let flash = load_flash(&mut idle_cpu(), path).unwrap_err();
            assert_eq!(flash.to_string(), format!("{path}: not a regular file"));
            let card = load_sd(&SdSlot::new(), path).unwrap_err();
            assert_eq!(card.to_string(), format!("{path}: not a regular file"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_to_a_regular_file_is_one() {
        let dir = scratch("regular-link");
        std::fs::write(dir.join("real"), b"x").unwrap();
        symlink("real", dir.join("link")).unwrap();
        assert!(check_regular(dir.join("link").to_str().unwrap()).is_ok());
        assert!(check_regular(dir.join("missing").to_str().unwrap()).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_in_a_sticky_directory_is_only_replaced_by_its_owner_or_the_directorys() {
        let sticky = 0o1777;
        // (directory mode, directory owner, file owner, user): refused only when sticky and
        // the user is neither owner and not privileged.
        assert!(sticky_refuses(sticky, 0, 1000, 1001, false));
        assert!(!sticky_refuses(sticky, 0, 1000, 1000, false));
        assert!(!sticky_refuses(sticky, 1001, 1000, 1001, false));
        // It is CAP_FOWNER that lets a user through, and not their number: root without it is
        // refused, and a user with it is not.
        assert!(sticky_refuses(sticky, 1, 1000, 0, false));
        assert!(!sticky_refuses(sticky, 0, 1000, 1001, true));
        assert!(!sticky_refuses(0o777, 0, 1000, 1001, false));
    }

    #[test]
    fn an_ordinary_file_is_not_immutable_and_an_unreadable_flag_is_not_a_reason() {
        let dir = scratch("immutable");
        std::fs::write(dir.join("a"), b"x").unwrap();
        assert!(!is_immutable(&File::open(dir.join("a")).unwrap()));
        assert_eq!(FS_IMMUTABLE_OR_APPEND, 0x30);
        // A file system that does not know the ioctl leaves the flags 0.
        assert!(!is_immutable(&File::open("/dev/null").unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_card_that_cannot_be_written_back_is_an_error() {
        let dir = scratch("sd-readonly");
        let file = dir.join("card.img");
        std::fs::write(&file, vec![0x11; 2 * 512]).unwrap();
        let (sd, mut memory) = wired_card(2);
        write_block_one(&mut memory, 0x99);
        // No temporary file can be made in a directory that cannot be written to (as root it
        // can, so the check is skipped there).
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // SAFETY: geteuid has no arguments and cannot fail.
        let root = unsafe { libc::geteuid() } == 0;

        let saved = save_sd(&sd, &sd_options(&file));

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !root {
            assert!(matches!(saved, Err(RunError::Sd { .. })));
        }
        assert_eq!(std::fs::read(&file).unwrap(), vec![0x11; 2 * 512]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // A machine that does nothing, with `options` for its files.
    fn idle_cpu() -> Cpu {
        let mut image = vec![0; 64];
        image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x0800_0021u32.to_le_bytes());
        let irqs = Irqs::new();
        Cpu::new(
            &Firmware::from_bytes(image).unwrap(),
            cmm2_memory(&irqs),
            irqs,
        )
        .unwrap()
    }

    #[test]
    fn the_files_are_saved_when_the_run_returns() {
        // `run_and_save` sets and clears the process-wide `HOLDING`, so it takes its turn.
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = scratch("save-return");
        let file = dir.join("board.flash");
        let options = RunOptions {
            flash: Some(file.to_str().unwrap().to_string()),
            ..RunOptions::default()
        };
        let mut cpu = idle_cpu();

        let (answer, saved) = run_and_save(&mut cpu, &SdSlot::new(), &options, |_| 7);

        assert_eq!(answer, 7);
        saved.unwrap();
        assert!(file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_files_are_saved_when_the_run_panics_and_the_panic_carries_on() {
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = scratch("save-panic");
        let file = dir.join("board.flash");
        let options = RunOptions {
            flash: Some(file.to_str().unwrap().to_string()),
            ..RunOptions::default()
        };
        let mut cpu = idle_cpu();

        // The message that was held back goes nowhere that shows.
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            run_and_save_saying(
                &mut cpu,
                &SdSlot::new(),
                &options,
                |_| -> () { panic!("a bug in the emulator") },
                &mut io::sink(),
            )
        }));

        let panic = panicked.expect_err("the panic is not swallowed");
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"a bug in the emulator"));
        assert_eq!(std::fs::read(&file).unwrap().len(), PERSISTENT_SIZE);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_panic_message_that_was_held_back_is_said_after_the_files_are_saved() {
        // A writer that notes whether the flash file was there when the message came.
        struct Probe {
            file: PathBuf,
            seen: Option<(bool, String)>,
            all: String,
        }
        impl Write for Probe {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let text = String::from_utf8_lossy(bytes).into_owned();
                self.all.push_str(&text);
                self.seen = Some((self.file.exists(), text));
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The real hook is in, as for the tests of it, so it holds this test's own panic too.
        with_the_hook();
        let dir = scratch("late-panic");
        let file = dir.join("board.flash");
        let options = RunOptions {
            flash: Some(file.to_str().unwrap().to_string()),
            ..RunOptions::default()
        };
        let mut probe = Probe {
            file: file.clone(),
            seen: None,
            all: String::new(),
        };
        let mut cpu = idle_cpu();
        // Two messages that were already held when the run panicked.
        hold_panic("the message\n".to_string());
        hold_panic("and another\n".to_string());

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            run_and_save_saying(
                &mut cpu,
                &SdSlot::new(),
                &options,
                |_| -> () { panic!("a bug in the emulator") },
                &mut probe,
            )
        }));

        assert!(panicked.is_err());
        // Said in the order they came, after the save, the run's own panic (held by the hook)
        // last; and not held any more.
        assert!(
            probe.all.starts_with("the message\nand another\n"),
            "{}",
            probe.all
        );
        assert!(probe.all.contains("a bug in the emulator"), "{}", probe.all);
        let last = probe.seen.as_ref().unwrap();
        assert!(last.0, "the file was there when the last was said");
        assert!(last.1.contains("a bug in the emulator"), "{last:?}");
        assert!(HELD_PANICS.lock().unwrap().is_empty());
        assert!(!HOLDING.load(std::sync::atomic::Ordering::Acquire));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_panic_during_a_run_is_held_whatever_standard_error_is_and_only_so_many_are_kept() {
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The message is not written when it happens, so what standard error is (a pipe with
        // room for a page, or a nearly full one) does not decide whether the save is waited for.
        for n in 0..HELD_PANICS_MAX + 4 {
            hold_panic(format!("panic {n}\n"));
        }
        let mut said = Vec::new();
        say_late_panic(&mut said);
        let said = String::from_utf8(said).unwrap();
        // The first ones, oldest first, and no more than the bound.
        assert_eq!(said.lines().count(), HELD_PANICS_MAX);
        assert!(said.starts_with("panic 0\npanic 1\n"));
        assert!(!said.contains(&format!("panic {HELD_PANICS_MAX}\n")));
        let mut again = Vec::new();
        say_late_panic(&mut again);
        assert!(again.is_empty());
    }

    #[test]
    fn a_run_is_marked_as_holding_panic_messages_while_it_runs_and_not_after() {
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cpu = idle_cpu();
        let (during, _) = run_and_save(&mut cpu, &SdSlot::new(), &RunOptions::default(), |_| {
            HOLDING.load(std::sync::atomic::Ordering::Acquire)
        });
        assert!(during);
        assert!(!HOLDING.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn a_panic_is_described_in_the_words_of_the_default_message() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("it broke");
        assert_eq!(
            describe_panic(Some("main"), Some(("src/x.rs", 3, 9)), &*payload),
            "thread 'main' panicked at src/x.rs:3:9:\nit broke\n"
        );
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("formatted 7"));
        assert_eq!(
            describe_panic(None, None, &*payload),
            "thread '<unnamed>' panicked:\nformatted 7\n"
        );
    }

    // Puts the panic hook in, once, for the tests that look at it; it only acts during a run.
    fn with_the_hook() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(install_panic_hook);
        set_main_thread();
    }

    #[test]
    fn the_installed_hook_holds_a_panic_of_the_run_and_it_is_said_after_the_save() {
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        with_the_hook();
        struct Probe {
            file: PathBuf,
            said: Vec<(bool, String)>,
        }
        impl Write for Probe {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.said.push((
                    self.file.exists(),
                    String::from_utf8_lossy(bytes).into_owned(),
                ));
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let dir = scratch("hook-panic");
        let file = dir.join("board.flash");
        let options = RunOptions {
            flash: Some(file.to_str().unwrap().to_string()),
            ..RunOptions::default()
        };
        let mut probe = Probe {
            file: file.clone(),
            said: Vec::new(),
        };
        let mut cpu = idle_cpu();

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            run_and_save_saying(
                &mut cpu,
                &SdSlot::new(),
                &options,
                |_| -> () { panic!("a bug in the run") },
                &mut probe,
            )
        }));

        assert!(panicked.is_err());
        // Held, and not written when it happened: it is said once, after the file is there.
        assert_eq!(probe.said.len(), 1, "{:?}", probe.said);
        assert!(probe.said[0].0, "the file was written first");
        assert!(probe.said[0].1.contains("panicked at"), "{:?}", probe.said);
        assert!(
            probe.said[0].1.contains("a bug in the run"),
            "{:?}",
            probe.said
        );
        assert!(HELD_PANICS.lock().unwrap().is_empty());
        // A panic after the run is the default hook's again, and the thread can hold again.
        assert!(!PANIC_SEEN.with(std::cell::Cell::get));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_panic_puts_the_terminal_back_on_the_main_thread_and_not_on_another() {
        let _turn = PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        with_the_hook();
        // A terminal as it is made, cooked: what raw mode saves and a panic puts back.
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in two descriptors, which are closed below.
        unsafe {
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null()
                ),
                0
            );
        }
        let lflag = || {
            // SAFETY: tcgetattr fills a plain termios.
            unsafe {
                let mut termios: libc::termios = std::mem::zeroed();
                libc::tcgetattr(slave, &mut termios);
                termios.c_lflag
            }
        };
        let panic_in_a_run = || {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                run_and_save_saying(
                    &mut idle_cpu(),
                    &SdSlot::new(),
                    &RunOptions::default(),
                    |_| -> () { panic!("a bug in the run") },
                    &mut io::sink(),
                )
            }));
        };
        let raw = terminal::RawMode::enable_on(slave).unwrap();
        let in_raw_mode = lflag();

        // Another thread is the main one: this panic leaves the terminal as it is.
        *MAIN_THREAD.lock().unwrap() = Some(
            std::thread::spawn(|| std::thread::current().id())
                .join()
                .unwrap(),
        );
        panic_in_a_run();
        assert_eq!(
            lflag(),
            in_raw_mode,
            "restored from a thread that is not the main one"
        );

        // This thread is the main one: the terminal is put back before the message.
        set_main_thread();
        panic_in_a_run();
        assert_ne!(lflag(), in_raw_mode, "not restored from the main thread");
        std::mem::forget(raw);
        // SAFETY: closing the two descriptors made above.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }

    // A panic in a `Drop` while a panic is being unwound, which aborts the process. The test
    // below runs this one in a process of its own (it is a test that does nothing without the
    // variable the other sets).
    #[test]
    fn double_panic_in_a_process_of_its_own() {
        if std::env::var_os("CMM2_TEST_DOUBLE_PANIC").is_none() {
            return;
        }
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) {
                panic!("the second panic");
            }
        }
        with_the_hook();
        let _ = run_and_save(
            &mut idle_cpu(),
            &SdSlot::new(),
            &RunOptions::default(),
            |_| -> () {
                let _bomb = Bomb;
                panic!("the first panic")
            },
        );
    }

    #[test]
    fn a_double_panic_writes_everything_that_was_held_and_the_new_message_before_the_abort() {
        use std::os::unix::process::ExitStatusExt;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::double_panic_in_a_process_of_its_own",
                "--nocapture",
            ])
            .env("CMM2_TEST_DOUBLE_PANIC", "1")
            .output()
            .unwrap();

        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.signal(), Some(libc::SIGABRT), "{said}");
        let first = said.find("the first panic").expect(&said);
        let second = said.find("the second panic").expect(&said);
        assert!(first < second, "oldest first: {said}");
    }

    #[test]
    fn a_failure_of_the_run_comes_before_those_of_the_trace_and_the_save() {
        let err = |text: &str| RunError::Fault(text.to_string());
        let mut said = Vec::new();
        let last = settle(
            Err(err("fault")),
            [Err(err("trace")), Err(err("shot")), Err(err("save"))],
            &mut said,
        )
        .unwrap_err();
        // The last is returned for the caller to say; the others were said, in order, before it.
        assert_eq!(last.to_string(), "save");
        assert_eq!(
            String::from_utf8(said).unwrap(),
            "cmm2-emulator: fault\ncmm2-emulator: trace\ncmm2-emulator: shot\n"
        );

        // With no fault the order is the trace, the screenshot, the save.
        let mut said = Vec::new();
        let last = settle(
            Ok(true),
            [Err(err("trace")), Ok(()), Err(err("save"))],
            &mut said,
        )
        .unwrap_err();
        assert_eq!(last.to_string(), "save");
        assert_eq!(String::from_utf8(said).unwrap(), "cmm2-emulator: trace\n");

        // One error alone is only returned, and none says nothing.
        let mut said = Vec::new();
        let last = settle(Err(err("fault")), [Ok(()), Ok(()), Ok(())], &mut said).unwrap_err();
        assert_eq!(last.to_string(), "fault");
        let last = settle(Ok(true), [Ok(()), Ok(()), Err(err("save"))], &mut said).unwrap_err();
        assert_eq!(last.to_string(), "save");
        assert!(matches!(
            settle(Ok(false), [Ok(()), Ok(()), Ok(())], &mut said),
            Ok(false)
        ));
        assert!(said.is_empty());
    }

    #[test]
    fn a_name_that_cannot_be_a_file_is_refused_at_the_start() {
        for path in [
            "/tmp/x/flash/",
            "/tmp/x/.",
            "/tmp/x/..",
            ".",
            "..",
            "x/",
            "/",
        ] {
            let err = check_regular(path).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{path}");
        }
        // Names that only look like them.
        for path in ["a.", "..a", ".a", "x/.hidden", "/tmp/x/flash"] {
            assert!(check_regular(path).is_ok(), "{path}");
        }
        let flash = load_flash(&mut idle_cpu(), "/tmp/cmm2-no-such-dir/flash/").unwrap_err();
        assert_eq!(
            flash.to_string(),
            "/tmp/cmm2-no-such-dir/flash/: not a file name (it is empty, or ends in a slash, or in . or ..)"
        );
        let card = load_sd(&SdSlot::new(), "/tmp/cmm2-no-such-dir/card/").unwrap_err();
        assert_eq!(
            card.to_string(),
            "/tmp/cmm2-no-such-dir/card/: not a file name (it is empty, or ends in a slash, or in . or ..)"
        );
    }

    #[test]
    fn an_empty_name_and_a_link_to_a_name_that_is_not_one_are_refused_at_the_start() {
        let empty = check_regular("").unwrap_err();
        assert_eq!(empty.kind(), io::ErrorKind::InvalidInput);

        let dir = scratch("link-to-slash");
        for target in ["newfile/", "nothere/.", "nothere/.."] {
            symlink(target, dir.join("lnk")).unwrap();
            let link = dir.join("lnk");
            let path = link.to_str().unwrap();

            let flash = load_flash(&mut idle_cpu(), path).unwrap_err();
            assert_eq!(
                flash.to_string(),
                format!(
                    "{path}: it leads to {}/{target}, which cannot be the name of a file, so the run could not save its flash and clock",
                    dir.display()
                ),
                "{target}"
            );
            // A card has to be there to be read, so a link to nothing is as good as no file.
            assert!(load_sd(&SdSlot::new(), path).is_err(), "{target}");
            std::fs::remove_file(&link).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Makes a file's time of last change `secs` seconds ago, by its path and without opening it
    // (a FIFO would wait for a reader).
    fn age(path: &Path, secs: u64) {
        let then = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
        let since = then.duration_since(std::time::UNIX_EPOCH).unwrap();
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: since.as_secs() as libc::time_t,
                tv_nsec: 0,
            },
        ];
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: utimensat with a NUL-terminated name and two timespecs that live through the
        // call.
        let done = unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                name.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        assert_eq!(done, 0);
    }

    #[test]
    fn an_old_temporary_file_that_nobody_holds_is_removed_and_the_others_are_kept() {
        let dir = scratch("stale-temps");
        let held = dir.join(".cmm2.4242.9.tmp");
        let unheld = dir.join(".cmm2.4242.1.tmp");
        let fresh = dir.join(".cmm2.4242.2.tmp");
        // Names that only look like ours, or are not a process's at all.
        let others = [
            ".cmm2.x.tmp",
            ".cmm2.+999999.+1.tmp",
            ".cmm2.-9999999.1.tmp",
            ".cmm2.0.1.tmp",
            ".cmm2.12.tmp",
            ".cmm2.12.3.tmp.keep",
            "a.img.1.1.tmp",
        ];
        for file in [&held, &unheld, &fresh] {
            std::fs::write(file, b"x").unwrap();
        }
        for name in others {
            std::fs::write(dir.join(name), b"x").unwrap();
            age(&dir.join(name), 3600);
        }
        std::fs::write(dir.join("a.img"), b"card").unwrap();
        for file in [&held, &unheld] {
            age(file, 3600);
        }
        // A save that is going on: the file is locked as `create_locked` does, and old.
        let lock = File::open(&held).unwrap();
        assert!(try_lock(&lock));

        remove_stale_temps(dir.join("a.img").to_str().unwrap());

        assert!(!unheld.exists(), "old and nobody's");
        assert!(held.exists(), "held by a save that is going on");
        assert!(fresh.exists(), "too new to tell");
        for name in others {
            assert!(dir.join(name).exists(), "{name}");
        }
        // Once the holder lets go (or is gone), the next start removes it.
        drop(lock);
        remove_stale_temps(dir.join("a.img").to_str().unwrap());
        assert!(!held.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_is_not_a_regular_file_is_never_opened_by_the_cleanup() {
        let dir = scratch("stale-odd");
        // A FIFO would block an open for reading until someone writes, and a link to a device
        // would be opened as the device. Both are named like a temporary file and are old.
        let fifo = dir.join(".cmm2.4242.1.tmp");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo with a NUL-terminated name that lives through the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
        age(&fifo, 3600);
        let link = dir.join(".cmm2.4242.2.tmp");
        symlink("/dev/null", &link).unwrap();
        std::fs::write(dir.join("a.img"), b"card").unwrap();

        // This returns, and so does not wait on the FIFO.
        remove_stale_temps(dir.join("a.img").to_str().unwrap());

        assert!(fifo.exists());
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        // The opening itself refuses a link and a FIFO, if one is swapped in after the look.
        assert!(open_to_lock(&link).is_none());
        assert!(open_to_lock(&fifo).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_left_over_file_that_cannot_be_read_is_removed_through_a_write_open() {
        if root() {
            return;
        }
        let dir = scratch("stale-wo");
        let write_only = dir.join(".cmm2.4242.1.tmp");
        let nothing = dir.join(".cmm2.4242.2.tmp");
        std::fs::write(&write_only, b"x").unwrap();
        std::fs::write(&nothing, b"x").unwrap();
        age(&write_only, 3600);
        age(&nothing, 3600);
        mode(&write_only, 0o200);
        mode(&nothing, 0o000);
        std::fs::write(dir.join("a.img"), b"card").unwrap();

        remove_stale_temps(dir.join("a.img").to_str().unwrap());

        // Write only can be locked and so is removed; with no right at all it cannot be, and
        // is left (README has the command to remove it).
        assert!(!write_only.exists());
        assert!(nothing.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_memory_devices_that_take_any_write_are_written_in_big_chunks() {
        // null, zero, full, random, urandom.
        for minor in [3, 5, 7, 8, 9] {
            assert!(memory_device_never_blocks(1, minor), "1:{minor}");
        }
        // /dev/mem (1, 1), /dev/port (1, 4), /dev/kmsg (1, 11), and other majors.
        for (major, minor) in [(1, 1), (1, 4), (1, 11), (4, 3), (5, 0), (136, 3)] {
            assert!(!memory_device_never_blocks(major, minor), "{major}:{minor}");
        }
    }

    #[test]
    fn the_memory_devices_are_written_in_big_chunks_and_a_terminal_is_not_one() {
        use std::os::fd::AsRawFd;
        let null = File::create("/dev/null").unwrap();
        assert!(never_blocks(null.as_raw_fd()));
        let mut out = Output::of(null.as_raw_fd(), None);
        assert!(!out.own);
        assert_eq!(out.chunk, BIG_CHUNK);
        out.write_all(&vec![7; 3 * BIG_CHUNK + 5]).unwrap();
        let (reader, _writer) = std::io::pipe().unwrap();
        assert!(!never_blocks(reader.as_raw_fd()));
        let (master, slave) = raw_pty();
        assert!(!never_blocks(master));
        assert!(!never_blocks(slave));
        // SAFETY: closing the two descriptors made above.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }

    #[test]
    fn the_temporary_file_of_a_save_is_locked_while_it_is_open() {
        let dir = scratch("locked-temp");
        let temp = dir.join(".cmm2.4242.1.tmp");
        let file = create_locked(&temp).unwrap();

        // Nothing else can lock it, and it is the file that is made new, never one taken over.
        assert!(!try_lock(&File::open(&temp).unwrap()));
        assert!(create_locked(&temp).is_err());

        drop(file);
        assert!(try_lock(&File::open(&temp).unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_digits_make_a_temporary_files_name() {
        assert!(is_temp_name(".cmm2.12.345.tmp"));
        for name in [
            ".cmm2.12.tmp",
            ".cmm2.a.345.tmp",
            "cmm2.12.345.tmp",
            ".cmm2.12.345",
            ".cmm2.+12.345.tmp",
            ".cmm2.12.+345.tmp",
            ".cmm2.-12.345.tmp",
            ".cmm2.0.1.tmp",
            ".cmm2.00.1.tmp",
            ".cmm2..1.tmp",
            ".cmm2.1..tmp",
            ".cmm2.99999999999999999999.1.tmp",
        ] {
            assert!(!is_temp_name(name), "{name}");
        }
    }

    #[test]
    fn starting_with_a_flash_or_a_card_file_clears_the_temporary_files_nobody_holds() {
        let dir = scratch("stale-at-start");
        let stale = dir.join(".cmm2.4242.1.tmp");
        std::fs::write(&stale, b"x").unwrap();
        age(&stale, 3600);
        std::fs::write(dir.join("card.img"), vec![0; MIN_IMAGE_SIZE]).unwrap();

        load_flash(&mut idle_cpu(), dir.join("board.flash").to_str().unwrap()).unwrap();
        assert!(!stale.exists());

        std::fs::write(&stale, b"x").unwrap();
        age(&stale, 3600);
        load_sd(&SdSlot::new(), dir.join("card.img").to_str().unwrap()).unwrap();
        assert!(!stale.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_read_is_said_so_at_the_start_for_both_files() {
        if root() {
            return;
        }
        let dir = scratch("unreadable");
        let file = dir.join("f");
        std::fs::write(&file, vec![0; MIN_IMAGE_SIZE]).unwrap();
        mode(&file, 0o200);
        let path = file.to_str().unwrap();

        let flash = load_flash(&mut idle_cpu(), path).unwrap_err();
        let card = load_sd(&SdSlot::new(), path).unwrap_err();

        assert_eq!(
            flash.to_string(),
            format!("{path}: the file cannot be read")
        );
        assert_eq!(card.to_string(), format!("{path}: the file cannot be read"));
        mode(&file, 0o644);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_of_the_longest_length_is_saved_and_a_longer_one_is_refused_at_the_start() {
        let dir = scratch("long-name");
        // The temporary file is not named after the file, so a name as long as there can be
        // is saved; the temporary file's suffix would have made it too long for the system.
        let longest = dir.join("a".repeat(255));
        let options = RunOptions {
            flash: Some(longest.to_str().unwrap().to_string()),
            ..RunOptions::default()
        };
        let mut cpu = idle_cpu();
        load_flash(&mut cpu, longest.to_str().unwrap()).unwrap();
        save_flash(&mut cpu, &options).unwrap();
        assert_eq!(files_in(&dir), ["a".repeat(255)]);
        load_flash(&mut cpu, longest.to_str().unwrap()).unwrap();

        let too_long = dir.join("a".repeat(256));
        let err = load_flash(&mut cpu, too_long.to_str().unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{}: a name on its path is longer than the file system allows, so the run could not save its flash and clock",
                too_long.display()
            )
        );
        // A card that long is not there to be read, which is the OS's to say.
        assert!(load_sd(&SdSlot::new(), too_long.to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_card_file_with_the_longest_name_is_written_back() {
        let dir = scratch("long-card");
        let file = dir.join("c".repeat(255));
        std::fs::write(&file, vec![0x11; MIN_IMAGE_SIZE]).unwrap();
        let sd = SdSlot::new();
        load_sd(&sd, file.to_str().unwrap()).unwrap();
        assert!(!sd.write_protected());
        sd.insert(vec![0x22; MIN_IMAGE_SIZE]).unwrap();

        write_atomically(file.to_str().unwrap(), &sd.eject().unwrap()).unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), vec![0x22; MIN_IMAGE_SIZE]);
        assert_eq!(files_in(&dir), ["c".repeat(255)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn searching_a_directory_and_writing_to_it_are_said_apart() {
        if root() {
            return;
        }
        let dir = scratch("search-write");
        std::fs::create_dir(dir.join("d")).unwrap();
        let path = dir.join("d/flash");
        let path = path.to_str().unwrap();

        mode(&dir.join("d"), 0o666);
        assert_eq!(
            why_not_saveable(path).as_deref(),
            Some("a directory on its path cannot be searched")
        );
        mode(&dir.join("d"), 0o555);
        assert_eq!(
            why_not_saveable(path).as_deref(),
            Some("its directory is not writable")
        );
        // A directory above it that cannot be searched hides the whole path.
        mode(&dir.join("d"), 0o755);
        mode(&dir, 0o666);
        assert_eq!(
            why_not_saveable(path).as_deref(),
            Some("a directory on its path cannot be searched")
        );
        mode(&dir, 0o755);
        assert_eq!(why_not_saveable(path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_ordinary_directory_is_not_append_only() {
        // chattr +a on a directory needs root, so the refusal itself is not tried; what is
        // here is that a directory that is not flagged is not taken for one.
        let dir = scratch("dir-flags");
        assert!(!is_immutable(&File::open(&dir).unwrap()));
        assert_eq!(why_not_saveable(dir.join("flash").to_str().unwrap()), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_chunk_of_a_write_that_cannot_be_reopened_is_by_the_kind_of_file() {
        use std::os::fd::AsRawFd;
        let (reader, writer) = std::io::pipe().unwrap();
        assert_eq!(chunk_for(writer.as_raw_fd()), libc::PIPE_BUF);
        assert_eq!(chunk_for(reader.as_raw_fd()), libc::PIPE_BUF);
        let tun_like = std::os::unix::net::UnixStream::pair().unwrap().0;
        assert_eq!(chunk_for(tun_like.as_raw_fd()), 256);
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in two descriptors, which are closed after.
        unsafe {
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null()
                ),
                0
            );
            assert_eq!(chunk_for(slave), 1);
            libc::close(master);
            libc::close(slave);
        }
    }

    // A pseudo-terminal, the slave in raw mode so that what is written to the master comes out
    // of it as it went in.
    fn raw_pty() -> (libc::c_int, libc::c_int) {
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in two descriptors, and cfmakeraw and tcsetattr work on a
        // termios that was got from the slave.
        unsafe {
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null()
                ),
                0
            );
            let mut termios: libc::termios = std::mem::zeroed();
            libc::tcgetattr(slave, &mut termios);
            libc::cfmakeraw(&mut termios);
            libc::tcsetattr(slave, libc::TCSANOW, &termios);
        }
        (master, slave)
    }

    #[test]
    fn only_a_pipe_or_the_slave_of_a_terminal_is_reopened() {
        use std::os::fd::AsRawFd;
        let (reader, writer) = std::io::pipe().unwrap();
        assert!(can_be_reopened(writer.as_raw_fd()));
        assert!(can_be_reopened(reader.as_raw_fd()));
        let (master, slave) = raw_pty();
        assert!(can_be_reopened(slave));
        // Opening /dev/ptmx makes a new pair, so the master is written as it is.
        assert!(!can_be_reopened(master));
        // Nor is another device (opening some does something), a regular file or a socket.
        assert!(!can_be_reopened(
            File::create("/dev/null").unwrap().as_raw_fd()
        ));
        let dir = scratch("reopen-kinds");
        assert!(!can_be_reopened(
            File::create(dir.join("f")).unwrap().as_raw_fd()
        ));
        let (socket, _other) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(!can_be_reopened(socket.as_raw_fd()));
        // SAFETY: closing the two descriptors made above.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_is_written_to_the_master_of_a_terminal_arrives_on_its_slave() {
        use std::io::Read;
        use std::os::fd::FromRawFd;
        let (master, slave) = raw_pty();
        let mut out = Output::of(master, None);
        assert!(!out.own);
        let reader = std::thread::spawn(move || {
            // SAFETY: the descriptor is the slave made above, and is this thread's from here.
            let mut slave = unsafe { File::from_raw_fd(slave) };
            let mut got = Vec::new();
            let mut buffer = [0; 4096];
            while got.len() < 20_000 {
                // A wait of some seconds, so that a failure is an assertion and not a hang.
                let mut waiting = libc::pollfd {
                    fd: std::os::fd::AsRawFd::as_raw_fd(&slave),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: poll on one live pollfd.
                if unsafe { libc::poll(&mut waiting, 1, 5000) } != 1 {
                    break;
                }
                match slave.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => got.extend_from_slice(&buffer[..count]),
                }
            }
            got
        });

        let sent: Vec<u8> = (0..20_000).map(|n| b'a' + (n % 26) as u8).collect();
        out.write_all(&sent).unwrap();

        assert_eq!(reader.join().unwrap(), sent);
        // SAFETY: closing the master made above.
        unsafe { libc::close(master) };
    }

    #[test]
    fn what_can_be_reopened_is_written_through_a_descriptor_of_its_own_and_in_big_chunks() {
        use std::os::fd::AsRawFd;
        let flags = |fd: libc::c_int| {
            // SAFETY: fcntl with F_GETFL only reads the flags.
            unsafe { libc::fcntl(fd, libc::F_GETFL) }
        };
        // A pipe: the writer is non-blocking, and the original is not made so.
        let (_reader, writer) = std::io::pipe().unwrap();
        let out = Output::of(writer.as_raw_fd(), None);
        assert!(out.own);
        assert_eq!(out.chunk, BIG_CHUNK);
        assert_ne!(out.fd, writer.as_raw_fd());
        assert_ne!(flags(out.fd) & libc::O_NONBLOCK, 0);
        assert_eq!(flags(writer.as_raw_fd()) & libc::O_NONBLOCK, 0);

        // A terminal as well, which is the case that was a byte at a time.
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty fills in two descriptors, which are closed after.
        unsafe {
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null()
                ),
                0
            );
            let out = Output::of(slave, None);
            assert!(out.own);
            assert_eq!(out.chunk, BIG_CHUNK);
            assert_eq!(flags(slave) & libc::O_NONBLOCK, 0);
            drop(out);
            libc::close(master);
            libc::close(slave);
        }

        // A regular file needs none of it.
        let dir = scratch("output-of");
        let file = File::create(dir.join("f")).unwrap();
        let out = Output::of(file.as_raw_fd(), None);
        assert!(!out.own);
        assert_eq!(out.fd, file.as_raw_fd());
        assert_eq!(out.chunk, BIG_CHUNK);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_trace_file_is_written_as_it_is_with_the_output_writer() {
        let dir = scratch("output-file");
        let mut out = Output::file(File::create(dir.join("t")).unwrap());
        out.write_all(&vec![b'x'; 200_000]).unwrap();
        assert_eq!(std::fs::read(dir.join("t")).unwrap().len(), 200_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
