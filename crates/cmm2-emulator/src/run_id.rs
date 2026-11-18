//! What is not the same in two runs of the tests: the process id alone is not, for runs that are
//! started at once in their own PID namespaces, where it is a small number that repeats. The time
//! and an address (which ASLR moves) are put with it. This file is used by the unit tests and, by
//! its path, by the integration tests (`tests/common/mod.rs`), so that there is one of it.

pub fn run_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let here = &nanos as *const u128 as usize;
        format!(
            "{}-{:x}-{:x}",
            std::process::id(),
            nanos & 0xFFFF_FFFF_FFFF,
            here & 0xFFFF_FFFF
        )
    })
}
