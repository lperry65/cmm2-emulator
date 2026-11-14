//! The writer for standard output and error and for files that a signal can end, and the errors
//! that tell that a signal did.

use crate::signals;
use std::fmt;
use std::fs::File;
use std::io;
use std::io::Write;

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

pub(crate) fn is_signal_end(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<SignalEnd>())
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
pub(crate) struct Output {
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
    pub(crate) fn stdout() -> Output {
        Output::of(1, None)
    }

    pub(crate) fn stderr() -> Output {
        Output::of(2, None)
    }

    pub(crate) fn file(file: File) -> Output {
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

#[cfg(test)]
mod tests;
