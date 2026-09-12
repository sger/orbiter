fn main() {
    println!("cargo:rerun-if-changed=src/local_anisette.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/local_anisette.m")
            .flag("-fobjc-arc")
            .compile("orbiter_local_anisette");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
