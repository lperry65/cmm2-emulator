use std::process::Command;

#[test]
fn version_flag_prints_name_and_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .arg("--version")
        .output()
        .expect("failed to run cmm2-emulator");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!("cmm2-emulator ", env!("CARGO_PKG_VERSION"), "\n")
    );
}

#[test]
fn unknown_argument_is_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .arg("--bogus")
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cmm2-emulator: unknown argument '--bogus'\nusage: cmm2-emulator --version\n"
    );
}

#[test]
fn no_arguments_prints_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_cmm2-emulator"))
        .output()
        .expect("failed to run cmm2-emulator");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "usage: cmm2-emulator --version\n"
    );
}
