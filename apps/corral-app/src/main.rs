//! Runs the Rust application directly on the machine's native backend.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

/// The app's root. Windows launches the binary directly, without `nsh run` reading `nsh.json`'s
/// `"theme"`, so the root carries the embedded `theme.json` itself; without it every custom
/// token (`ui::tokens::color`) falls back to the foreground and text vanishes into its fill.
/// The app id names its support directory.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn root() -> Result<platform::Root, String> {
    let root = platform::Root::of::<nsh_app_corral::App>();
    #[cfg(target_os = "windows")]
    let root = root.app_id("com.github.pawissanutt.corral").tokens(
        nsh_base::tokens_of(Some(include_str!("../theme.json")))
            .map_err(|error| format!("the embedded theme.json is unreadable: {error}"))?,
    );
    Ok(root)
}

/// Where the installed app's `asset:/` files are: `assets\` beside the executable (the app's own
/// under `assets\@nsh-app-corral\`), whatever the working directory or the launching shortcut.
#[cfg(target_os = "windows")]
fn asset_base() -> Result<platform::AssetBase, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot locate the app's executable: {error}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "the app's executable has no directory".to_owned())?;
    Ok(platform::AssetBase::Directory(dir.join("assets")))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> Result<std::process::ExitCode, String> {
    use std::process::Termination;

    env_logger::init();
    Ok(platform::run_app(
        platform::AppSource::Rust(root()?),
        platform::AppOptions {
            title: "Corral".to_owned(),
            #[cfg(target_os = "linux")]
            app_id: "com.example.corral".to_owned(),
            #[cfg(target_os = "windows")]
            app_id: "com.github.pawissanutt.corral".to_owned(),
            #[cfg(target_os = "windows")]
            asset_base: Some(asset_base()?),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            single_instance: true,
            size: (480, 640),
            ..Default::default()
        },
    )
    .report())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
