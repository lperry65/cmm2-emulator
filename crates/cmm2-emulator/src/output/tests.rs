use super::*;

use crate::test_support::scratch;
use std::fs::File;
use std::io::Write;

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
