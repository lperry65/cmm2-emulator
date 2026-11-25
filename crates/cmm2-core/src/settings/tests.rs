//! The settings file format v2 (docs/settings-format.md): what it accepts, what it refuses,
//! and the proof that a migrated file leaves the machine - flash, clock and clock chip - as
//! the old one did.

use super::*;
use crate::Cpu;
use crate::ds3231::STATE_SIZE;
use crate::profile::B5_IMAGE_SHA256;
use crate::profile::CLOCK_SIZE;
use crate::{B5, FLASH_BASE, Firmware, Irqs, RC6, Region, cmm2_memory};

// The b5 sizes as the old files had them: the block and the user flash, then the tails. The
// numbers repeat the profile table's on purpose, as in cpu/tests.rs.
const BLOCK_SIZE: usize = 0x20;
const USER_FLASH: u32 = 0xE_0000;

/// A synthetic image of `len` bytes: a vector table, then a counting pattern, so rows inside
/// the image are not alike and never all FF.
fn synthetic_image(len: usize) -> Vec<u8> {
    let mut image = vec![0u8; len];
    image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x0800_0101u32.to_le_bytes());
    for (at, byte) in image.iter_mut().enumerate().skip(0x100) {
        *byte = (at % 251) as u8;
    }
    image
}

fn machine(image: &[u8], profile: &'static Profile) -> Cpu {
    let firmware = Firmware::from_bytes(image.to_vec())
        .unwrap()
        .with_profile(profile);
    Cpu::new(&firmware, cmm2_memory(&Irqs::new()), Irqs::new()).unwrap()
}

/// What the baseline says a byte is: the image's own inside it, FF past its end.
fn baseline(image: &[u8], addr: u32) -> u8 {
    let inside = (addr - FLASH_BASE) as usize;
    image.get(inside).copied().unwrap_or(0xFF)
}

fn baseline_row(image: &[u8], addr: u32) -> Vec<u8> {
    (0..ROW as u32).map(|i| baseline(image, addr + i)).collect()
}

/// A v1 file: the regions' bytes (the baseline everywhere, with `patch`ed runs laid over them
/// at their flash addresses), then `tail`.
fn v1_file(
    profile: &Profile,
    image: &[u8],
    regions: &[Region],
    tail: &[u8],
    patch: &[(u32, &[u8])],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(profile.flash_size());
    for &Region { addr, len } in regions {
        for at in addr..addr + len {
            bytes.push(baseline(image, at));
        }
    }
    for &(at, value) in patch {
        let region = regions
            .iter()
            .find(|r| at >= r.addr && at + value.len() as u32 <= r.addr + r.len)
            .expect("a patch is inside a region");
        let offset = regions
            .iter()
            .take_while(|r| r.addr < region.addr)
            .map(|r| r.len as usize)
            .sum::<usize>()
            + (at - region.addr) as usize;
        bytes[offset..offset + value.len()].copy_from_slice(value);
    }
    bytes.extend(tail);
    bytes
}

fn clock_tail() -> Vec<u8> {
    let mut tail = Vec::new();
    tail.extend(
        [0x12_30_45u32, 0x26_91_12]
            .iter()
            .flat_map(|w| w.to_le_bytes()),
    );
    tail
}

/// The tails a v1 file could end with: the clock, the clock and the first DS3231 form's
/// offset, and the clock and the DS3231's own state.
fn tails() -> [(&'static str, Vec<u8>); 3] {
    let clock = clock_tail();
    let mut offset = clock.clone();
    offset.extend((-3600i64).to_le_bytes());
    let mut state = clock.clone();
    state.extend([0u8; STATE_SIZE - 2]);
    state.push(1);
    state.push(0x08);
    [("clock", clock), ("offset", offset), ("ds3231", state)]
}

/// The forms a v1 file of `profile` comes in, as `regions_for` counts them today: six for b5
/// (two region sets, three tails each) and three for RC6 (one region set), nine in all.
fn v1_forms(profile: &'static Profile) -> Vec<(String, &'static [Region], Vec<u8>)> {
    tails()
        .into_iter()
        .flat_map(|(form, tail)| {
            std::iter::once((format!("all {form}"), profile.regions, tail.clone())).chain(
                profile
                    .old_regions()
                    .map(|old| (format!("old {form}"), old, tail)),
            )
        })
        .collect()
}

/// The load order's proof: machine A takes the v1 bytes (today's path), B the v2 encoding of
/// the same state, and the two machines are read equal - the whole 2 MB flash array word for
/// word, the clock, and the DS3231's kept state. The clock's own bytes must also match the
/// file's: every v1 tail starts with them, and migration carries them unchanged. The 16-byte
/// offset form converts once, into the chip's own state (docs/settings-format.md), so it is
/// compared as the chip keeps it, on both machines.
fn assert_migrates_word_for_word(image: &[u8], profile: &'static Profile, v1: &[u8]) -> Settings {
    let mut a = machine(image, profile);
    a.load_persistent(v1)
        .expect("the v1 file loads on today's path");
    let v2 = a.save_settings().expect("the machine encodes v2");
    let settings = decode_v2(&v2, image, profile).expect("the v2 bytes read back");
    let mut b = machine(image, profile);
    b.load_settings(&settings).expect("the v2 settings apply");
    for addr in (FLASH_BASE..FLASH_BASE + 0x20_0000).step_by(4) {
        assert_eq!(
            a.read32(addr),
            b.read32(addr),
            "the flash differs at {addr:#010X}, and the image ends at {:#010X}",
            FLASH_BASE + image.len() as u32
        );
    }
    let regions = profile
        .regions_for(v1.len())
        .expect("the helper is only given v1 files of this profile");
    let clock_at = regions
        .iter()
        .map(|region| region.len as usize)
        .sum::<usize>();
    assert_eq!(
        settings.clock,
        [
            u32::from_le_bytes(v1[clock_at..clock_at + 4].try_into().expect("a clock word")),
            u32::from_le_bytes(
                v1[clock_at + 4..clock_at + 8]
                    .try_into()
                    .expect("a clock word")
            ),
        ],
        "the clock's file bytes cross the migration unchanged"
    );
    let kept = |cpu: &Cpu| {
        let memory = cpu.memory();
        (memory.rtc().calendar(), memory.ds3231().saved())
    };
    assert_eq!(
        kept(&a),
        kept(&b),
        "the clock and the chip's kept state agree between the v1 machine and the migrated one"
    );
    settings
}

/// A settings for the round-trip tests: rows inside both of b5's regions.
fn sample() -> Settings {
    Settings {
        entries: vec![
            (0x080F_8540, vec![0x11; 32]),
            (0x0812_0000, vec![0x22; 64]),
            (0x0812_0080, vec![0x33; 32]),
        ],
        clock: [0x12_30_45, 0x26_91_12],
        ds3231: None,
    }
}

fn encoded_sample(image: &[u8]) -> Vec<u8> {
    encode(image, &sha256(image), &sample())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// The accepts.

#[test]
fn crc32_is_the_ieee_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(&[]), 0);
}

