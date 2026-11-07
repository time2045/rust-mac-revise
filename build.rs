fn main() {
    let icon = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR missing"))
        .join("icon.ico");
    std::fs::copy("icons/icon.ico", &icon).expect("cannot copy application icon");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    let windows = tauri_build::WindowsAttributes::new().window_icon_path(icon);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("cannot build Windows application resources");
}
