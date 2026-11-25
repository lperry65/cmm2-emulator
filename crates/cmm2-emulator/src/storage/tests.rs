use super::*;
use crate::args::RunOptions;
use crate::atomic_write::write_atomically;
use crate::error::RunError;
use crate::saveable::{check_regular, why_not_saveable};
use crate::test_support::{age, files_in, idle_cpu, mode, peak_rss_kib, root, scratch};
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
    load_flash(&mut second, path, None).unwrap();

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
    assert_eq!(std::fs::read(&file).unwrap().len(), B5.persistent_size());
    assert_eq!(B5.persistent_size(), 917_544);
    std::fs::remove_dir_all(&dir).unwrap();
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

    let err = load_flash(&mut cpu, file.to_str().unwrap(), None).unwrap_err();

    assert!(err.to_string().ends_with("more than 200 years"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

// The board's steady save, at the level the rest of these tests work at. Every other storage
// test seeds through `load_flash`; this one calls the save directly,
// because until now the v2 steady save was exercised only by the ignored real-firmware tests.
// The file must be the v2 form, and a link planted at the board's own name must be replaced and
// not written through - the promise the docs make about the board's own files rests on this line.
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
    std::fs::remove_dir_all(&outside).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_read_only_flash_file_is_saved_all_the_same_as_the_save_replaces_the_file() {
    let dir = scratch("flash-readonly");
    let file = dir.join("board.flash");
    let mut cpu = idle_cpu();
    std::fs::write(&file, cpu.persistent().unwrap()).unwrap();
    mode(&file, 0o444);
    let path = file.to_str().unwrap();

    load_flash(&mut cpu, path, None).unwrap();
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
    std::fs::write(&file, cpu.persistent().unwrap()).unwrap();
    mode(&dir, 0o555);
    let path = file.to_str().unwrap();

    // The file is writable and exists; the rename of a new one over it is what fails. A
    // first run, with no file yet, is refused too.
    let err = load_flash(&mut cpu, path, None).unwrap_err();
    let first = load_flash(&mut cpu, dir.join("new.flash").to_str().unwrap(), None).unwrap_err();

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

    let err = load_flash(&mut idle_cpu(), path, None).unwrap_err();

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
    let err = load_flash(&mut idle_cpu(), through_file.to_str().unwrap(), None).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "{}: {}/plain is not a directory, so the run could not save its flash and clock",
            through_file.display(),
            dir.display()
        )
    );
    let looped = dir.join("loop");
    let err = load_flash(&mut idle_cpu(), looped.to_str().unwrap(), None).unwrap_err();
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
        let flash = load_flash(&mut idle_cpu(), path, None).unwrap_err();
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
    let flash = load_flash(&mut idle_cpu(), "/tmp/cmm2-no-such-dir/flash/", None).unwrap_err();
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

        let flash = load_flash(&mut idle_cpu(), path, None).unwrap_err();
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

    load_flash(
        &mut idle_cpu(),
        dir.join("board.flash").to_str().unwrap(),
        None,
    )
    .unwrap();
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

    let flash = load_flash(&mut idle_cpu(), path, None).unwrap_err();
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
    load_flash(&mut cpu, longest.to_str().unwrap(), None).unwrap();
    save_flash(&mut cpu, &options).unwrap();
    assert_eq!(files_in(&dir), ["a".repeat(255)]);
    load_flash(&mut cpu, longest.to_str().unwrap(), None).unwrap();

    let too_long = dir.join("a".repeat(256));
    let err = load_flash(&mut cpu, too_long.to_str().unwrap(), None).unwrap_err();
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
        let err = load_flash(&mut unknown_cpu(), path, None).unwrap_err();
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
    std::fs::remove_dir_all(&dir).unwrap();
}