#[test]
fn sha256_is_the_known_digest_of_abc() {
    assert_eq!(
        sha256(b"abc"),
        [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad
        ]
    );
}

#[test]
fn a_round_trip_with_no_entries_survives() {
    let image = synthetic_image(1 << 16);
    let settings = Settings {
        entries: vec![],
        clock: [7, 9],
        ds3231: None,
    };
    let bytes = encode(&image, &sha256(&image), &settings);
    assert_eq!(decode_v2(&bytes, &image, &B5).unwrap(), settings);
}

#[test]
fn a_round_trip_keeps_entries_clock_and_ds3231() {
    let image = synthetic_image(1 << 16);
    for ds3231 in [None, Some([1u8; STATE_SIZE])] {
        let settings = Settings { ds3231, ..sample() };
        let bytes = encode(&image, &sha256(&image), &settings);
        assert_eq!(decode_v2(&bytes, &image, &B5).unwrap(), settings);
    }
}

// A v2 file written out byte by byte, so that the layout in docs/settings-format.md is pinned by
// something other than this crate's own encoder. Every field was worked out by hand from that
// page; the SHA-256 and the CRC-32 came from Python (`hashlib.sha256` and `zlib.crc32`), not from
// this crate. The image is the 15 bytes of b"golden image 01".
const GOLDEN_IMAGE: &[u8] = b"golden image 01";

fn golden_file() -> Vec<u8> {
    [
        // Header, 48 bytes: the magic, version 2, flags 0, reserved 0 0.
        &[0x43, 0x4d, 0x4d, 0x32, 0x02, 0x00, 0x00, 0x00][..],
        // image_len = 15, little-endian.
        &[0x0f, 0x00, 0x00, 0x00],
        // image_hash = hashlib.sha256(GOLDEN_IMAGE).digest().
        &[
            0xfd, 0x0b, 0xa8, 0xb9, 0x24, 0xf2, 0x83, 0x95, 0x56, 0x55, 0x32, 0xd2, 0x6d, 0x80,
            0x01, 0xe1, 0x3a, 0xa4, 0x31, 0x27, 0x68, 0x4e, 0x4e, 0x7f, 0x50, 0x99, 0x21, 0x81,
            0x2d, 0x67, 0x58, 0x02,
        ],
        // entry_count = 2.
        &[0x02, 0x00, 0x00, 0x00],
        // Entry 1: addr 0x080F8540 (the b5 block), len 32, then 32 bytes of 0xA5.
        &[0x40, 0x85, 0x0f, 0x08, 0x20, 0x00, 0x00, 0x00],
        &[0xa5; 32],
        // Entry 2: addr 0x08120020 (the row after the first of the user flash), len 64, then the
        // bytes 0x40 to 0x7F.
        &[0x20, 0x00, 0x12, 0x08, 0x40, 0x00, 0x00, 0x00],
        &[
            0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d,
            0x4e, 0x4f, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b,
            0x5c, 0x5d, 0x5e, 0x5f, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
            0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77,
            0x78, 0x79, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e, 0x7f,
        ],
        // Clock: TR 0x00123045, then DR 0x00269112, a little-endian word each.
        &[0x45, 0x30, 0x12, 0x00, 0x12, 0x91, 0x26, 0x00],
        // The DS3231's state, 10 bytes: 1 to 10.
        &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a],
        // CRC-32 of the 178 bytes above (zlib.crc32 = 0xE30C7CB7), little-endian.
        &[0xb7, 0x7c, 0x0c, 0xe3],
    ]
    .concat()
}

#[test]
fn a_golden_v2_file_reads_and_is_written_back_byte_for_byte() {
    let file = golden_file();
    assert_eq!(
        file.len(),
        HEADER_SIZE + (8 + 32) + (8 + 64) + 8 + STATE_SIZE + CRC_SIZE
    );
    let want = Settings {
        entries: vec![
            (0x080F_8540, vec![0xA5; 32]),
            (0x0812_0020, (0x40..0x80).collect()),
        ],
        clock: [0x0012_3045, 0x0026_9112],
        ds3231: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    };

    assert_eq!(decode_v2(&file, GOLDEN_IMAGE, &B5).unwrap(), want);
    assert_eq!(encode(GOLDEN_IMAGE, &sha256(GOLDEN_IMAGE), &want), file);
}

