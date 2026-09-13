//! Compile the macOS-only Objective-C shims this crate links against.
//!
//! Two pieces of Orbiter cannot be written in Rust: talking to the system's own authentication
//! support, and storing a signing key in the login Keychain. Both are small Objective-C files
//! compiled here and linked into the crate.
//!
//! On any other platform this does nothing, and the Rust side degrades to "not supported here"
//! rather than failing to build.

/// Compile and link the Objective-C shims, on macOS only.
///
/// Declares both sources as build inputs so editing either triggers a rebuild, compiles them with
/// ARC, and links the two system frameworks they use. Off macOS it emits only the rerun
/// declarations and compiles nothing.
///
/// # Panics
///
/// `cc::Build::compile` aborts the build if the toolchain is missing or a source fails to
/// compile. That is the correct outcome: a crate that silently linked no shim would fail later,
/// at runtime, on a user's machine.
fn main() {
    println!("cargo:rerun-if-changed=src/local_anisette.m");
    println!("cargo:rerun-if-changed=src/keychain.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/local_anisette.m")
            .file("src/keychain.m")
            .flag("-fobjc-arc")
            .compile("orbiter_local_anisette");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Security");
    }
}
