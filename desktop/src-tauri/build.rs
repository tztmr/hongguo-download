fn main() {
    tauri_build::build();

    #[cfg(windows)]
    {
        let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
        // tauri-winres links binaries by default. Library tests also need the
        // Common Controls manifest, using the artifact for this target ABI.
        let resource_name = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => "resource.lib",
            _ => "libresource.a",
        };
        let resource = std::path::Path::new(&out_dir).join(resource_name);
        println!("cargo:rustc-link-arg={}", resource.display());
    }
}