#[test]
fn encoding_the_same_settings_twice_gives_the_same_bytes() {
    let image = synthetic_image(1 << 16);
    assert_eq!(
        encoded_sample(&image),
        encode(&image, &sha256(&image), &sample())
    );
}

#[test]
fn every_row_changed_is_a_full_dump_and_round_trips() {
    let image = synthetic_image(1 << 16);
    // The pathological case: no entry left, the entries are a full dump of the regions.
    let settings = Settings {
        entries: vec![
            (0x080F_8540, vec![0u8; BLOCK_SIZE]),
            (0x0812_0000, vec![0u8; USER_FLASH as usize]),
        ],
        clock: [0, 0],
        ds3231: None,
    };
    let bytes = encode(&image, &sha256(&image), &settings);
    assert_eq!(decode_v2(&bytes, &image, &B5).unwrap(), settings);
}

#[test]
fn classify_sorts_the_two_formats_and_the_other_release() {
    let image = synthetic_image(1 << 16);
    let v2 = encoded_sample(&image);
    assert!(matches!(classify(&v2, &B5).unwrap(), FileKind::V2));
    for (what, regions, tail) in v1_forms(&B5) {
        let bytes = v1_file(&B5, &image, regions, &tail, &[]);
        assert!(
            matches!(classify(&bytes, &B5).unwrap(), FileKind::V1),
            "a b5 file of {what} is v1"
        );
    }
    for (what, regions, tail) in v1_forms(&RC6) {
        let bytes = v1_file(&RC6, &image, regions, &tail, &[]);
        assert!(
            matches!(classify(&bytes, &B5).unwrap(), FileKind::OtherRelease(p) if p.key == "rc6"),
            "an RC6 file of {what} is the other release"
        );
    }
}

#[test]
fn the_v1_sizes_are_exactly_nine_forms() {
    // b5: both region sets with all three tails; RC6: one set (its block was never new).
    assert_eq!(v1_forms(&B5).len(), 6);
    assert_eq!(v1_forms(&RC6).len(), 3);
    let image = synthetic_image(1 << 16);
    for (what, regions, tail) in v1_forms(&B5) {
        let bytes = v1_file(&B5, &image, regions, &tail, &[]);
        assert!(
            B5.regions_for(bytes.len()).is_some(),
            "regions_for must still take a {what} file"
        );
        assert!(
            B5.regions_for(bytes.len() - 1).is_none(),
            "one byte less than {what} is not a size"
        );
        assert!(
            B5.regions_for(bytes.len() + 1).is_none(),
            "one byte more than {what} is not a size"
        );
    }
}

// The rejects. Each file has exactly one wrong thing.

fn tamper(image: &[u8], at: usize, bytes: &[u8]) -> Vec<u8> {
    let mut file = encoded_sample(image);
    file[at..at + bytes.len()].copy_from_slice(bytes);
    file
}

#[test]
fn a_wrong_size_that_is_no_ones_is_corrupt() {
    let err = classify(&[0u8; 7], &B5).unwrap_err();
    assert!(
        err.to_string().contains("corrupt"),
        "a stranger is corrupt, not new: {err}"
    );
}

#[test]
fn a_newer_version_is_refused_as_newer() {
    let image = synthetic_image(1 << 16);
    for version in [3u8, 255] {
        let err = decode_v2(&tamper(&image, 4, &[version]), &image, &B5).unwrap_err();
        assert!(
            matches!(err, SettingsError::Newer { version: v } if v == u32::from(version)),
            "version {version}: {err}"
        );
        assert!(
            err.to_string().contains("newer than this emulator's 2"),
            "the newer message names the emulator's version: {err}"
        );
    }
    // A version below 2 with the magic is not readable, and says so as corrupt.
    let err = decode_v2(&tamper(&image, 4, &[1]), &image, &B5).unwrap_err();
    assert!(matches!(err, SettingsError::Corrupt(_)), "version 1: {err}");
}

#[test]
fn flags_or_reserved_bits_that_are_not_zero_are_refused() {
    let image = synthetic_image(1 << 16);
    for at in [5usize, 6, 7] {
        let err = decode_v2(&tamper(&image, at, &[0x02]), &image, &B5).unwrap_err();
        assert!(
            matches!(err, SettingsError::ReservedBits { version: 2 }),
            "byte {at} set: {err}"
        );
        assert!(err.to_string().contains("Update the emulator"), "{err}");
    }
}

#[test]
fn the_hash_gate_fires_on_a_wrong_length_or_hash_and_names_both() {
    let image = synthetic_image(1 << 16);
    let mut other = synthetic_image(1 << 16);
    other[0x120] ^= 0xFF;

    let saved_for_other = encode(&other, &sha256(&other), &sample());
    let err = decode_v2(&saved_for_other, &image, &B5).unwrap_err();
    let (saved, running) = match &err {
        SettingsError::Mismatch { saved, running } => (saved, running),
        other => {
            panic!("the same length with a different hash is the hash gate, not {other}");
        }
    };
    assert_eq!(saved.len as usize, image.len());
    assert_eq!(saved.sha256, sha256(&other));
    assert_eq!(running.sha256, sha256(&image));
    assert_eq!(running.name, Some("7.00.00b5"));
    let said = err.to_string();
    // The names come from the lengths, and the first 12 hex of each hash is shown.
    assert!(said.contains("image 65536 bytes"), "{said}");
    assert_eq!(
        said,
        format!(
            "these settings were saved for a different firmware (unknown firmware, image 65536 \
             bytes, sha256 {}; this run's is 7.00.00b5, image 65536 bytes, sha256 {}). Burn the \
             firmware they were made for, or start fresh by deleting them.",
            hex(&sha256(&other)[..6]),
            hex(&sha256(&image)[..6]),
        )
    );
    assert!(
        said.contains(&format!("sha256 {}", hex(&sha256(&other)[..6]))),
        "{said}"
    );
    assert!(
        said.contains("Burn the firmware they were made for"),
        "{said}"
    );

    // A wrong length is the same gate.
    let mut file = encoded_sample(&image);
    file[8..12].copy_from_slice(&((image.len() + 1) as u32).to_le_bytes());
    assert!(
        matches!(
            decode_v2(&file, &image, &B5),
            Err(SettingsError::Mismatch { .. })
        ),
        "a wrong image_len is the hash gate"
    );
}

