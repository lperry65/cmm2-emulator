use super::*;
use crate::args::RunOptions;
use crate::atomic_write::write_atomically;
use crate::atomic_write::{create_new_atomically, replace_atomically};
use crate::error::RunError;
use crate::saveable::{check_regular, why_not_saveable};
use crate::test_support::{age, files_in, idle_cpu, in_own_process, mode, root, scratch};
use cmm2_core::{B5, Firmware, Irqs, MIN_IMAGE_SIZE, RC6, STATE_SIZE, SdSlot, cmm2_memory};
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
    assert_eq!(std::fs::read(&file).unwrap().len(), B5.persistent_size());
    assert_eq!(B5.persistent_size(), 917_544);
}

#[test]
fn a_flash_file_with_a_ds3231_offset_that_is_too_far_away_is_refused() {
    let dir = scratch("flash-offset-far");
    let file = dir.join("board.flash");
    let mut cpu = idle_cpu();
    let mut bytes = cpu.persistent().unwrap();
    bytes.extend(i64::MAX.to_le_bytes());
    bytes.extend([1, 0x88]);
    std::fs::write(&file, bytes).unwrap();

    let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

    assert!(err.to_string().ends_with("more than 200 years"), "{err}");
}

// The board's steady save, called directly (the other storage tests seed through `load_flash`).
// The file must be the v2 form, and a link at the board's own name must be replaced and not
// written through, as docs/usage.md says.
#[test]
fn a_boards_steady_save_writes_v2_and_replaces_its_own_name() {
    let dir = scratch("save-v2");
    let file = dir.join("settings.flash");
    let path = file.to_str().unwrap();
    let options = RunOptions {
        flash: Some(path.to_string()),
        ..RunOptions::default()
    };
    let mut cpu = idle_cpu();
    // One row of the user flash changed first (the block is the file's first 32 bytes, so the
    // row at 0x0812_0000 follows it): a save with nothing to write would pass with a table the
    // header never counted.
    let mut seed = cpu.persistent().unwrap();
    seed[0x20..0x24].copy_from_slice(&0xCA_FE_F0_0Du32.to_le_bytes());
    cpu.load_persistent(&seed).unwrap();
    cpu.use_settings_v2();

    save_flash(&mut cpu, &options).unwrap();

    let saved = std::fs::read(&file).unwrap();
    assert!(saved.starts_with(&MAGIC), "the board's file is v2: {path}");
    let decoded = decode_v2(&saved, cpu.image(), &B5).expect("the board's own file reads back");
    let mut row = vec![0xFFu8; 32];
    row[..4].copy_from_slice(&0xCA_FE_F0_0Du32.to_le_bytes());
    assert_eq!(
        decoded.entries,
        vec![(0x0812_0000, row)],
        "the save carries the one changed row, and nothing else"
    );
    // `entry_count`, at the offset the format gives it (docs/settings-format.md): the header's
    // claim and the table the bytes hold are the same number.
    assert_eq!(
        u32::from_le_bytes(saved[44..48].try_into().expect("the entry count")),
        decoded.entries.len() as u32,
        "the header counts the rows the file holds"
    );
    assert!(
        saved.len() < 1024,
        "the save is the rows that changed, not the whole flash: {} bytes",
        saved.len()
    );

    // The same name, now held by a link out of the board directory: the save replaces the name
    // and leaves what it pointed at alone, as the migration's backup does.
    let outside = scratch("save-v2-outside");
    let victim = outside.join("SavedFlash.bin");
    std::fs::write(&victim, b"somebody else's file").unwrap();
    std::fs::remove_file(&file).unwrap();
    symlink(&victim, &file).unwrap();

    save_flash(&mut cpu, &options).unwrap();

    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"somebody else's file",
        "nothing was written through the link"
    );
    let held = std::fs::symlink_metadata(&file).unwrap();
    assert!(
        held.file_type().is_file(),
        "the link is gone: the board's name holds its own file"
    );
    assert!(
        std::fs::read(&file).unwrap().starts_with(&MAGIC),
        "and the file it holds is the board's v2 file"
    );
}

#[test]
fn a_read_only_flash_file_is_saved_all_the_same_as_the_save_replaces_the_file() {
    let dir = scratch("flash-readonly");
    let file = dir.join("board.flash");
    let mut cpu = idle_cpu();
    std::fs::write(&file, cpu.persistent().unwrap()).unwrap();
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
}

