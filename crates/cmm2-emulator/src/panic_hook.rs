//! What a panic does while a run is going: the terminal is put back before its message, and the
//! message is held until the files are saved, as the default hook would wait on a standard error
//! that nobody reads.

use crate::terminal;
use std::io;

use std::io::Write;

/// The messages of panics that are held back while a run is going, oldest first. A run that
/// panics is saved after the message would have been written, so the message waits for the save.
/// Only so many are kept, the first ones, as the first panic is the one that matters.
static HELD_PANICS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
const HELD_PANICS_MAX: usize = 8;

/// A run is going, and what it does in `run_and_save` is saved if it panics.
pub(crate) static HOLDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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
    pub(crate) static PANIC_SEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
pub(crate) fn install_panic_hook() {
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
pub(crate) fn say_late_panic(out: &mut impl Write) {
    let late = std::mem::take(
        &mut *HELD_PANICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for text in late {
        let _ = out.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests;
