//! The real firmware with a card in the socket, and with none.

mod common;

use cmm2_core::{Cpu, Irqs, cmm2_memory};
use common::*;

// A 64 MB FAT32 card image made with mkfs.vfat. The test is ignored and already needs the
// firmware, and it needs this tool as well (dosfstools): without it the test fails and says so,
// and does not pass having tested nothing. The file lives in the temp directory under the
// test's own name.
fn fat32_image(name: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("cmm2-core-{}-{name}.img", std::process::id()));
    std::fs::File::create(&path)
        .and_then(|file| file.set_len(64 << 20))
        .unwrap();
    let made = std::process::Command::new("mkfs.vfat")
        .args(["-F", "32", "-n", "CMM2"])
        .arg(&path)
        .output();
    let image = match made {
        Ok(output) if output.status.success() => Ok(std::fs::read(&path).unwrap()),
        Ok(output) => Err(format!(
            "mkfs.vfat: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
        Err(err) => Err(format!(
            "this test needs mkfs.vfat (the dosfstools package) to make a card image: {err}"
        )),
    };
    std::fs::remove_file(&path).unwrap();
    image.unwrap_or_else(|message| panic!("{message}"))
}

// A little-endian number of up to four bytes.
fn le(bytes: &[u8]) -> usize {
    bytes.iter().rev().fold(0, |n, &b| n << 8 | b as usize)
}

// The contents of the file `name` (8.3 as stored: eight characters of name and three of
// extension, blank padded) in the root directory of a FAT32 image. The directory and the
// file have to fit in their first cluster.
fn fat32_file(image: &[u8], name: &[u8; 11]) -> Option<Vec<u8>> {
    let bytes_per_sector = le(&image[11..13]);
    let cluster_size = image[13] as usize * bytes_per_sector;
    let fat_sectors = le(&image[36..40]);
    let data = (le(&image[14..16]) + image[16] as usize * fat_sectors) * bytes_per_sector;
    let cluster = |n: usize| data + (n - 2) * cluster_size;
    let root = &image[cluster(le(&image[44..48]))..][..cluster_size];
    let entry = root.chunks(32).find(|entry| entry[..11] == name[..])?;
    let first = le(&entry[20..22]) << 16 | le(&entry[26..28]);
    Some(image[cluster(first)..][..le(&entry[28..32])].to_vec())
}

// The CMM2 has no SAVE or LOAD for programs: AUTOSAVE takes a program typed at the console
// into a file (F1, as ESC O P, ends it and loads the file too), and RUN and LIST take a file
// name. The first command that reaches the card also initialises it, which costs about 5
// million instructions (the bit-bang runs slowly until the card is up); the later ones are
// quick.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_lists_saves_and_runs_a_program_on_a_card() {
    let image = fat32_image("saves");
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let sd = memory.sd_slot();
    sd.insert(image).unwrap();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    let mut say = |keys: &str| type_keys(&mut cpu, &console, keys).0;

    let files = say("FILES\r");
    assert!(files.contains("0 directories, 0 files"), "{files:?}");
    assert!(!files.contains("not found"), "{files:?}");
    say("NEW\r");
    let saved = say("AUTOSAVE \"T.BAS\"\rPRINT \"FROM THE CARD\"\r\x1bOP");
    assert!(saved.contains("Saved to SDcard"), "{saved:?}");
    let files = say("FILES\r");
    assert!(files.contains("T.BAS"), "{files:?}");

    // The program in memory is the one just saved. After NEW it is gone, and comes back
    // from the card.
    assert!(say("LIST\r").contains("PRINT \"FROM THE CARD\""));
    say("NEW\r");
    assert!(!say("LIST\r").contains("FROM THE CARD"));
    assert!(say("LIST \"T.BAS\"\r").contains("PRINT \"FROM THE CARD\""));
    let ran = say("RUN \"T.BAS\"\r");
    assert!(ran.contains("FROM THE CARD\r\n"), "{ran:?}");

    let image = sd.eject().unwrap();
    let file = fat32_file(&image, b"T       BAS").expect("no T.BAS on the card");
    assert!(String::from_utf8_lossy(&file).contains("PRINT \"FROM THE CARD\""));
}

// Issue 64: MMBasic ranks identifiers, and from rank 63 up it builds the code with a divide by 63
// that the compiler turned into a SMULL by 0x82082083. The core ran SMULL unsigned, so a program
// with more than about 85 distinct names got corrupt tokens, and RUN stopped with "Program damaged:
// symbol table". Before the fix that was all RUN printed.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_a_program_with_a_hundred_identifiers() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let sd = memory.sd_slot();
    sd.insert(fat32_image("identifiers")).unwrap();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);

    let mut lines: Vec<String> = (0..100).map(|n| format!("CONST C{n:03} = {n}")).collect();
    lines.extend(["PRINT \"SUM\"; C099", "END", "SUB SUBA", "END SUB"].map(String::from));

    // The console's input buffer is small, so I feed AUTOSAVE a line at a time and let the firmware
    // catch up before the next. Each line ends in LF. That is the firmware's doing, not the
    // emulator's: the console passes a CR through unchanged, AUTOSAVE stores a bare CR as it is,
    // and the firmware's own loader does not take that as a line end, so RUN says "Line is too
    // long".
    console.push_input(b"AUTOSAVE \"IDS.BAS\"\r");
    cpu.run(10_000_000);
    for line in &lines {
        console.push_input(format!("{line}\n").as_bytes());
        cpu.run(300_000);
        console.take_output();
    }
    console.push_input(b"\x1bOP");
    run_until(&mut cpu, &console, 2_000_000_000, |output| {
        output.contains("Saved to SDcard\r\n> ")
    });
    let mut say = |keys: &str| type_keys(&mut cpu, &console, keys).0;

    let ran = say("RUN \"IDS.BAS\"\r");
    assert!(!ran.contains("damaged"), "{ran:?}");
    assert!(ran.contains("SUM 99"), "{ran:?}");
}

// A card with its write-protect switch on is read, and the firmware refuses a write to it, with
// its own message; the image is left as it was and the card is not marked as written to.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_refuses_to_write_to_a_write_protected_card() {
    let image = fat32_image("protected");
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let sd = memory.sd_slot();
    sd.insert(image.clone()).unwrap();
    sd.set_write_protected(true);
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);
    let mut say = |keys: &str| type_keys(&mut cpu, &console, keys).0;

    let files = say("FILES\r");
    assert!(files.contains("0 directories, 0 files"), "{files:?}");
    let refused = say("OPEN \"W.TXT\" FOR OUTPUT AS #1\r");
    assert!(refused.contains("write protected"), "{refused:?}");
    assert!(!sd.modified());
    assert_eq!(sd.eject().unwrap(), image);
}

// With no card the socket reports it empty, and the firmware says so without a card to ask.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_says_the_card_is_not_present_with_no_card() {
    let firmware = firmware();
    let irqs = Irqs::new();
    let memory = cmm2_memory(&irqs);
    let console = memory.console();
    let mut cpu = Cpu::new(&firmware, memory, irqs).unwrap();
    boot_to_second_prompt(&mut cpu, &console);

    // The first error the firmware prints after a boot comes out empty (not understood yet),
    // so the second FILES is the one that says it.
    type_keys(&mut cpu, &console, "FILES\r");
    let (files, _) = type_keys(&mut cpu, &console, "FILES\r");
    assert!(files.contains("SD card not present"), "{files:?}");
}
