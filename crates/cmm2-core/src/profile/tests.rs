use super::*;

// The numbers here repeat the table's on purpose: the ignored real-firmware tests check the
// table's against the real images, and these check that the code does what the table says.

fn regions(profile: &Profile) -> Vec<(u32, u32)> {
    profile.regions.iter().map(|r| (r.addr, r.len)).collect()
}

#[test]
fn b5_keeps_its_own_block_and_the_user_flash() {
    assert_eq!(regions(&B5), [(0x080F_8540, 0x20), (0x0812_0000, 0xE_0000)]);
    // 32 + 917,504 for the regions, then the clock.
    assert_eq!(B5.flash_size(), 917_536);
    assert_eq!(B5.persistent_size(), 917_544);
    assert!(B5.first_is_new);
}

#[test]
fn rc6_keeps_its_block_its_options_and_its_program_flash() {
    assert_eq!(
        regions(&RC6),
        [
            (0x080F_6F00, 0x20),
            (0x0814_0000, 0x1000),
            (0x0816_0000, 0xA_0000),
        ]
    );
    assert_eq!(RC6.flash_size(), 32 + 4096 + 655_360);
    assert_eq!(RC6.persistent_size(), 659_496);
}

#[test]
fn rc6_does_not_keep_the_firmware_code_between_its_options_and_its_program() {
    // The options are the first 4 KB of the sector at 0x0814_0000; the rest of it is code.
    let covers = |addr: u32| {
        RC6.regions
            .iter()
            .any(|r| (r.addr..r.addr + r.len).contains(&addr))
    };
    assert!(covers(0x0814_0000) && covers(0x0814_0FFF));
    assert!(!covers(0x0814_1000) && !covers(0x0815_8219));
    assert!(covers(0x0816_0000));
}

#[test]
fn a_release_is_named_only_when_its_length_and_hash_both_agree() {
    assert_eq!(
        Profile::named_release(1_179_432, &B5_IMAGE_SHA256),
        Some("7.00.00b5")
    );
    assert_eq!(
        Profile::named_release(1_409_568, &RC6_IMAGE_SHA256),
        Some("7.00.00RC6")
    );
    // Either half on its own names nothing: the length is a claim, and so is a stranger's hash.
    assert_eq!(Profile::named_release(1_409_568, &B5_IMAGE_SHA256), None);
    assert_eq!(Profile::named_release(1_179_432, &RC6_IMAGE_SHA256), None);
    assert_eq!(Profile::named_release(1_179_432, &[0u8; 32]), None);
    assert_eq!(Profile::named_release(0, &B5_IMAGE_SHA256), None);
    assert_eq!(Profile::named_release(0xFFFF_FFFF, &[0u8; 32]), None);
}

// The bound a settings file is refused against before it is read. The numbers repeat the
// table's on purpose, as everywhere else in these tests.
#[test]
fn the_largest_settings_file_is_one_no_legitimate_file_exceeds() {
    // b5 with every row of its regions changed: the header, an entry of 8 bytes for each of
    // its 28,673 rows, those rows' 917,536 bytes, the longest tail (the clock's 8 bytes and
    // the DS3231's 10) and the checksum.
    assert_eq!(
        Profile::largest_settings_file(),
        48 + 28_673 * 8 + 917_536 + 18 + 4
    );
    assert_eq!(Profile::largest_settings_file(), 1_146_990);
    // No raw file either release ever wrote, with any of its tails, is above it: the other
    // release's file has to be read to be named.
    for profile in [&B5, &RC6] {
        for tail in [CLOCK_SIZE, VERSION_1_SIZE, WITH_DS3231_SIZE] {
            let v1 = profile.flash_size() + tail;
            assert!(
                profile.regions_for(v1).is_some() && v1 <= Profile::largest_settings_file(),
                "{v1} is a file {} writes",
                profile.name
            );
        }
    }
}

#[test]
fn only_b5_has_settings_files_from_before_its_block_was_kept() {
    assert_eq!(B5.old_regions().unwrap(), &B5.regions[1..]);
    assert_eq!(RC6.old_regions(), None);
}

#[test]
fn an_unknown_image_has_no_profile() {
    assert_eq!(Profile::identify(&[]), None);
    assert_eq!(Profile::identify(&[0; 1024]), None);
    // The right length is not enough.
    assert_eq!(Profile::identify(&vec![0; 1_179_432]), None);
    assert_eq!(Profile::identify(&vec![0; 1_409_568]), None);
}

#[test]
fn a_profile_is_found_by_name() {
    assert_eq!(Profile::named("b5"), Some(&B5));
    assert_eq!(Profile::named("RC6"), Some(&RC6));
    assert_eq!(Profile::named("8"), None);
}
