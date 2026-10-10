//! Sets the configuration flags of the target: the dynamic recompiler's
//! hosts and OpenXR's.

fn main() {
    // The hosts the dynamic recompiler (src/dynrec) generates code for.
    println!("cargo::rustc-check-cfg=cfg(dynrec)");
    if matches!(std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("x86_64" | "aarch64")) {
        println!("cargo::rustc-cfg=dynrec");
    }
    // VR headsets through OpenXR, which the `vr` feature has where there
    // are OpenXR runtimes to talk to: Linux and Windows.
    println!("cargo::rustc-check-cfg=cfg(xr)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if std::env::var_os("CARGO_FEATURE_VR").is_some() && matches!(os.as_str(), "linux" | "windows") {
        println!("cargo::rustc-cfg=xr");
    }
    // The relay program uses nothing of SDL, which the library links. The
    // GNU linker leaves such a library out by itself; macOS's keeps it
    // unless told otherwise, and the relay would want SDL2 to start.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo::rustc-link-arg-bin=rust-dos-relay=-Wl,-dead_strip_dylibs");
    }
}