#[test]
fn a_flash_file_in_a_directory_that_cannot_be_written_is_refused_at_the_start() {
    if root() {
        return;
    }
    let dir = scratch("flash-dir-readonly");
    let file = dir.join("board.flash");
    let mut cpu = idle_cpu();
    std::fs::write(&file, cpu.persistent().unwrap()).unwrap();
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
}

#[test]
fn what_is_not_a_regular_file_is_refused_for_the_flash_and_the_card() {
    let dir = scratch("not-regular");
    let fifo = dir.join("pipe");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: mkfifo with a NUL-terminated name that lives through the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

    // None of them is opened, or this would never return for the FIFO.
    for path in [fifo.as_path(), &*dir, Path::new("/dev/null")] {
        let path = path.to_str().unwrap();
        let flash = load_flash(&mut idle_cpu(), path).unwrap_err();
        assert_eq!(flash.to_string(), format!("{path}: not a regular file"));
        let card = load_sd(&SdSlot::new(), path).unwrap_err();
        assert_eq!(card.to_string(), format!("{path}: not a regular file"));
    }
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
}

// A machine whose image is not one I know.
fn unknown_cpu() -> Cpu {
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
fn a_flash_file_with_a_firmware_that_has_no_layout_is_refused_at_the_start() {
    let dir = scratch("flash-no-layout");
    let new = dir.join("new.flash");
    let old = dir.join("old.flash");
    std::fs::write(&old, vec![0xFF; 917_522]).unwrap();

    // The first run, with no file yet, is refused as well: nothing could be saved at the end.
    for file in [&new, &old] {
        let path = file.to_str().unwrap();
        let err = load_flash(&mut unknown_cpu(), path).unwrap_err();
        assert!(matches!(err, RunError::NoLayout { .. }));
        assert_eq!(
            err.to_string(),
            format!(
                "{path}: this firmware has no settings layout yet, so it cannot save or load \
                 settings (see issue 82)"
            )
        );
    }
    assert!(!new.exists());
}

// A board of a firmware I know (v2 mode) that finds a raw file of the other release's size:
// the raw reader's message, and no word of "corrupt" (docs/settings-format.md, reading rule 7).
#[test]
fn a_v2_mode_run_says_the_other_release_message_before_it_says_corrupt() {
    let dir = scratch("storage-other-release");
    let file = dir.join("settings.flash");
    let size = RC6.persistent_size();
    std::fs::write(&file, vec![0xFFu8; size]).unwrap();
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();

    let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

    let said = err.to_string();
    assert!(
        said.contains(
            "that is the size of a file for firmware 7.00.00RC6, so it probably belongs to \
             another release"
        ),
        "the shipped wording is kept: {said}"
    );
    assert!(!said.contains("corrupt"), "{said}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        vec![0xFFu8; size],
        "the refusal wrote nothing"
    );
}

// The largest file either format can be is a fact about the format, not about the file, so it
// is a bound the read can be checked against before it happens (docs/settings-format.md, "The
// largest file either format can be"). The message is the same in every mode. The number is
// written out, as the doc has it, so that a bound that moved would not move the tests with it.
const LARGEST: usize = 1_146_990;

fn assert_refused_on_size(err: RunError, path: &str, size: u64) {
    let said = err.to_string();
    assert!(
        said.starts_with(&format!("{path}: the settings file is corrupt (")),
        "{said}"
    );
    assert!(said.contains(&format!("it is {size} bytes")), "{said}");
    assert!(
        said.contains(&format!(
            "more than the {LARGEST} bytes any settings file of the firmwares I know can be"
        )),
        "{said}"
    );
    assert!(said.ends_with("); nothing was loaded."), "{said}");
}

// One byte over the largest file is over, in a scratch run and on the board alike.
#[test]
fn a_file_one_byte_over_the_largest_is_refused_on_its_size() {
    let dir = scratch("flash-one-over");
    let file = dir.join("settings.flash");
    let size = LARGEST as u64 + 1;
    std::fs::write(&file, vec![0u8; size as usize]).unwrap();
    let path = file.to_str().unwrap();
    let mut raw = idle_cpu();
    let mut board = idle_cpu();
    board.use_settings_v2();

    for cpu in [&mut raw, &mut board] {
        assert_refused_on_size(load_flash(cpu, path).unwrap_err(), path, size);
    }
}

// A file far larger than any is refused on its size without being read. Proved in a process of its
// own whose address space is limited to a little more than it has now: reading this file whole
// needs a gigabyte, so a read fails (`fs::read` reports it as out of memory) and the message would
// be that error and not the size. The file is sparse, so it costs the disk nothing. (A file that
// cannot be read is refused before the size is looked at, so a mode-000 file or a FIFO cannot
// make this proof.)
#[test]
fn a_file_bigger_than_any_settings_file_is_refused_before_it_is_read() {
    const TOO_BIG: u64 = 1 << 30;
    let dir = scratch("flash-too-big");
    let file = dir.join("settings.flash");
    std::fs::File::create(&file)
        .unwrap()
        .set_len(TOO_BIG)
        .unwrap();
    let path = file.to_str().unwrap().to_string();

    let ran = in_own_process(
        "storage::tests::a_file_bigger_than_any_settings_file_is_refused_before_it_is_read",
        "",
        &[],
        |_| {
            // The address space the process has now, plus 256 MiB: not room for the file.
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            let used_kib: u64 = status
                .lines()
                .find_map(|line| line.strip_prefix("VmSize:"))
                .map(|rest| rest.trim().trim_end_matches("kB").trim().parse().unwrap())
                .expect("VmSize is in /proc/self/status");
            let limit = (used_kib << 10) + (256 << 20);
            let limits = libc::rlimit {
                rlim_cur: limit,
                rlim_max: limit,
            };
            // SAFETY: setrlimit is given a pointer to a struct that lives through the call.
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &limits) }, 0);
            let mut raw = idle_cpu();
            let mut board = idle_cpu();
            board.use_settings_v2();
            for cpu in [&mut raw, &mut board] {
                assert_refused_on_size(load_flash(cpu, &path).unwrap_err(), &path, TOO_BIG);
            }
            // The child ends with `_exit`, so this test's own directory in it is removed here.
            std::fs::remove_dir_all(&dir).unwrap();
        },
    );

    if let Some(ran) = ran {
        assert_eq!(
            ran.ended,
            crate::test_support::Ended::Exit(0),
            "{}",
            ran.stderr
        );
    }
}

