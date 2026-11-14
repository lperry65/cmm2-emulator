use super::*;
use crate::args::RunOptions;
use crate::atomic_write::write_atomically;
use crate::error::RunError;
use crate::saveable::{check_regular, why_not_saveable};
use crate::test_support::{age, files_in, idle_cpu, mode, root, scratch};
use cmm2_core::PERSISTENT_SIZE;
use cmm2_core::{Irqs, MIN_IMAGE_SIZE, SdSlot, cmm2_memory};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

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