// Every byte of the hash counts: a gate that looked at the first half only would take another
// image's settings.
#[test]
fn the_hash_gate_compares_every_byte_of_the_hash() {
    let image = synthetic_image(1 << 16);
    for byte in [0usize, 15, 16, 31] {
        let mut file = encoded_sample(&image);
        file[12 + byte] ^= 0x01;
        assert!(
            matches!(
                decode_v2(&file, &image, &B5),
                Err(SettingsError::Mismatch { .. })
            ),
            "a hash that differs in byte {byte} only is another image's"
        );
    }
}

// An entry is whole inside one region: at the edges of every region of both profiles, the first
// and last rows are taken, and a row past either end, or an entry that runs over one, is not.
#[test]
fn entries_are_accepted_only_whole_inside_a_region() {
    let image = synthetic_image(1 << 16);
    let row = ROW as u32;
    for profile in [&B5, &RC6] {
        let read = |addr: u32, len: u32| {
            let settings = Settings {
                entries: vec![(addr, vec![0x5A; len as usize])],
                clock: [0, 0],
                ds3231: None,
            };
            decode_v2(&encode(&image, &sha256(&image), &settings), &image, profile)
        };
        for region in profile.regions {
            let end = region.addr + region.len;
            let name = format!("{} region at {:#010X}", profile.name, region.addr);
            assert!(read(region.addr, row).is_ok(), "{name}: the first row");
            assert!(read(end - row, row).is_ok(), "{name}: the last row");
            assert!(read(end, row).is_err(), "{name}: one row past the end");
            assert!(
                read(region.addr - row, row).is_err(),
                "{name}: one row before"
            );
            assert!(
                read(end - row, 2 * row).is_err(),
                "{name}: runs one row over the end"
            );
            assert!(
                read(region.addr - row, 2 * row).is_err(),
                "{name}: starts one row before"
            );
        }
        // The last region ends where flash does: an entry there that runs past it would write
        // beyond the flash.
        let last = profile.regions.last().unwrap();
        assert_eq!(last.addr + last.len, 0x0820_0000, "{}", profile.name);
        assert!(
            read(0x081F_FFE0, 64).is_err(),
            "{}: past the end of flash",
            profile.name
        );
        // An entry that spans two regions is refused too, whatever lies between them.
        let (first, second) = (profile.regions[0], profile.regions[1]);
        let span = second.addr + row - first.addr;
        assert!(
            read(first.addr, span).is_err(),
            "{}: spans two regions",
            profile.name
        );
    }
}

#[test]
fn an_unknown_saved_firmware_is_named_as_unknown() {
    let image = synthetic_image(1 << 16);
    // A header that claims a length no profile of mine has.
    let mut file = encoded_sample(&image);
    file[8..12].copy_from_slice(&123_456u32.to_le_bytes());
    file[12..44].copy_from_slice(&sha256(&vec![7u8; 123_456]));
    let err = decode_v2(&file, &image, &B5).unwrap_err();
    match &err {
        SettingsError::Mismatch { saved, .. } => assert_eq!(saved.name, None),
        other => panic!("a length I do not know is the hash gate, not {other}"),
    }
    assert!(err.to_string().contains("unknown firmware"), "{err}");
}

// The other release's image length, repeated from the profile table on purpose: the point is a
// header that tells that lie about an image whose hash I do have.
const OTHER_RELEASE_LEN: u32 = 1_409_568;

// A header's length is the writer's claim; its hash is the thing the gate checks. Naming a
// release from the length alone lets a crafted one tell the user to burn a firmware that cannot
// end the mismatch, so a length alone buys no firmware name (profile.rs, `Profile::named_release`).
#[test]
fn a_crafted_length_buys_no_firmware_name_in_the_refusal() {
    let image = synthetic_image(1 << 16);
    let err = decode_v2(
        &tamper(&image, 8, &OTHER_RELEASE_LEN.to_le_bytes()),
        &image,
        &B5,
    )
    .unwrap_err();
    let (saved, running) = match &err {
        SettingsError::Mismatch { saved, running } => (saved, running),
        other => panic!("a length that is not this image's is the hash gate, not {other}"),
    };
    assert_eq!(saved.name, None, "a length on its own names no release");
    assert_eq!(running.name, Some("7.00.00b5"));

    // Only the saved half of the message, since this run's image is named, and rightly so.
    let said = err.to_string();
    let (said, _) = said
        .split_once("; this run's is")
        .expect("the message names both images");
    assert!(!said.contains("7.00.00"), "no release is named: {said}");
    assert!(said.contains("unknown firmware"), "{said}");
    assert!(
        said.contains(&format!("image {OTHER_RELEASE_LEN} bytes")),
        "{said}"
    );
    assert!(
        said.contains(&format!("sha256 {}", hex(&sha256(&image)[..6]))),
        "the length is refused, but the hash is still shown: {said}"
    );
}

