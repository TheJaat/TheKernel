//! Tells the linker to use linker.ld.
//!
//! A build script rather than a flag in .cargo/config.toml because the
//! path has to be absolute: the linker does not run from this directory.

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    // One script per architecture: where the kernel is loaded, and
    // what a loader expects to find at its start, differ.
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("cargo sets CARGO_CFG_TARGET_ARCH");
    let script = if arch == "aarch64" { "linker-aarch64.ld" } else { "linker.ld" };
    println!("cargo:rustc-link-arg-bins=-T{dir}/{script}");
    println!("cargo:rerun-if-changed={script}");

    // For the `big-image` feature: a quarter of a megabyte that
    // cannot be compressed away or mistaken for zeroes, and that the
    // kernel can check byte by byte once it runs. The rule for byte `i`
    // is repeated in main.rs.
    if std::env::var_os("CARGO_FEATURE_BIG_IMAGE").is_some() {
        let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
        let ballast: Vec<u8> = (0..256 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        std::fs::write(format!("{out}/ballast.bin"), ballast).expect("writing the ballast");
    }
}