// The bound is the format's own, so a file of exactly the largest size is not refused by it:
// what it is refused for is what it actually gets wrong, which here is a tail of nothing.
#[test]
fn a_file_of_exactly_the_largest_size_is_not_refused_on_its_size() {
    let dir = scratch("flash-largest");
    let file = dir.join("settings.flash");
    let largest = LARGEST;
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();
    let mut bytes = cpu.save_settings().unwrap();
    // Its header names this run's image, so the gate is passed; what the file gets wrong is
    // that it is nothing but tail, right up to the last byte the format can be.
    bytes.resize(largest, 0);
    assert_eq!(bytes.len(), largest);
    std::fs::write(&file, &bytes).unwrap();
    let path = file.to_str().unwrap();

    let err = load_flash(&mut cpu, path).unwrap_err();

    let said = err.to_string();
    assert!(said.contains("the tail is "), "what it gets wrong: {said}");
    assert!(
        !said.contains("more than the "),
        "the size bound refused a file that size allows: {said}"
    );
}

// The backup of the file a migration replaces sits beside the live one as a plain file of its
// own. A name there that a link or a directory holds is refused: following it would write the
// user's saved program outside the board directory (docs/settings-format.md, "Migrating a v1
// file").
#[test]
fn a_migration_refuses_a_backup_name_that_is_not_a_regular_file() {
    #[derive(Debug, Copy, Clone)]
    enum Shape {
        LinkToFile,
        DanglingLink,
        Directory,
    }
    let outside = scratch("backup-outside");
    let victim = outside.join("SavedFlash.bin");
    std::fs::write(&victim, b"somebody else's file").unwrap();
    let nowhere = outside.join("nowhere");
    let kind = |path: &Path| {
        let file_type = std::fs::symlink_metadata(path).unwrap().file_type();
        if file_type.is_symlink() {
            "link"
        } else if file_type.is_dir() {
            "directory"
        } else {
            "a file"
        }
    };

    for shape in [Shape::LinkToFile, Shape::DanglingLink, Shape::Directory] {
        let dir = scratch("backup-not-a-file");
        let file = dir.join("settings.flash");
        let backup = dir.join("settings.flash.v1");
        let mut cpu = idle_cpu();
        let original = cpu.persistent().unwrap();
        std::fs::write(&file, &original).unwrap();
        match shape {
            Shape::LinkToFile => symlink(&victim, &backup).unwrap(),
            Shape::DanglingLink => symlink(&nowhere, &backup).unwrap(),
            Shape::Directory => std::fs::create_dir(&backup).unwrap(),
        }
        cpu.use_settings_v2();

        let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

        assert_eq!(
            err.to_string(),
            format!(
                "{}: not backing up the old settings to {}: that name is not a regular file. \
                 Move it out of the way and start again; no file was written.",
                file.display(),
                backup.display()
            )
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            original,
            "{shape:?}: the live file is the v1 file it was"
        );
        assert_eq!(
            kind(&backup),
            match shape {
                Shape::Directory => "directory",
                _ => "link",
            },
            "{shape:?}: what held the name still holds it"
        );
        assert!(
            !nowhere.exists(),
            "{shape:?}: nothing was written through a dangling link"
        );
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"somebody else's file",
            "{shape:?}: nothing was written through the link"
        );
    }
}

