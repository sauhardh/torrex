use std::env;
use std::path::PathBuf;

/// Returns the download directory based on OS.
pub fn config_dir(appname: &str) -> PathBuf {
    let base_dir = get_config_dir().expect("Could not determine config directory");
    base_dir.join(appname)
}

#[allow(dead_code)]
/// Returns the download directory of the Linux OS.
///
/// Example:
#[cfg(target_os = "linux")]
fn get_config_dir() -> Option<PathBuf> {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(xdg))
    } else if let Ok(home) = env::var("HOME") {
        Some(PathBuf::from(home).join(".config"))
    } else {
        None
    }
}

/// Returns the download directory of the Window OS.
///
/// Example: C:\Users\<username>\AppData\Roaming\<appname>\
#[cfg(target_os = "windows")]
fn get_config_dir() -> Option<PathBuf> {
    env::var("APPDATA").ok().map(PathBuf::from)
}

/// Returns the download directory of the MacOS.
///
/// Example: /Users/<username>/Library/Application Support/<appname>/
#[cfg(target_os = "macos")]
fn get_config_dir() -> Option<PathBuf> {
    env::var("HOME").ok().map(|home| {
        PathBuf::from(home)
            .join("Library")
            .joi("Application Support")
    })
}
