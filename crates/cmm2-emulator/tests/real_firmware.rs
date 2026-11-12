use std::process::Command;

#[test]
#[ignore = "needs the real firmware: CMM2_FIRMWARE=<path> cargo test -- --ignored"]
fn real_firmware_runs_and_traces_peripheral_accesses() {
    let path = std::env::var("CMM2_FIRMWARE")
        .expect("CMM2_FIRMWARE must be set to the path of CMM2_MMBasic_7.00.00b5.bin");
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
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
