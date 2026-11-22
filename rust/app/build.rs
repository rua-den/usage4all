//! Embeds the icon + version info (rc.exe from the Windows SDK) into the exe.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("FileDescription", "Claude usage");
        res.set("ProductName", "claude-usage");
        if let Err(e) = res.compile() {
            println!("cargo:warning=resource embedding skipped: {e}");
        }
    }
}
