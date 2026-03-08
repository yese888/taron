fn main() {
    // Only auto-install on Linux
    #[cfg(target_os = "linux")]
    {
        let header = std::path::Path::new("/usr/include/rocksdb/c.h");
        if !header.exists() {
            eprintln!("[ taron-core build.rs ] librocksdb-dev not found — installing automatically...");

            let status = std::process::Command::new("sh")
                .args([
                    "-c",
                    "apt-get update -qq && apt-get install -y --no-install-recommends \
                     librocksdb-dev libzstd-dev clang libclang-dev",
                ])
                .status();

            match status {
                Ok(s) if s.success() => {
                    eprintln!("[ taron-core build.rs ] librocksdb-dev installed successfully.");
                }
                Ok(s) => {
                    eprintln!(
                        "[ taron-core build.rs ] apt-get exited with status {}. \
                         If the build fails, run manually:\n  \
                         sudo apt-get install -y librocksdb-dev libzstd-dev clang libclang-dev",
                        s
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[ taron-core build.rs ] Could not run apt-get: {}. \
                         Run manually:\n  \
                         sudo apt-get install -y librocksdb-dev libzstd-dev clang libclang-dev",
                        e
                    );
                }
            }
        }
    }
}