// The other half of that rule: a header whose length AND hash are one release's own does buy
// its name, so a file from the other firmware still says which one to burn.
#[test]
fn a_saved_release_is_named_when_its_length_and_hash_are_both_its_own() {
    // A header carrying b5's length and the SHA-256 of its real image, met by a stranger.
    let claimed_for_b5 = synthetic_image(1_179_432);
    let file = encode(&claimed_for_b5, &B5_IMAGE_SHA256, &sample());
    let running = synthetic_image(1 << 16);
    let err = decode_v2(&file, &running, &B5).unwrap_err();
    let saved = match &err {
        SettingsError::Mismatch { saved, .. } => saved,
        other => panic!("the header names another image than this run's, not {other}"),
    };
    assert_eq!(saved.name, Some("7.00.00b5"));
    let said = err.to_string();
    let (saved_side, _) = said
        .split_once("; this run's is")
        .expect("the message names both images");
    assert!(saved_side.contains("7.00.00b5"), "{saved_side}");
    assert!(saved_side.contains("image 1179432 bytes"), "{saved_side}");
    assert!(
        saved_side.contains(&format!("sha256 {}", hex(&B5_IMAGE_SHA256[..6]))),
        "{saved_side}"
    );
}

#[test]
fn an_absurd_entry_count_is_corrupt_before_anything_is_reserved() {
    let image = synthetic_image(1 << 16);
    let err = decode_v2(&tamper(&image, 44, &u32::MAX.to_le_bytes()), &image, &B5).unwrap_err();
    assert!(
        matches!(&err, SettingsError::Corrupt(r) if r.contains("entry count")),
        "{err}"
    );
}

// The count is bounded by the rows the profile's regions hold: that many is a count the file can
// still fail on (it has no table), and one more is refused as a count.
#[test]
fn exactly_one_entry_more_than_the_rows_is_refused_as_a_count() {
    let image = synthetic_image(1 << 16);
    let rows = (B5.flash_size() / ROW) as u32;
    assert_eq!(rows, (USER_FLASH + BLOCK_SIZE as u32) / ROW as u32);

    let err = decode_v2(&no_table_file(&image, rows), &image, &B5).unwrap_err();
    assert!(
        matches!(&err, SettingsError::Corrupt(r) if r == "the file ends inside the entry table"),
        "as many entries as rows is within the bound: {err}"
    );
    let err = decode_v2(&no_table_file(&image, rows + 1), &image, &B5).unwrap_err();
    assert!(
        matches!(&err, SettingsError::Corrupt(r) if r.contains("entry count")),
        "one more than the rows: {err}"
    );
}

/// The same file, refused by the chip's own range instead of by the format: the entries and the
/// clock in it are sound, so this is the one refusal that used to find the machine already
/// written to. A file that is refused has nothing applied (docs/settings-format.md, "Reading
/// rules"), and the run that refused it saves nothing.
#[test]
fn a_file_the_chip_refuses_has_nothing_applied() {
    let image = synthetic_image(1 << 16);
    let addr = 0x0812_0000u32;
    let mut row = baseline_row(&image, addr);
    row[..4].copy_from_slice(&0xCA_FE_F0_0Du32.to_le_bytes());
    let mut state = [0u8; STATE_SIZE];
    state[..8].copy_from_slice(&i64::MAX.to_le_bytes()); // far beyond the 200 years the chip keeps
    state[8] = 1; // the flag that says the offset means a moment
    let settings = Settings {
        entries: vec![(addr, row)],
        clock: [0x12_30_45, 0x26_91_12],
        ds3231: Some(state),
    };
    let file = encode(&image, &sha256(&image), &settings);
    let row_words = |cpu: &mut Cpu| {
        (0..ROW as u32)
            .step_by(4)
            .map(|i| cpu.read32(addr + i))
            .collect::<Vec<_>>()
    };
    let kept = |cpu: &Cpu| {
        let memory = cpu.memory();
        (memory.rtc().calendar(), memory.ds3231().saved())
    };
    let mut cpu = machine(&image, &B5);
    let words = row_words(&mut cpu);
    let clock_and_chip = kept(&cpu);

    let read = decode_v2(&file, &image, &B5).expect("the file itself is a sound v2 file");
    let err = cpu.load_settings(&read).unwrap_err();

    assert!(
        err.to_string().contains("more than 200 years"),
        "the refusal is the chip's: {err}"
    );
    assert_eq!(
        row_words(&mut cpu),
        words,
        "a refused file wrote no row of the flash"
    );
    assert_eq!(
        kept(&cpu),
        clock_and_chip,
        "and left the clock and the chip as they were"
    );
}

