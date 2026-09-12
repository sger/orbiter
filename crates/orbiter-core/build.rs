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
