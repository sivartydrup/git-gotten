fn main() {
    tauri_build::build();

    // Tauri links its Windows resources (including the Common Controls v6
    // manifest) into application binaries, but not integration test binaries.
    // Wry imports TaskDialogIndirect, which is absent from the default v5 DLL.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let resource = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            "resource.lib"
        } else {
            "libresource.a"
        };
        let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(resource);
        println!("cargo:rustc-link-arg-tests={}", path.display());
    }
}