/// A v2 file that promises `count` entries and holds none: the header, then today's tail of 8
/// clock bytes and the checksum, with a checksum that fits.
fn no_table_file(image: &[u8], count: u32) -> Vec<u8> {
    let file = encoded_sample(image);
    let tail_at = file.len() - CLOCK_SIZE - CRC_SIZE;
    let mut bytes = file[..HEADER_SIZE].to_vec();
    bytes.extend_from_slice(&file[tail_at..tail_at + CLOCK_SIZE]);
    bytes.extend_from_slice(&[0u8; CRC_SIZE]);
    bytes[44..48].copy_from_slice(&count.to_le_bytes());
    let crc_at = bytes.len() - CRC_SIZE;
    let crc = crc32(&bytes[..crc_at]);
    bytes[crc_at..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

// A table the file does not hold is the file ending inside the table. Reading on past the header
// takes the clock's bytes for an entry, and refuses an address the file never claimed.
#[test]
fn a_file_that_ends_inside_the_entry_table_says_so() {
    let image = synthetic_image(1 << 16);
    // One entry promised, and every row of this profile's regions promised at once.
    for count in [1u32, (B5.flash_size() / ROW) as u32] {
        let err = decode_v2(&no_table_file(&image, count), &image, &B5).unwrap_err();
        assert!(
            matches!(&err, SettingsError::Corrupt(reason)
                if reason == "the file ends inside the entry table"),
            "count {count}: {err}"
        );
    }
}

#[test]
fn truncation_at_every_boundary_is_corrupt() {
    let image = synthetic_image(1 << 16);
    let file = encoded_sample(&image);
    assert!(file.len() > HEADER_SIZE + 16);
    let boundaries = [
        HEADER_SIZE - 1,           // inside the header
        HEADER_SIZE + 3,           // inside an entry's address
        HEADER_SIZE + 12,          // inside the first entry's value
        file.len() - 3,            // inside the CRC
        file.len() - CRC_SIZE,     // no CRC at all
        file.len() - CRC_SIZE - 2, // half the tail, no CRC
    ];
    for cut in boundaries {
        let err = decode_v2(&file[..cut], &image, &B5).unwrap_err();
        assert!(
            matches!(&err, SettingsError::Corrupt(_)),
            "cut at {cut}: {err}"
        );
        assert!(
            err.to_string().contains("the settings file is corrupt ("),
            "the corrupt wording: {err}"
        );
        assert!(err.to_string().contains("nothing was loaded"), "{err}");
    }
}

#[test]
fn a_flipped_crc_is_corrupt() {
    let image = synthetic_image(1 << 16);
    let mut file = encoded_sample(&image);
    let last = file.len() - 1;
    file[last] ^= 0x80;
    let err = decode_v2(&file, &image, &B5).unwrap_err();
    assert!(
        matches!(&err, SettingsError::Corrupt(r) if r.contains("checksum")),
        "{err}"
    );
}

#[test]
fn the_row_rules_are_enforced() {
    type Reject = (&'static str, Vec<(u32, Vec<u8>)>);
    let image = synthetic_image(1 << 16);
    let rejects: [Reject; 9] = [
        (
            "a length that is not a row",
            vec![(0x0812_0000, vec![0u8; 31])],
        ),
        ("an empty entry", vec![(0x0812_0000, vec![])]),
        ("an address off the row", vec![(0x0812_0004, vec![0u8; 32])]),
        (
            "an entry outside a region",
            vec![(0x0811_0000, vec![0u8; 32])],
        ),
        (
            "an entry across two regions",
            vec![(0x080F_8540, vec![0u8; 0x20 + 0x1000])],
        ),
        (
            "two overlapping entries",
            vec![(0x0812_0000, vec![0u8; 64]), (0x0812_0020, vec![0u8; 32])],
        ),
        (
            "entries out of order",
            vec![(0x0812_0080, vec![0u8; 32]), (0x080F_8540, vec![0u8; 32])],
        ),
        (
            "a repeated address",
            vec![(0x0812_0000, vec![0u8; 32]), (0x0812_0000, vec![0u8; 32])],
        ),
        (
            "an entry whose bounds overflow",
            vec![(0xFFFF_FFF0, vec![0u8; 32])],
        ),
    ];
    for (why, entries) in rejects {
        let settings = Settings {
            entries,
            ..sample()
        };
        let bytes = encode(&image, &sha256(&image), &settings);
        let err = decode_v2(&bytes, &image, &B5).unwrap_err();
        assert!(
            matches!(&err, SettingsError::Corrupt(_)),
            "{why} must be refused: {err}"
        );
    }
}

#[test]
fn only_the_two_tails_today_are_read() {
    let image = synthetic_image(1 << 16);
    let file = encoded_sample(&image);
    let after_entries = file.len() - CLOCK_SIZE - CRC_SIZE;
    let (body, tail_and_crc) = file.split_at(after_entries);
    let (clock, crc) = tail_and_crc.split_at(CLOCK_SIZE);
    // The v1 DS3231 offset form (8 + 8) never appears in a v2 file, and no other size either:
    // the tail is the clock, or the clock and the DS3231's ten bytes.
    for (why, tail) in [
        ("no clock at all", Vec::new()),
        (
            "a clock and four stray bytes",
            [clock.to_vec(), vec![0u8; 4]].concat(),
        ),
        (
            "the v1 offset form",
            [clock.to_vec(), vec![0u8; 8]].concat(),
        ),
    ] {
        let mut file = body.to_vec();
        file.extend(&tail);
        file.extend(crc);
        let err = decode_v2(&file, &image, &B5).unwrap_err();
        assert!(
            matches!(&err, SettingsError::Corrupt(r) if r.contains("tail")),
            "{why}: {err}"
        );
    }
}

#[test]
fn a_v1_file_that_happens_to_start_with_the_magic_is_refused_and_left_alone() {
    // Four bytes of "CMM2" at the head of a region dump: `classify` sees the magic first, so
    // the file is refused and never read as v1 or migrated.
    let image = synthetic_image(1 << 16);
    let mut bytes = v1_file(&B5, &image, B5.regions, &clock_tail(), &[]);
    bytes[..4].copy_from_slice(b"CMM2");
    assert!(
        classify(&bytes, &B5).is_err(),
        "the magic wins over the size"
    );
    // And when the byte the dump happens to hold where the version sits is below 2, the file
    // is refused as corrupt: the header is not readable, and nothing of it is loaded.
    bytes[4] = 1;
    let err = classify(&bytes, &B5).unwrap_err();
    assert!(matches!(&err, SettingsError::Corrupt(_)), "{err}");
}

#[test]
fn the_corrupt_message_is_the_one_the_user_sees() {
    let said = SettingsError::Corrupt("the checksum does not match".into()).to_string();
    assert_eq!(
        said,
        "the settings file is corrupt (the checksum does not match); nothing was loaded."
    );
}

// The equivalence suite: the file changes, the flash does not.

#[test]
fn b5_at_its_real_length_migrates_word_for_word() {
    // The real b5 length: the image ends 216 bytes below 0x08120000, so every row of the user
    // flash region reads FF, inside the file or out of it.
    let image = synthetic_image(1_179_432);
    let v1 = v1_file(&B5, &image, B5.regions, &clock_tail(), &[]);
    let settings = assert_migrates_word_for_word(&image, &B5, &v1);
    // The block is inside the image, and its baseline is the image's own bytes: untouched, it
    // needs no entry, and the clock came across.
    assert!(
        settings.entries.is_empty(),
        "an untouched b5 file migrates to no entries"
    );
    assert_eq!(settings.clock, [0x12_30_45, 0x26_91_12]);
}

#[test]
fn changed_rows_inside_the_image_become_entries_and_equal_ones_do_not() {
    // An image that reaches into the user flash region: the rows at 0x08120000 and
    // 0x08120020 are wholly inside it, and the row at 0x08120040 straddles the end.
    let image = synthetic_image(0x12_0048);
    let mut one_row = baseline_row(&image, 0x0812_0020);
    one_row[4] ^= 0xFF; // one byte of the row differs
    let wide_row = vec![0x5Au8; ROW]; // a row wholly past the end, differing everywhere
    let v1 = v1_file(
        &B5,
        &image,
        B5.regions,
        &clock_tail(),
        &[
            // The row at 0x08120000 is left exactly as the baseline has it: no entry.
            (0x0812_0020, &one_row),
            (0x0812_0080, &wide_row),
        ],
    );
    let settings = assert_migrates_word_for_word(&image, &B5, &v1);
    assert_eq!(
        settings.entries,
        vec![(0x0812_0020, one_row), (0x0812_0080, wide_row)],
        "one changed byte in a row makes that row's entry, and only that row's"
    );
}

#[test]
fn contiguous_changed_rows_merge_into_one_entry() {
    // The row is the unit of the diff, but neighbours travel as one entry (section 1 of
    // the design): the file holds the bytes, and where they stop is the entry's length.
    let image = synthetic_image(0x12_0048);
    let row_of = |at: u32, byte: u8| {
        let mut row = baseline_row(&image, at);
        row[0] = byte; // one changed byte is enough to make the row a changed one
        row
    };
    let first = row_of(0x0812_0020, 0x07);
    let second = row_of(0x0812_0040, 0x09); // the row after it, straddling the image end
    let mut merged = first.clone();
    merged.extend(&second);
    let v1 = v1_file(
        &B5,
        &image,
        B5.regions,
        &clock_tail(),
        &[(0x0812_0020, &first), (0x0812_0040, &second)],
    );
    let settings = assert_migrates_word_for_word(&image, &B5, &v1);
    assert_eq!(settings.entries, vec![(0x0812_0020, merged)]);
}

#[test]
fn a_row_that_straddles_the_image_end_follows_the_baseline_rule() {
    // The image ends 8 bytes into the row at 0x08120000: the baseline is the image's 8 bytes
    // then FF. The v1 file with exactly those bytes gives no entry; one byte off anywhere in
    // the row, and the whole row of 32 is an entry.
    let image = synthetic_image(0x12_0008);
    let same = v1_file(&B5, &image, B5.regions, &clock_tail(), &[]);
    let settings = assert_migrates_word_for_word(&image, &B5, &same);
    assert!(
        settings.entries.is_empty(),
        "the straddling row equals the baseline"
    );

    let mut off = v1_file(&B5, &image, B5.regions, &clock_tail(), &[]);
    let last_of_row = BLOCK_SIZE + ROW - 1; // the row's last byte, in its FF half
    off[last_of_row] ^= 0x01;
    let settings = assert_migrates_word_for_word(&image, &B5, &off);
    let mut want = baseline_row(&image, 0x0812_0000);
    want[ROW - 1] ^= 0x01;
    assert_eq!(
        settings.entries,
        vec![(0x0812_0000, want)],
        "a byte off in the FF half makes the whole row an entry"
    );
}

#[test]
fn rc6_options_inside_its_image_save_only_the_changed_rows() {
    // RC6 keeps its options in the first 4 KB of the sector at 0x08140000, wholly inside its
    // image; the program flash from 0x08160000 is past the end, as b5's user flash is.
    let image = synthetic_image(0x15_8220);
    let options_row = 0x0814_0000u32;
    let mut row = baseline_row(&image, options_row);
    row[..4].copy_from_slice(&0xDE_AD_BE_EFu32.to_le_bytes());
    let v1 = v1_file(
        &RC6,
        &image,
        RC6.regions,
        &clock_tail(),
        &[(options_row, &row)],
    );
    let settings = assert_migrates_word_for_word(&image, &RC6, &v1);
    assert_eq!(
        settings.entries,
        vec![(options_row, row)],
        "the options rows equal to the baseline are not saved"
    );
}

#[test]
fn every_v1_size_form_loads_word_for_word() {
    // The nine forms of the probe above, each against the image of its own profile at its real
    // length, with one changed row inside the user flash or the program flash.
    for (profile, len, changed_row) in [
        (&B5, 1_179_432usize, 0x0812_0000u32),
        (&RC6, 0x15_8220, 0x0816_0000),
    ] {
        let image = synthetic_image(len);
        let block = profile.regions[0].addr;
        let mut row = baseline_row(&image, changed_row);
        row[..4].copy_from_slice(&0xCA_FE_F0_0Du32.to_le_bytes());
        for (what, regions, tail) in v1_forms(profile) {
            // The last row of every region the file holds, changed as much as the row above.
            // These are the bytes at the end of the user's flash, and a walk that stops one row
            // short of a region's end loses exactly them: nothing else
            // in this sweep changes a region's final row, so the machine comparison below would
            // stay green without this patch. A single-row region (the 32-byte block) is its own
            // first row, and the block is what the assertions under this one are about.
            let last_rows: Vec<(u32, Vec<u8>)> = regions
                .iter()
                .filter(|region| region.len > ROW as u32)
                .map(|region| {
                    let at = region.addr + region.len - ROW as u32;
                    let mut value = baseline_row(&image, at);
                    value[..4].copy_from_slice(&0x1F_1F_1F_FEu32.to_le_bytes());
                    (at, value)
                })
                .collect();
            let mut patch: Vec<(u32, &[u8])> = vec![(changed_row, row.as_slice())];
            patch.extend(last_rows.iter().map(|(at, value)| (*at, value.as_slice())));
            let v1 = v1_file(profile, &image, regions, &tail, &patch);
            let settings = assert_migrates_word_for_word(&image, profile, &v1);
            assert!(
                settings
                    .entries
                    .iter()
                    .any(|(at, value)| *at == changed_row && *value == row),
                "{what}: the changed row must read across"
            );
            for (at, value) in &last_rows {
                // Carried as part of an entry that covers it, with these bytes at this address:
                // a run merge may start the entry before it, which is still the same row kept.
                assert!(
                    settings.entries.iter().any(|(start, bytes)| *start <= *at
                        && *at + ROW as u32 <= *start + bytes.len() as u32
                        && bytes[(*at - *start) as usize..][..ROW] == value[..]),
                    "{what}: the last row of the region at {at:#010X} must read across"
                );
            }
            let block_entry = settings
                .entries
                .iter()
                .find(|(at, _)| *at == block)
                .map(|(_, value)| value.clone());
            if regions.len() < profile.regions.len() {
                // The old form lacks the block: the loader's zeros must come across as an entry.
                assert_eq!(
                    block_entry,
                    Some(vec![0u8; BLOCK_SIZE]),
                    "{what}: the block a file lacks must be zeroed, not left FF"
                );
            } else {
                assert_eq!(
                    block_entry, None,
                    "{what}: a block left as the image has it needs no entry"
                );
            }
            // The tail carries across in every form (the helper compares the clock's bytes
            // against the file's, and both machines' clock and chip state). What is left to
            // say here is what each form's chip half must become.
            if tail.len() == CLOCK_SIZE {
                assert_eq!(
                    settings.ds3231, None,
                    "{what}: a chip as it was made earns no state bytes"
                );
            } else if tail.len() == CLOCK_SIZE + 8 {
                // The offset form: converted once into the chip's own state - the moment
                // it meant is compared between the machines, by the helper.
                let kept = settings.ds3231.unwrap_or_else(|| {
                    panic!("{what}: the offset form must become the chip's state")
                });
                assert_eq!(kept[8] & 1, 1, "{what}: the state says the time was set");
            } else {
                let from_file: [u8; STATE_SIZE] = tail[tail.len() - STATE_SIZE..]
                    .try_into()
                    .expect("the state's size");
                assert_eq!(
                    settings.ds3231,
                    Some(from_file),
                    "{what}: the chip's state bytes cross byte-identical"
                );
            }
        }
    }
}

#[test]
fn the_old_form_of_a_b5_file_zeroes_the_block_it_lacks() {
    // The b5 loader's zero-fill has to survive the migration: a file from before the block
    // was kept leaves zeros in it, not FF.
    let image = synthetic_image(1_179_432);
    let old = v1_file(&B5, &image, &B5.regions[1..], &clock_tail(), &[]);
    let settings = assert_migrates_word_for_word(&image, &B5, &old);
    let block = settings
        .entries
        .iter()
        .find(|(at, _)| *at == B5.regions[0].addr)
        .expect("the block the old file lacks must be an entry of zeros");
    assert_eq!(block.1, vec![0u8; BLOCK_SIZE]);
}

#[test]
fn save_settings_twice_gives_the_same_bytes() {
    let image = synthetic_image(0x12_0048);
    let v1 = v1_file(
        &B5,
        &image,
        B5.regions,
        &clock_tail(),
        &[(0x0812_0000, &[0x01; 32])],
    );
    let mut cpu = machine(&image, &B5);
    cpu.load_persistent(&v1).unwrap();
    let once = cpu.save_settings().unwrap();
    let twice = cpu.save_settings().unwrap();
    assert_eq!(once, twice);
    // And what it saved, it can read back.
    assert_eq!(
        decode_v2(&once, &image, &B5).unwrap().entries,
        vec![(0x0812_0000, vec![0x01u8; 32])]
    );
}

#[test]
fn a_raw_file_of_the_other_release_size_is_the_other_release_not_corrupt() {
    // "Corrupt" may only fire after every legitimate branch is excluded (docs/settings-format.md,
    // reading rule 7).
    let rc6_size = RC6.flash_size() + CLOCK_SIZE;
    assert!(
        matches!(
            classify(&vec![0xFF; rc6_size], &B5),
            Ok(FileKind::OtherRelease(p)) if p.key == "rc6"
        ),
        "an RC6-size file must offer the other-release message, not a corrupt one"
    );
}
