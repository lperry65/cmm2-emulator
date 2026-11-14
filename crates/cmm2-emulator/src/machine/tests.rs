use super::*;

use crate::args::RunOptions;
use crate::test_support::scratch;

#[test]
fn the_ds3231_is_fitted_and_shows_the_hosts_time() {
    let dir = scratch("ds3231-fitted");
    let firmware = dir.join("firmware.bin");
    let mut image = vec![0; 64];
    image[..4].copy_from_slice(&0x2000_8000u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x0800_0021u32.to_le_bytes());
    std::fs::write(&firmware, image).unwrap();

    let machine = build_machine(firmware.to_str().unwrap(), &RunOptions::default()).unwrap();

    // The chip's own default clock stands at the year 2000, until the host's is attached.
    let shown = machine.cpu.memory().ds3231().time();
    assert_ne!(shown.year, 2000);
    std::fs::remove_dir_all(&dir).unwrap();
}
