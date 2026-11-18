mod common;

use common::emulator;

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_and_traces_peripheral_accesses() {
    let path = std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
    let output = emulator()
        .args(["--firmware", &path, "--run", "100000", "--trace-mmio"])
        .output()
        .expect("failed to run cmm2-emulator");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\nstopped: instruction limit\n"),
        "{stdout}"
    );
    assert!(stdout.contains("\ninstructions 100000\n"), "{stdout}");
    assert!(stdout.contains(" RCC_CR "), "{stdout}");
}

// About 196.5 million instructions to the first prompt and about 20 million more to answer it.
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_asks_for_the_keyboard_and_takes_the_answer() {
    let path = std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
    let output = emulator()
        .args(["--firmware", &path, "--console"])
        .args(["--on", "Keyboard type", "1", "--run", "260000000"])
        .output()
        .expect("failed to run cmm2-emulator");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Keyboard type"), "{stdout}");
    assert!(stdout.contains("Enter time (24hr clock)"), "{stdout}");
}

// What `PRINT RND` prints at the prompt of a first boot, for a run with the given RNG seed
// option (none for the host's entropy). MMBasic's RND reads the hardware RNG, so RANDOMIZE does
// not fix it; the seed does. The run takes about 450 million instructions to the prompt.
fn rnd_with(seed: Option<&str>) -> String {
    let path = std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
    let mut command = emulator();
    command
        .args(["--firmware", &path, "--console"])
        .args(["--on", "Keyboard type", "1"])
        .args(["--on", "HH:MM:SS", "12:00:00\\r"])
        .args(["--on", "DD/MM/YY", "12/11/26\\r"])
        // The banner of the second boot ends in a prompt; the first boot's does not.
        .args(["--on", "Allardice\r\n\r\n>", "PRINT RND\\r"])
        .args(["--run", "700000000"]);
    if let Some(seed) = seed {
        command.args(["--rng-seed", seed]);
    }
    let output = command.output().expect("failed to run cmm2-emulator");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let after = stdout
        .split("PRINT RND\r\n")
        .nth(1)
        .unwrap_or_else(|| panic!("no answer in {stdout}"));
    after.split("\r\n").next().unwrap().to_string()
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_rnd_repeats_with_the_same_rng_seed_and_differs_with_another() {
    let first = rnd_with(Some("1"));
    assert!(first.trim().starts_with("0."), "{first:?}");
    assert_eq!(rnd_with(Some("1")), first);
    assert_ne!(rnd_with(Some("2")), first);
}