// The raw bytes of the file a migration replaces are never lost: a free `.v1` takes them, and
// when a plain file is already there (an older migration's copy) the new bytes go to the first
// free `.v1.2`, `.v1.3` and so on. The older copies are left alone.
#[test]
fn a_migration_keeps_the_old_bytes_beside_the_live_file() {
    for (taken, want) in [
        (vec![], "settings.flash.v1"),
        (vec!["settings.flash.v1"], "settings.flash.v1.2"),
        (
            vec!["settings.flash.v1", "settings.flash.v1.2"],
            "settings.flash.v1.3",
        ),
        // A gap is filled: the first free name, not the one after the last.
        (
            vec!["settings.flash.v1", "settings.flash.v1.3"],
            "settings.flash.v1.2",
        ),
    ] {
        let dir = scratch("backup-beside");
        let file = dir.join("settings.flash");
        let mut cpu = idle_cpu();
        let original = cpu.persistent().unwrap();
        std::fs::write(&file, &original).unwrap();
        for name in &taken {
            std::fs::write(dir.join(name), format!("the older copy {name}")).unwrap();
        }
        cpu.use_settings_v2();

        load_flash(&mut cpu, file.to_str().unwrap()).unwrap();

        assert_eq!(
            std::fs::read(dir.join(want)).unwrap(),
            original,
            "{taken:?}: the new raw bytes are in {want}"
        );
        for name in &taken {
            assert_eq!(
                std::fs::read(dir.join(name)).unwrap(),
                format!("the older copy {name}").as_bytes(),
                "{taken:?}: {name} is left alone"
            );
        }
        let live = std::fs::read(&file).unwrap();
        assert!(live.starts_with(b"CMM2"), "the live file is v2 now");
        let mut names: Vec<String> = taken.iter().map(|name| name.to_string()).collect();
        names.push(want.to_string());
        names.push("settings.flash".to_string());
        names.sort();
        assert_eq!(files_in(&dir), names, "{taken:?}: nothing else was made");
    }
}

// Every name that is tried goes through the same check as `.v1`: a link or a directory at a later
// name is refused, never followed or replaced, and nothing at all is written.
#[test]
fn a_migration_refuses_a_later_backup_name_that_is_not_a_regular_file() {
    let outside = scratch("later-outside");
    let victim = outside.join("SavedFlash.bin");
    std::fs::write(&victim, b"somebody else's file").unwrap();
    for directory in [false, true] {
        let dir = scratch("later-not-a-file");
        let file = dir.join("settings.flash");
        let mut cpu = idle_cpu();
        let original = cpu.persistent().unwrap();
        std::fs::write(&file, &original).unwrap();
        std::fs::write(dir.join("settings.flash.v1"), b"the older copy").unwrap();
        let second = dir.join("settings.flash.v1.2");
        if directory {
            std::fs::create_dir(&second).unwrap();
        } else {
            symlink(&victim, &second).unwrap();
        }
        cpu.use_settings_v2();

        let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

        assert_eq!(
            err.to_string(),
            format!(
                "{}: not backing up the old settings to {}: that name is not a regular file. \
                 Move it out of the way and start again; no file was written.",
                file.display(),
                second.display()
            )
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            original,
            "the live file is as it was"
        );
        assert_eq!(
            files_in(&dir),
            ["settings.flash", "settings.flash.v1", "settings.flash.v1.2"]
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"somebody else's file");
    }
}

