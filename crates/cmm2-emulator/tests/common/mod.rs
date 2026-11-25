//! What the integration tests share: running the emulator, the temporary files, and the guest
//! programs they run, hand-assembled.

// Each test file uses some of these and not others.
#![allow(dead_code)]

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// The emulator, with a board directory that can never be made (a path under /dev/null), so that
// a test that reaches for the board and has not given itself one fails and does not touch the
// owner's own board. A test of the board sets its own with `.env(HOME_VARIABLE, ...)`. This is the
// only place that names the binary (tests/guard.rs sees to that), so every test goes through it.
pub const HOME_VARIABLE: &str = "CMM2_EMULATOR_HOME";
// The programs these tests run are not a firmware I know, so they have no settings layout of
// their own. This names one for an image that has none, as a user could. A test of the refusal
// removes it.
pub const LAYOUT_VARIABLE: &str = "CMM2_SETTINGS_LAYOUT";
// The same for the board, which the other variable never reaches: for the tests that burn one.
pub const BOARD_LAYOUT_VARIABLE: &str = "CMM2_BOARD_LAYOUT_FOR_TESTS";
pub const NO_BOARD: &str = "/dev/null/no-board";
const EMULATOR: &str = env!("CARGO_BIN_EXE_cmm2-emulator");

pub fn emulator() -> Command {
    let mut command = Command::new(EMULATOR);
    // The other places the board could be looked for are as impossible, so that a test that
    // removes `CMM2_EMULATOR_HOME` (or sets it to nothing, which counts as unset in a shell) still
    // cannot reach the owner's board.
    command
        .env(HOME_VARIABLE, NO_BOARD)
        .env("XDG_DATA_HOME", NO_BOARD)
        .env("HOME", NO_BOARD)
        .env("USERPROFILE", NO_BOARD)
        .env(LAYOUT_VARIABLE, "b5")
        .env(BOARD_LAYOUT_VARIABLE, "b5");
    command
}

// A child process that is killed, by its handle, and waited for when this is dropped, so that a
// test that fails with the emulator still running (or never gets as far as stopping it) does not
// leave it going: a live run does not end when its standard input does. It stands for the
// `Child` (it derefs to it); `release` gives the child back for a test that waits for it itself.
pub struct Guard(Option<Child>);

impl Guard {
    pub fn new(child: Child) -> Guard {
        Guard(Some(child))
    }

    pub fn release(mut self) -> Child {
        self.0.take().unwrap()
    }
}

