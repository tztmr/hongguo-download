fn main() {
    tauri_build::build();

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-env-changed=HONGGUO_WINDOWS_TEST_RESOURCES");
        if std::env::var("HONGGUO_WINDOWS_TEST_RESOURCES").as_deref() != Ok("1") {
            return;
        }
        let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
        // Opt in only for `cargo test --lib`: tauri-winres already links the
        // app binary, so adding this resource there would duplicate it on MSVC.
        // Library test harnesses need the same Common Controls manifest.
        let resource_name = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => "resource.lib",
            _ => "libresource.a",
        };
        let resource = std::path::Path::new(&out_dir).join(resource_name);
        println!("cargo:rustc-link-arg={}", resource.display());
    }
}