// `.v1` and `.v1.2` to `.v1.99` taken is the end of the names: refused, and nothing is written.
#[test]
fn a_migration_with_every_backup_name_taken_refuses_and_writes_nothing() {
    let dir = scratch("backups-full");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    let mut names = vec!["settings.flash.v1".to_string()];
    names.extend((2..=99).map(|n| format!("settings.flash.v1.{n}")));
    for name in &names {
        std::fs::write(dir.join(name), name).unwrap();
    }
    cpu.use_settings_v2();

    let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

    let path = file.display();
    assert_eq!(
        err.to_string(),
        format!(
            "{path}: not backing up the old settings: the names from {path}.v1 to {path}.v1.99 \
             are all taken. Move some of them out of the way and start again; no file was \
             written."
        )
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "the live file is as it was"
    );
    assert_eq!(files_in(&dir).len(), names.len() + 1, "no file was added");
    for name in &names {
        assert_eq!(
            std::fs::read(dir.join(name)).unwrap(),
            name.as_bytes(),
            "{name}"
        );
    }
}

// The migrated file is read back before any file is touched, so an encoder that wrote something
// the reader refuses stops the migration with nothing written. The encoder is handed in for the
// test: the real one never does this, and the read-back is what guards against the day it does.
#[test]
fn a_migration_whose_new_file_does_not_read_back_writes_nothing() {
    let dir = scratch("migrate-readback");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();
    // An older copy is there; offering it as the way back from this failure would be wrong, as
    // the live file is still the raw file it was.
    std::fs::write(dir.join("settings.flash.v1"), b"an older copy").unwrap();

    let err = migrate_with(
        &mut cpu,
        path,
        &original,
        |cpu| {
            let mut bytes = cpu.save_settings()?;
            let last = bytes.len() - 1;
            bytes[last] ^= 0x80; // a checksum that does not fit the file
            Ok(bytes)
        },
        create_new_atomically,
        read_file,
        replace_atomically,
    )
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "{path}: the new settings file made for the migration did not read back (the \
             checksum does not match the file); nothing was written, and {path} is untouched."
        )
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "the live file is as it was"
    );
    assert_eq!(
        files_in(&dir),
        ["settings.flash", "settings.flash.v1"],
        "nothing was added"
    );
}

// The board's `--flash <file>` is the board's file too: a raw file of any name is migrated the
// way `settings.flash` is, and its raw bytes are kept beside it.
#[test]
fn a_raw_file_given_to_a_board_with_flash_is_migrated_like_the_boards_own() {
    let dir = scratch("board-other-file");
    let file = dir.join("mine.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    cpu.use_settings_v2();

    load_flash(&mut cpu, file.to_str().unwrap()).unwrap();

    assert_eq!(std::fs::read(dir.join("mine.flash.v1")).unwrap(), original);
    assert!(std::fs::read(&file).unwrap().starts_with(&MAGIC));
    assert_eq!(files_in(&dir), ["mine.flash", "mine.flash.v1"]);
}

// A backup name that cannot be looked at is refused before anything is written. Here the name is
// a byte too long for the file system while the live file's is fine. (This is the name-check
// branch; the order of the two writes is pinned by the next test.)
#[test]
fn a_migration_whose_backup_name_cannot_be_looked_at_writes_nothing() {
    let dir = scratch("backup-name-too-long");
    let file = dir.join("a".repeat(253));
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    let before = std::fs::metadata(&file).unwrap();
    cpu.use_settings_v2();

    let err = load_flash(&mut cpu, file.to_str().unwrap()).unwrap_err();

    assert!(matches!(err, RunError::Flash { .. }), "{err}");
    assert!(
        err.to_string()
            .ends_with("File name too long (os error 36)"),
        "{err}"
    );
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert_eq!(std::fs::metadata(&file).unwrap().ino(), before.ino());
    assert_eq!(files_in(&dir), ["a".repeat(253)]);
}

// The live file is replaced only once its backup is safely in place. The backup name is chosen
// fine and the backup write then fails (by a writer handed in for the test), so the live file has
// to be left as it was: the same bytes in the same inode.
#[test]
fn a_migration_whose_backup_write_fails_leaves_the_live_file_alone() {
    let dir = scratch("backup-write-fails");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    let before = std::fs::metadata(&file).unwrap();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();

    let err = migrate_with(
        &mut cpu,
        path,
        &original,
        Cpu::save_settings,
        |_, _| Err(io::Error::other("the planned failure")),
        read_file,
        replace_atomically,
    )
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        format!("{path}.v1: the planned failure"),
        "the error names the backup"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "the live file is byte-identical"
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().ino(),
        before.ino(),
        "and was not replaced"
    );
    assert_eq!(files_in(&dir), ["settings.flash"]);
}

