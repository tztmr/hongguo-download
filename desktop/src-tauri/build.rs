fn main() {
    tauri_build::build();

    #[cfg(windows)]
    {
        let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
        let resource = std::path::Path::new(&out_dir).join("libresource.a");
        println!("cargo:rustc-link-arg={}", resource.display());
    }
}