// A board of a firmware I know (v2 mode) that finds a raw file of the other release's size:
// the shipped message, kept verbatim, and no word of "corrupt" (docs/settings-format.md, rule 7
// of the reading rules).
#[test]
fn a_v2_mode_run_says_the_other_release_message_before_it_says_corrupt() {
    let dir = scratch("storage-other-release");
    let file = dir.join("settings.flash");
    let size = RC6.persistent_size();
    std::fs::write(&file, vec![0xFFu8; size]).unwrap();
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();

    let err = load_flash(&mut cpu, file.to_str().unwrap(), None).unwrap_err();

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
// largest file either format can be"). The size
// this test writes is far above any file's, and above the 128 MiB the test allows itself to
// grow by: 4 GiB of sparse nothing, which costs its reader about 4 GiB and this test nothing.
const TOO_BIG: u64 = 4 << 30;

#[test]
fn a_file_bigger_than_any_settings_file_is_refused_before_it_is_read() {
    let dir = scratch("flash-too-big");
    let file = dir.join("settings.flash");
    // Sparse, so the bytes are not really there: a run that reads the file whole makes them so.
    std::fs::File::create(&file)
        .unwrap()
        .set_len(TOO_BIG)
        .unwrap();
    let path = file.to_str().unwrap();
    let mut raw = idle_cpu();
    let mut board = idle_cpu();
    board.use_settings_v2();
    let before = peak_rss_kib();

    for cpu in [&mut raw, &mut board] {
        let err = load_flash(cpu, path, None).unwrap_err();
        let said = err.to_string();
        assert!(
            said.starts_with(&format!("{path}: the settings file is corrupt (")),
            "{said}"
        );
        assert!(said.contains(&format!("it is {TOO_BIG} bytes")), "{said}");
        assert!(
            said.contains("bytes any settings file of the firmwares I know can be"),
            "{said}"
        );
        assert!(said.ends_with("); nothing was loaded."), "{said}");
    }
    // The refusal is not allowed to have read the file. Measured, not guessed: run alone, and run
    // beside the rest of `storage`, the process peak moved by 0 or by 128 KiB across the runs made
    // while writing this, in no order that repeats. Reading a file this size would move it by
    // about the file's own size: 1,136,068 KiB for a 1 GiB one, against an 87,292 KiB baseline.
    // The peak is the process's highest, so tests running
    // alongside raise it and nothing lowers it; 128 MiB is the room that needs, and a
    // thirty-second of what reading this file would cost.
    assert!(
        peak_rss_kib() - before < 128 * 1024,
        "the peak grew by {} KiB, more than a refusal that leaves the file unread",
        peak_rss_kib() - before
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// The bound is the format's own, so a file of exactly the largest size is not refused by it:
// what it is refused for is what it actually gets wrong, which here is a tail of nothing.
#[test]
fn a_file_of_exactly_the_largest_size_is_not_refused_on_its_size() {
    let dir = scratch("flash-largest");
    let file = dir.join("settings.flash");
    let largest = Profile::largest_settings_file();
    let mut cpu = idle_cpu();
    cpu.use_settings_v2();
    let mut bytes = cpu.save_settings().unwrap();
    // Its header names this run's image, so the gate is passed; what the file gets wrong is
    // that it is nothing but tail, right up to the last byte the format can be.
    bytes.resize(largest, 0);
    assert_eq!(bytes.len(), largest);
    std::fs::write(&file, &bytes).unwrap();
    let path = file.to_str().unwrap();

    let err = load_flash(&mut cpu, path, None).unwrap_err();

    let said = err.to_string();
    assert!(said.contains("the tail is "), "what it gets wrong: {said}");
    assert!(
        !said.contains("more than the "),
        "the size bound refused a file that size allows: {said}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
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

        let err = load_flash(&mut cpu, file.to_str().unwrap(), None).unwrap_err();

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
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::remove_dir_all(&outside).unwrap();
}

// The same step with the two names it is meant to see: a name that is free takes the old bytes,
// and a backup that is already there is the older copy and is left alone.
#[test]
fn a_migration_keeps_the_old_bytes_beside_the_live_file() {
    for existing in [None, Some(b"the older copy".as_slice())] {
        let name = if existing.is_some() {
            "backup-already-there"
        } else {
            "backup-beside"
        };
        let dir = scratch(name);
        let file = dir.join("settings.flash");
        let backup = dir.join("settings.flash.v1");
        let mut cpu = idle_cpu();
        let original = cpu.persistent().unwrap();
        std::fs::write(&file, &original).unwrap();
        if let Some(old) = existing {
            std::fs::write(&backup, old).unwrap();
        }
        cpu.use_settings_v2();

        load_flash(&mut cpu, file.to_str().unwrap(), None).unwrap();

        assert_eq!(
            std::fs::read(&backup).unwrap(),
            existing.unwrap_or(&original),
            "the backup is {}",
            existing.map_or("the file that was replaced", |_| "the one already there")
        );
        let live = std::fs::read(&file).unwrap();
        assert!(live.starts_with(b"CMM2"), "the live file is v2 now");
        assert_eq!(files_in(&dir), ["settings.flash", "settings.flash.v1"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

// Migration at the file level, over every size form a b5 file comes in. The test above seeds
// `cpu.persistent()`, which is today's own shape, so a backup taken
// from the machine rather than from the file would pass it: measured, that seed is 917,544 bytes
// in the file and 917,544 from the machine. The other five forms tell the two apart, by the two
// normalizations the machine always applies - the 32-byte block an old file lacks (917,512 in the
// file, 917,544 from the machine), and the 16-byte offset tail that becomes the chip's 10-byte
// state (917,552 in the file, 917,554 from the machine).
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

        load_flash(&mut cpu, file.to_str().unwrap(), None).unwrap_or_else(|err| {
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
        load_flash(&mut again, file.to_str().unwrap(), None).unwrap_or_else(|err| {
            panic!("{what}: the migrated file will not load on the next start: {err}")
        });
        assert_eq!(
            files_in(&dir),
            ["settings.flash", "settings.flash.v1"],
            "{what}: migration leaves the file and its copy, nothing else"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
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

    let err = load_flash(&mut cpu, path, None).unwrap_err();

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
    std::fs::remove_dir_all(&dir).unwrap();
}