// If the live replace fails after the backup was written, the next start finds that backup (the
// same bytes) and uses it: it does not write the same bytes again under the next name. A raw
// file with other bytes still goes to the next free name.
#[test]
fn a_retried_migration_reuses_the_backup_it_already_made() {
    let dir = scratch("backup-retried");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();

    for _ in 0..3 {
        let err = migrate_with(
            &mut cpu,
            path,
            &original,
            Cpu::save_settings,
            create_new_atomically,
            read_file,
            |_, _| Err(io::Error::other("the planned failure")),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), format!("{path}: the planned failure"));
    }

    assert_eq!(files_in(&dir), ["settings.flash", "settings.flash.v1"]);
    assert_eq!(
        std::fs::read(dir.join("settings.flash.v1")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "the live file is intact"
    );

    // The same bytes again, this time with the live replace working: still the one backup.
    load_flash(&mut cpu, path).unwrap();
    assert_eq!(files_in(&dir), ["settings.flash", "settings.flash.v1"]);
    assert!(std::fs::read(&file).unwrap().starts_with(&MAGIC));

    // Other raw bytes are another file, and keep their own copy.
    let mut other = original.clone();
    other[0x40] ^= 0xFF;
    std::fs::write(&file, &other).unwrap();
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();
    load_flash(&mut cpu, path).unwrap();
    assert_eq!(
        files_in(&dir),
        ["settings.flash", "settings.flash.v1", "settings.flash.v1.2"]
    );
    assert_eq!(
        std::fs::read(dir.join("settings.flash.v1")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(dir.join("settings.flash.v1.2")).unwrap(),
        other
    );
}

// The copy is made with a hard link. A file system that refuses one stops the migration with a
// message that says why, and writes nothing: no copy by another way is tried, as a copy that
// could be replaced under us is no copy.
#[test]
fn a_file_system_that_refuses_the_hard_link_is_said_so_and_nothing_is_written() {
    let dir = scratch("backup-no-links");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();

    let err = migrate_with(
        &mut cpu,
        path,
        &original,
        Cpu::save_settings,
        |_, _| Err(io::Error::from(io::ErrorKind::Unsupported)),
        read_file,
        replace_atomically,
    )
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "{path}: not backing up the old settings to {path}.v1: the copy is made with a hard \
             link, and this file system refused one (unsupported). Nothing was \
             written, and {path} is untouched."
        )
    );
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert_eq!(files_in(&dir), ["settings.flash"]);
}

// A copy of another length cannot be a copy of this file, so it is not read at all; one of the
// same length is read to be compared.
#[test]
fn only_a_backup_of_the_same_length_is_read_to_see_if_it_is_the_copy() {
    let dir = scratch("backup-read-only-if-same-length");
    let file = dir.join("settings.flash");
    let original = vec![0x5Au8; 100];
    std::fs::write(&file, &original).unwrap();
    std::fs::write(dir.join("settings.flash.v1"), vec![0x5Au8; 99]).unwrap();
    std::fs::write(dir.join("settings.flash.v1.2"), vec![0x5Au8; 100]).unwrap();
    let read = std::cell::RefCell::new(Vec::new());
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();

    migrate_with(
        &mut cpu,
        path,
        &original,
        Cpu::save_settings,
        create_new_atomically,
        |name| {
            read.borrow_mut().push(name.to_string());
            std::fs::read(name)
        },
        replace_atomically,
    )
    .unwrap();

    assert_eq!(
        *read.borrow(),
        [format!("{path}.v1.2")],
        "only the one of the same length"
    );
    assert_eq!(
        files_in(&dir),
        ["settings.flash", "settings.flash.v1", "settings.flash.v1.2"]
    );
}