impl std::ops::Deref for Guard {
    type Target = Child;
    fn deref(&self) -> &Child {
        self.0.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// `spawn`, into a `Guard`.
pub trait SpawnGuarded {
    fn spawn_guarded(&mut self) -> Guard;
}

impl SpawnGuarded for Command {
    fn spawn_guarded(&mut self) -> Guard {
        Guard::new(self.spawn().expect("failed to run cmm2-emulator"))
    }
}

// Paths that are removed when this is dropped, a failed test too: a directory with what is in it
// (made writable again first, as a test may have left it read-only), a file as it is.
pub struct Cleanup(pub Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            if path.is_dir() {
                let _ = std::fs::set_permissions(
                    path,
                    std::os::unix::fs::PermissionsExt::from_mode(0o755),
                );
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

// The path of the emulator binary, for a command that has to be built round it (one that runs it
// in a mount namespace, say). The command that is built has to set the board variables itself: the
// environment of `emulator()` does not come with a path. This is the only place that takes the
// program out of a command.
pub fn emulator_path() -> String {
    emulator().get_program().to_str().unwrap().to_string()
}

pub fn run(args: &[&str]) -> Output {
    emulator()
        .args(args)
        // The window tests have no display to open on.
        .env("SDL_VIDEODRIVER", "dummy")
        .output()
        .expect("failed to run cmm2-emulator")
}

pub mod board;

#[path = "../../src/run_id.rs"]
mod run_id;

pub fn temp_path(name: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cmm2-cli-{}-{name}.{extension}", run_id::run_id()))
}

pub const CODE: u32 = 0x0800_0100;

// Writes a firmware image whose reset handler is `code` at CODE, as Thumb halfwords.
pub fn temp_program(name: &str, code: &[u16]) -> PathBuf {
    let path = temp_path(name, "bin");
    let mut bytes = vec![0; 0x100];
    bytes[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&(CODE | 1).to_le_bytes());
    bytes.extend(code.iter().flat_map(|half| half.to_le_bytes()));
    std::fs::write(&path, bytes).unwrap();
    path
}

// movs r0,#1; movs r1,#2; movs r2,#3; b .
pub const THREE: [u16; 4] = [0x2001, 0x2102, 0x2203, 0xE7FE];

// Sets UE, TE and RE in USART1_CR1, sends "ready\r\n", then echoes each byte it receives plus
// one. The address of USART1 is the literal at +52.
pub const ECHO: [u16; 28] = [
    0x480C, 0x210D, 0x6001, 0x2272, 0x6282, 0x2265, 0x6282, 0x2261, 0x6282, 0x2264, 0x6282, 0x2279,
    0x6282, 0x220D, 0x6282, 0x220A, 0x6282, 0x69C2, 0x2320, 0x421A, 0xD0FB, 0x6A42, 0x3201, 0x6282,
    0xE7F7, 0x0000, 0x1000, 0x4001,
];

pub const UNDEFINED: [u16; 1] = [0xDE00]; // udf #0

// Counts its boots in AXI SRAM, sends a letter for each ('A' for the first) and asks for a
// system reset until the third boot, which spins. The literals are at +32: the counter's
// address, USART1, AIRCR and the reset request.
pub const REBOOT: [u16; 24] = [
    0x4807, 0x6801, 0x3101, 0x6001, 0x4A06, 0x2309, 0x6013, 0x2440, 0x1864, 0x6294, 0x2903, 0xDA02,
    0x4A03, 0x4B04, 0x6013, 0xE7FE, 0x0100, 0x2400, 0x1000, 0x4001, 0xED0C, 0xE000, 0x0004, 0x05FA,
];

pub const RESET_LINE: &str = "cmm2-emulator: system reset\n";

// Runs the program with `args` after `--firmware <image>`, and removes the image.
pub fn run_program(name: &str, code: &[u16], args: &[&str]) -> Output {
    let path = temp_program(name, code);
    let mut all = vec!["--firmware", path.to_str().unwrap()];
    all.extend_from_slice(args);
    let output = run(&all);
    std::fs::remove_file(&path).unwrap();
    output
}

// The path of the 7.00.00RC6 image: a second firmware I know, for the tests that need one
// (outside the repo, never committed, like CMM2_FIRMWARE).
pub fn rc6_firmware() -> String {
    std::env::var("CMM2_FIRMWARE_RC6")
        .expect("CMM2_FIRMWARE_RC6 must be set to the path of CMM2_MMBasic_7.00.00RC6.bin")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// Brings up a 1 x 1 screen: the LTDC enabled, active area 1 x 1, layer 1 on it in RGB565 at the
// start of AXI SRAM, fetching 1 line, reloaded, and one red pixel stored there. The addresses
// are literals at +56.
pub const SCREEN: [u16; 46] = [
    0x480D, 0x490E, 0x4A0E, 0x6182, 0x4A0E, 0x6102, 0x4A0C, 0x604A, 0x4A0C, 0x608A, 0x4A0B, 0x60CA,
    0x4A0B, 0x614A, 0x4A0B, 0x618A, 0x4A0B, 0x62CA, 0x4A0B, 0x630A, 0x4A05, 0x634A, 0x4A04, 0x6242,
    0x4A09, 0x4B07, 0x801A, 0xE7FE, 0x1000, 0x5000, 0x1080, 0x5000, 0x0001, 0x0000, 0x0001, 0x0001,
    0x0002, 0x0000, 0x00FF, 0x0000, 0x0000, 0x2400, 0x0009, 0x0002, 0xF800, 0x0000,
];

// The flash file is b5's 32-byte block, then the last 0xE0000 bytes of the flash, the saved options
// in its last sector, then the clock's time and date, a word each. The DS3231's state comes after
// them once the guest has set its time, which these programs do not.
pub const FLASH_FILE_SIZE: usize = 0x20 + 0xE_0000 + 8;

pub const OPTIONS_AT: usize = 0x20 + 0xC_0000;

// Adds one to the word at 0x081E_0000, the start of the saved options (the literal is at +14),
// then ends as `end`. An erased word is -1, so the first run leaves 0.
pub fn count_in_flash(end: u16) -> [u16; 8] {
    [0x4802, 0x6801, 0x3101, 0x6001, end, 0x0000, 0x0000, 0x081E]
}

pub const SPIN: u16 = 0xE7FE; // b .

pub fn options_word(flash: &[u8]) -> u32 {
    u32::from_le_bytes(flash[OPTIONS_AT..OPTIONS_AT + 4].try_into().unwrap())
}

pub fn flash_path(name: &str) -> PathBuf {
    temp_path(name, "flash")
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// Writes 0x5A to every byte of block 1 of the SD card, which it drives through the GPIO pins
// the way the firmware does (CMD0, CMD55, ACMD41 and CMD24 over port C and port D), then prints
// "done\n" on USART1 and ends as `end`. It was assembled with GNU as for a Cortex-M7 (the
// source is not kept: it is a bit-banged SPI byte routine and the command sequence).
pub const WRITE_CARD: [u16; 114] = [
    0xF640, 0x0000, 0xF6C5, 0x0002, 0xF640, 0x4100, 0xF6C5, 0x0102, 0xF64F, 0x72FF, 0xF6CF, 0x521F,
    0x6002, 0xF64F, 0x72DF, 0xF6CF, 0x72FF, 0x600A, 0x2201, 0x0492, 0x618A, 0x2640, 0x2700, 0xF000,
    0xF82C, 0x2677, 0x2700, 0xF000, 0xF828, 0x2669, 0x2740, 0x063F, 0xF000, 0xF823, 0x2658, 0x2701,
    0xF000, 0xF81F, 0x22FE, 0xF000, 0xF837, 0xF240, 0x2802, 0x225A, 0xF000, 0xF832, 0xF1B8, 0x0801,
    0xD1F9, 0x22FF, 0xF000, 0xF82C, 0xF241, 0x0000, 0xF2C4, 0x0001, 0x2109, 0x6001, 0x2164, 0x6281,
    0x216F, 0x6281, 0x216E, 0x6281, 0x2165, 0x6281, 0x210A, 0x6281, 0xE7FE, 0xB500, 0x4632, 0xF000,
    0xF817, 0x0E3A, 0xF000, 0xF814, 0x0C3A, 0xF000, 0xF811, 0x0A3A, 0xF000, 0xF80E, 0x463A, 0xF000,
    0xF80B, 0x2201, 0xF000, 0xF808, 0x22FF, 0xF000, 0xF805, 0xF000, 0xF803, 0xF000, 0xF801, 0xBD00,
    0x2308, 0x0614, 0xF241, 0x0500, 0xBF58, 0xF04F, 0x5580, 0x6185, 0xF44F, 0x6580, 0x6185, 0xF04F,
    0x6580, 0x6185, 0x0052, 0x3B01, 0xD1EF, 0x4770,
];

// Where in WRITE_CARD the end of the program is, and its address.
pub const WRITE_CARD_END: usize = 68;

pub fn write_card(end: u16) -> [u16; 114] {
    let mut code = WRITE_CARD;
    code[WRITE_CARD_END] = end;
    code
}

// The smallest card there is, 512 KB: its first four blocks are filled with their own number
// (1 to 4) and the rest with zeros.
pub fn card_image() -> Vec<u8> {
    let mut image: Vec<u8> = (1..=4u8).flat_map(|b| [b; 512]).collect();
    image.resize(512 * 1024, 0);
    image
}

// A card image as `card_image` makes it, and its inode and time of
// writing. A card the guest did not write to is not written back, so the file is the same one;
// one that it did is a new file, as it is written through a temporary file and a rename.
pub fn sd_card(name: &str) -> (PathBuf, Vec<u8>) {
    let path = temp_path(name, "img");
    let image = card_image();
    std::fs::write(&path, &image).unwrap();
    (path, image)
}

pub fn inode(path: &PathBuf) -> u64 {
    std::fs::metadata(path).unwrap().ino()
}

// The card image as the guest of WRITE_CARD leaves it.
pub fn written(image: &[u8]) -> Vec<u8> {
    let mut image = image.to_vec();
    image[512..1024].fill(0x5A);
    image
}

pub fn written_back(path: &PathBuf, image: &[u8], inode_before: u64) -> bool {
    inode(path) != inode_before && std::fs::read(path).unwrap() == written(image)
}

// Starts a live run of WRITE_CARD with the flash file and the card, and returns it once it has
// printed "done\n", so that the guest is running and has written the card.
pub fn start_writing_run(name: &str, flash: &PathBuf, card: &PathBuf) -> Guard {
    let path = temp_program(name, &write_card(SPIN));
    let mut child = emulator()
        .args(["--firmware", path.to_str().unwrap(), "--console", "--flash"])
        .arg(flash)
        .arg("--sd")
        .arg(card)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn_guarded();
    let mut out = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut byte = [0];
        while out.read(&mut byte).is_ok_and(|n| n == 1) {
            buffer.push(byte[0]);
            if buffer.ends_with(b"done\n") {
                let _ = sender.send(());
                break;
            }
        }
        // Keep draining, so the emulator is never blocked on a full pipe.
        let _ = std::io::copy(&mut out, &mut std::io::sink());
    });
    if receiver.recv_timeout(Duration::from_secs(30)).is_err() {
        panic!("the guest never said it was done");
    }
    std::fs::remove_file(&path).unwrap();
    child
}

pub fn wait_for(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the signal did not end the run");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn mode(path: &PathBuf, mode: u32) {
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
}

// As root a mode bit stops nothing, so the tests that depend on one are not run.
pub fn root() -> bool {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

// Runs the emulator with `args` in a user and mount namespace in which `dir` is a read-only
// mount (the file system is read-only to it, whatever the modes). None, with a note, when the
// machine does not let a user make one.
pub fn run_with_read_only_mount(dir: &Path, firmware: &Path, args: &[&str]) -> Option<Output> {
    let script =
        "mount --bind \"$1\" \"$1\" && mount -o remount,bind,ro \"$1\" && shift && exec \"$@\"";
    let output = Command::new("unshare")
        .args(["-rm", "sh", "-c", script, "sh"])
        .arg(dir)
        .arg(EMULATOR)
        .args(["--firmware", firmware.to_str().unwrap()])
        .args(args)
        .env(HOME_VARIABLE, NO_BOARD)
        .env("XDG_DATA_HOME", NO_BOARD)
        .env("HOME", NO_BOARD)
        .env("USERPROFILE", NO_BOARD)
        .env(LAYOUT_VARIABLE, "b5")
        .env(BOARD_LAYOUT_VARIABLE, "b5")
        .env("SDL_VIDEODRIVER", "dummy")
        .output();
    match output {
        // unshare, or the mount in it, was refused: sh says which on stderr.
        Ok(output) if !output.status.success() && stderr(&output).contains("unshare:") => {
            eprintln!(
                "skipped: no user and mount namespaces here: {}",
                stderr(&output)
            );
            None
        }
        Ok(output) if output.status.code() == Some(32) => {
            eprintln!(
                "skipped: no mounts in a user namespace here: {}",
                stderr(&output)
            );
            None
        }
        Ok(output) => Some(output),
        Err(err) => {
            eprintln!("skipped: no unshare here: {err}");
            None
        }
    }
}

pub fn make_fifo(path: &Path) {
    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo with a NUL-terminated name that lives through the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
}
