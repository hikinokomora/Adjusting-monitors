fn main() {
    // Icon and version info for the .exe shown in Explorer and the taskbar.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Выравнивание мониторов");
        res.set("FileDescription", "Выравнивание мониторов");
        res.set("LegalCopyright", "© 2026 hikinokomora");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