// Names that are taken again and again are looked at a bounded number of times: then the
// migration is refused and nothing is written. The writer fails with another error if the bound
// is ever lost, so that a loop that never ends does not hang the suite.
#[test]
fn a_backup_name_that_is_always_taken_before_the_write_ends_in_a_refusal() {
    let dir = scratch("backup-always-raced");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    let before = std::fs::metadata(&file).unwrap();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();
    let mut tries = 0;

    let err = migrate_with(
        &mut cpu,
        path,
        &original,
        Cpu::save_settings,
        |_, _| {
            tries += 1;
            Err(if tries > 1000 {
                io::Error::other("the bound was lost")
            } else {
                io::Error::from(io::ErrorKind::AlreadyExists)
            })
        },
        read_file,
        replace_atomically,
    )
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "{path}: not backing up the old settings: the names from {path}.v1 to {path}.v1.99 \
             are all taken. Move some of them out of the way and start again; no file was \
             written."
        )
    );
    assert_eq!(tries, 99, "one try for each name there is");
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert_eq!(std::fs::metadata(&file).unwrap().ino(), before.ino());
    assert_eq!(files_in(&dir), ["settings.flash"]);
}

// A file that appears at the chosen backup name between the check and the write is never
// replaced: the backup goes to the next free name instead.
#[test]
fn a_backup_name_taken_just_before_the_write_is_not_replaced() {
    let dir = scratch("backup-raced");
    let file = dir.join("settings.flash");
    let mut cpu = idle_cpu();
    let original = cpu.persistent().unwrap();
    std::fs::write(&file, &original).unwrap();
    cpu.use_settings_v2();
    let path = file.to_str().unwrap();
    let mut planted = false;

    migrate_with(
        &mut cpu,
        path,
        &original,
        Cpu::save_settings,
        |name, bytes| {
            if !std::mem::replace(&mut planted, true) {
                std::fs::write(name, b"somebody else's file")?;
            }
            create_new_atomically(name, bytes)
        },
        read_file,
        replace_atomically,
    )
    .unwrap();

    assert_eq!(
        std::fs::read(dir.join("settings.flash.v1")).unwrap(),
        b"somebody else's file"
    );
    assert_eq!(
        std::fs::read(dir.join("settings.flash.v1.2")).unwrap(),
        original
    );
    assert!(std::fs::read(&file).unwrap().starts_with(&MAGIC));
    assert_eq!(
        files_in(&dir),
        ["settings.flash", "settings.flash.v1", "settings.flash.v1.2"],
        "no temporary file is left"
    );
}

