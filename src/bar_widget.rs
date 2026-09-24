//! The bar widget, offered once on the first start.
//!
//! The package installs the widget to `/usr/share`, but the Omarchy shell only
//! loads plugins from `~/.config/omarchy/plugins`, and a package has no
//! business writing in a home directory. So the app asks, and on a yes links
//! the widget there and puts it on the right of the bar.

use std::path::PathBuf;
use std::process::Command;

use gtk::glib;

use crate::settings;

const ID: &str = "jankeesvw.meeting-recorder";
const SOURCE: &str = "/usr/share/omarchy-meeting-recorder/plugin";

fn target() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".config"))
        .join("omarchy/plugins")
        .join(ID)
}

/// Worth asking: on Omarchy, installed as a package, not in the bar yet and not asked before.
pub fn should_offer() -> bool {
    !settings::bar_widget_offered()
        && glib::find_program_in_path("omarchy").is_some()
        && PathBuf::from(SOURCE).join("manifest.json").is_file()
        && std::fs::symlink_metadata(target()).is_err()
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = [stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("failed");
        Err(format!("{program}: {message}"))
    }
}

/// Links the widget into the shell's plugin folder and enables it. Blocking.
pub fn add() -> Result<(), String> {
    let target = target();
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if std::fs::symlink_metadata(&target).is_err() {
        std::os::unix::fs::symlink(SOURCE, &target).map_err(|e| e.to_string())?;
    }
    run("omarchy-shell", &["shell", "rescanPlugins"])?;
    run("omarchy", &["plugin", "enable", ID, "--section", "right"])
}
