mod common;

use common::*;

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

// One run of the firmware on a settings file, to the prompt and `typed` there. A first boot asks
// its setup questions (the answers are ignored when it is not asked them); the banner of a boot
// that has been set up already ends in a prompt, and that is where the text is typed. The run
// stops on its instruction limit, which saves the file. Returns what the console showed.
fn run_on_settings(firmware: &str, flash: &std::path::Path, typed: &str) -> String {
    let output = emulator()
        // The layout is the firmware's own, never the one a variable names.
        .env_remove(LAYOUT_VARIABLE)
        .args(["--firmware", firmware, "--console"])
        .args(["--flash", flash.to_str().unwrap()])
        .args(["--on", "Keyboard type", "1"])
        .args(["--on", "HH:MM:SS", "12:00:00\\r"])
        .args(["--on", "DD/MM/YY", "12/11/26\\r"])
        .args(["--on", "Allardice\r\n\r\n>", typed])
        .args(["--run", "900000000"])
        .output()
        .expect("failed to run cmm2-emulator");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// The first run with a new settings file sets the function key; the second run with the same
// file must not ask the setup questions and must still have the key, which the firmware keeps in
// flash. Part of that is the block inside its own image, which a first start would erase.
fn settings_keep_the_function_key(variable: &str, name: &str, size: usize) {
    let firmware = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must be set"));
    let flash = flash_path(name);
    let _ = std::fs::remove_file(&flash);

    let first = run_on_settings(&firmware, &flash, "OPTION F1 \"hello\"\\r");
    assert!(first.contains("Keyboard type"), "{first}");

    let second = run_on_settings(&firmware, &flash, "OPTION LIST\\r");
    let saved = std::fs::read(&flash).unwrap().len();
    std::fs::remove_file(&flash).unwrap();
    assert!(!second.contains("Keyboard type"), "{second}");
    assert!(!second.contains("Enter time"), "{second}");
    let listed = second.split("OPTION LIST\r\n").nth(1).unwrap_or("");
    assert!(
        listed.contains("hello"),
        "OPTION LIST did not show F1: {second}"
    );
    assert_eq!(saved, size);
}

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_b5_keeps_its_settings_in_the_flash_file() {
    settings_keep_the_function_key("CMM2_FIRMWARE", "real-b5-settings", 917_544);
}

#[test]
#[ignore = "needs the RC6 firmware: CMM2_FIRMWARE_RC6=<path> cargo test --release -- --ignored"]
fn real_firmware_rc6_keeps_its_settings_in_the_flash_file() {
    settings_keep_the_function_key("CMM2_FIRMWARE_RC6", "real-rc6-settings", 659_496);
}

// A b5 settings file from before the 32-byte block was kept is the new file without its first 32
// bytes. The firmware must still find its function key: the loader gives it a block of zeros,
// which is what the firmware writes there itself, and not FF, which sends it down the first-start
// path. (A file from a run that stopped before the firmware wrote the block has no block to
// find, and would skip the first-start setup: I accept that.)
#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test --release -- --ignored"]
fn real_firmware_b5_keeps_the_function_key_from_an_old_form_settings_file() {
    let firmware = std::env::var("CMM2_FIRMWARE").expect("CMM2_FIRMWARE must be set");
    let flash = flash_path("real-b5-old-form");
    let _ = std::fs::remove_file(&flash);
    run_on_settings(&firmware, &flash, "OPTION F1 \"hello\"\\r");
    let new_form = std::fs::read(&flash).unwrap();
    std::fs::write(&flash, &new_form[32..]).unwrap();

    let second = run_on_settings(&firmware, &flash, "OPTION LIST\\r");
    std::fs::remove_file(&flash).unwrap();

    assert!(!second.contains("Keyboard type"), "{second}");
    let listed = second.split("OPTION LIST\r\n").nth(1).unwrap_or("");
    assert!(
        listed.contains("hello"),
        "OPTION LIST did not show F1: {second}"
    );
}