// Migration at the file level, over every size form a b5 file comes in. The tests above seed
// `cpu.persistent()`, so a backup taken from the machine and not from the file would pass them.
// The other five forms tell the two apart: the machine always adds the 32-byte block an old file
// lacks (917,512 bytes in the file, 917,544 from the machine), and turns the 16-byte offset tail
// into the chip's 10-byte state (917,552 in the file, 917,554 from the machine).
#[test]
fn every_b5_size_form_migrates_with_its_own_bytes_kept() {
    // The v1 tail: the clock's two words, then either the first DS3231 form's offset or the
    // chip's own state.
    const CLOCK_BYTES: usize = 8;
    let mut idle = idle_cpu();
    let today = idle.persistent().unwrap();
    let (regions, clock) = today.split_at(today.len() - CLOCK_BYTES);
    let mut offset_tail = clock.to_vec();
    offset_tail.extend((-3600i64).to_le_bytes()); // an hour behind UTC, as the old files saved it
    let mut state_tail = clock.to_vec();
    state_tail.extend([0u8; STATE_SIZE - 2]);
    state_tail.extend([1, 0x08]); // the chip's state, with the time marked set
    let tails = [
        ("clock", clock.to_vec()),
        ("offset", offset_tail),
        ("ds3231", state_tail),
    ];

    let mut seeds: Vec<(String, Vec<u8>)> = Vec::new();
    for (shape, body) in [("all", regions), ("old", &regions[0x20..])] {
        for (tail_name, tail) in &tails {
            let mut seed = body.to_vec();
            seed.extend(tail);
            seeds.push((format!("{shape} regions, {tail_name} tail"), seed));
        }
    }
    // The six forms the assertions below must be run over, written out: if the profile's table
    // ever changes, a form quietly dropping out of the sweep turns red here instead.
    assert_eq!(
        seeds
            .iter()
            .map(|(what, seed)| format!("{what}={}", seed.len()))
            .collect::<Vec<_>>(),
        [
            "all regions, clock tail=917544",
            "all regions, offset tail=917552",
            "all regions, ds3231 tail=917554",
            "old regions, clock tail=917512",
            "old regions, offset tail=917520",
            "old regions, ds3231 tail=917522",
        ],
        "every size form a b5 file comes in"
    );

    for (what, seed) in seeds {
        let size = seed.len();
        let dir = scratch("migrate-forms");
        let file = dir.join("settings.flash");
        let backup = dir.join("settings.flash.v1");
        std::fs::write(&file, &seed).unwrap();
        let mut cpu = idle_cpu();
        cpu.use_settings_v2();

        load_flash(&mut cpu, file.to_str().unwrap()).unwrap_or_else(|err| {
            panic!("{what}: a file of {size} bytes is one of this profile's own: {err}")
        });

        assert_eq!(
            std::fs::read(&backup).unwrap(),
            seed,
            "{what}: the backup is the file's own bytes, not what the machine writes today"
        );
        let live = std::fs::read(&file).unwrap();
        assert!(live.starts_with(&MAGIC), "{what}: the live file is v2 now");
        decode_v2(&live, cpu.image(), &B5)
            .unwrap_or_else(|err| panic!("{what}: the migrated file must read back: {err}"));
        // A board that starts again on the migrated file takes it as its own: the converted tail
        // of the offset form has to survive the round trip, not just its first write.
        let mut again = idle_cpu();
        again.use_settings_v2();
        load_flash(&mut again, file.to_str().unwrap()).unwrap_or_else(|err| {
            panic!("{what}: the migrated file will not load on the next start: {err}")
        });
        assert_eq!(
            files_in(&dir),
            ["settings.flash", "settings.flash.v1"],
            "{what}: migration leaves the file and its copy, nothing else"
        );
    }
}

// A run that reads the raw layout and finds a v2 file refuses it and says how to go on, in words
// that are true whichever run it was (a scratch run, or a board that had no recognised firmware).
#[test]
fn a_raw_run_that_meets_a_v2_file_says_how_to_go_on_without_naming_a_variable() {
    let dir = scratch("raw-meets-v2");
    let file = dir.join("mine.flash");
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();
    let v2 = cpu.save_settings().unwrap();
    std::fs::write(&file, &v2).unwrap();
    let path = file.to_str().unwrap();

    let err = load_flash(&mut idle_cpu(), path).unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "{path}: these settings were saved by the board of this emulator (format v2), and \
             this run reads the raw layout, which cannot hold them. Read them with the board \
             (cmm2-emulator --flash {path}, with no --firmware) once the firmware they were made \
             for is burned onto it, or point --flash at a raw file. To start fresh, move {path} \
             away."
        )
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        v2,
        "the refusal left the file alone"
    );
}

// The clause that offers a restore names the backup that is really there, whatever the
// `--flash` file was called.
#[test]
fn the_restore_clause_names_the_backup_of_the_file_that_was_named() {
    let dir = scratch("restore-names-itself");
    let file = dir.join("myflash");
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();
    let mut bytes = cpu.save_settings().unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80; // the checksum no longer fits the file
    std::fs::write(&file, &bytes).unwrap();
    std::fs::write(dir.join("myflash.v1"), b"the old dump").unwrap();
    let path = file.to_str().unwrap();

    let err = load_flash(&mut cpu, path).unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "{path}: the settings file is corrupt (the checksum does not match the file); \
             nothing was loaded. Restore {path}.v1 or start fresh by deleting them."
        )
    );
    assert!(
        !err.to_string().contains("settings.flash.v1"),
        "it names the file that exists, not the default one"
    );
}
