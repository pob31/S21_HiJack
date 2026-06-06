//! Build script: embed a Windows icon + version-info resource into the `.exe`.
//!
//! This runs only when building *for* Windows (guarded on the target OS), and
//! the `winresource` build-dependency is itself Windows-host-only (see the
//! `[target.'cfg(windows)'.build-dependencies]` table in Cargo.toml), so the
//! Linux/macOS release builds never compile or invoke it.
//!
//! The embedded resource gives the installed `s21_hijack.exe`:
//!   * its taskbar / Explorer icon,
//!   * the icon the `.s21show` file association points at
//!     (`DefaultIcon = s21_hijack.exe,0` in assets/s21_hijack_assoc.iss), and
//!   * FileVersion / ProductVersion in the Properties → Details tab,
//! matching the `CARGO_PKG_VERSION` the in-app update check compares on GitHub.

fn main() {
    // Re-run if the icon changes. The version is read from Cargo.toml, which
    // Cargo already tracks, so no extra rerun directive is needed for it.
    println!("cargo:rerun-if-changed=assets/icon.ico");

    #[cfg(windows)]
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    // Only embed when the *target* is Windows. On a Windows host cross-building
    // for another OS there is no Windows resource to emit (and no target
    // resource compiler), so skip cleanly.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "S21 HiJack");
    res.set(
        "FileDescription",
        "S21 HiJack - DiGiCo S21/S31 snapshot manager",
    );
    res.set("CompanyName", "pob31");
    res.set("LegalCopyright", "Licensed under MIT OR Apache-2.0");
    // winresource derives FileVersion / ProductVersion from CARGO_PKG_VERSION.

    if let Err(e) = res.compile() {
        // Embedding the icon/version is cosmetic — never fail the whole build
        // over it. Warn so the cause is visible, and ship an icon-less exe.
        println!("cargo:warning=failed to embed Windows resources: {e}");
    }
}
