//! Remembered preferences: the audio format, the transcription language,
//! the name you go by in transcripts and whether the bar widget was offered.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use gtk::glib;

use crate::APP_NAME;
use crate::export::Format;
use crate::transcribe::LANGUAGES;

fn path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join(APP_NAME).join("settings.json")
}

fn load() -> serde_json::Value {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Updates one key and keeps the others.
fn save(key: &str, value: &str) {
    let mut settings = load();
    settings[key] = serde_json::Value::String(value.to_owned());
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, settings.to_string());
}

pub fn load_format() -> Format {
    load()["format"]
        .as_str()
        .map(Format::from_key)
        .unwrap_or(Format::Mono)
}

pub fn save_format(format: Format) {
    save("format", format.key());
}

/// A whisper language code from `LANGUAGES`, "auto" when unset or unknown.
pub fn load_language() -> &'static str {
    let settings = load();
    let saved = settings["language"].as_str().unwrap_or("auto");
    LANGUAGES
        .iter()
        .map(|(code, _)| *code)
        .find(|code| *code == saved)
        .unwrap_or("auto")
}

pub fn save_language(code: &str) {
    save("language", code);
}

/// The diarization engine. Keep local as the default so upgrading does not
/// change the app's current behaviour.
pub fn load_diarization() -> &'static str {
    match load()["diarization"].as_str() {
        Some("off") => "off",
        Some("remote") => "remote",
        _ => "local",
    }
}

pub fn save_diarization(mode: &str) {
    save("diarization", mode);
}

pub fn load_transcription() -> &'static str {
    match load()["transcription"].as_str() {
        Some("remote") => "remote",
        _ => "local",
    }
}

pub fn save_transcription(mode: &str) {
    save("transcription", mode);
}

/// Whether a recording should start transcription as soon as it is saved.
pub fn load_process_after_recording() -> bool {
    load()["process_after_recording"].as_bool().unwrap_or(true)
}

pub fn save_process_after_recording(enabled: bool) {
    let mut settings = load();
    settings["process_after_recording"] = serde_json::Value::Bool(enabled);
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, settings.to_string());
}

pub fn load_live_diarization() -> bool {
    load()["live_diarization"].as_bool().unwrap_or(false)
}

pub fn save_live_diarization(enabled: bool) {
    let mut settings = load();
    settings["live_diarization"] = serde_json::Value::Bool(enabled);
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, settings.to_string());
}

/// The remote server address is not secret.
pub fn load_remote_ip() -> String {
    load()["remote_ip"].as_str().unwrap_or_default().to_owned()
}

pub fn save_remote_ip(ip: &str) {
    save("remote_ip", ip);
}

/// The remote key lives in the desktop keyring, never in settings.json.
pub fn load_remote_api_key() -> String {
    Command::new("secret-tool")
        .args(["lookup", "application", APP_NAME, "item", "remote-api-key"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|key| key.trim().to_owned())
        .unwrap_or_default()
}

/// Saves an empty key by removing the previous entry. The key itself goes via
/// stdin so it never appears in the process list.
pub fn save_remote_api_key(key: &str) -> bool {
    if key.trim().is_empty() {
        return Command::new("secret-tool")
            .args(["clear", "application", APP_NAME, "item", "remote-api-key"])
            .status()
            .is_ok_and(|status| status.success());
    }
    let Ok(mut child) = Command::new("secret-tool")
        .args([
            "store",
            "--label=Meeting Recorder remote API key",
            "application",
            APP_NAME,
            "item",
            "remote-api-key",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .as_mut()
        .is_some_and(|stdin| stdin.write_all(key.trim().as_bytes()).is_ok());
    wrote && child.wait().is_ok_and(|status| status.success())
}

/// What the mic side is called in new transcripts, "You" until you change it.
pub fn load_your_name() -> String {
    load()["your_name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(crate::meeting::DEFAULT_YOU)
        .to_owned()
}

pub fn save_your_name(name: &str) {
    save("your_name", name);
}

/// Whether the app already asked to put its widget in the bar.
pub fn bar_widget_offered() -> bool {
    load()["bar_widget_offered"].as_str() == Some("yes")
}

pub fn set_bar_widget_offered() {
    save("bar_widget_offered", "yes");
}
