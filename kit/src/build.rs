//! For a program's `build.rs` (DESIGN.md 4.1, 15.4): one call, `toolbox_kit::build::manifest()`.
//!
//! It reads what the program says about itself in its `Cargo.toml` – `[package]` and
//! `[package.metadata.toolbox]` – and from that
//! - writes `program.json` next to the build output (as `<package>.program.json`; `build.ps1` copies it into
//!   `dist\`),
//! - hands id and manifest to the code (`kit::meta!()`),
//! - compiles name, description, version and icon into the exe, so Task Manager and the taskbar show
//!   "Hardware Monitor" with its icon.
//!
//! So every one of these facts exists once.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::res;

/// Call from `build.rs`. Stops the build with a message if `Cargo.toml` lacks something.
pub fn manifest() {
    if let Err(e) = run() {
        // Cargo shows this as the build error.
        panic!("toolbox-kit: {e}");
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("the environment variable {name} is missing – call this from build.rs"))
}

fn run() -> Result<(), String> {
    let dir = PathBuf::from(env("CARGO_MANIFEST_DIR")?);
    let package = env("CARGO_PKG_NAME")?;
    let version = env("CARGO_PKG_VERSION")?;
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");

    let metadata = cargo_metadata(&dir, &package)?;
    let toolbox = metadata.pointer("/metadata/toolbox").and_then(Value::as_object).cloned();
    let toolbox = toolbox.ok_or("Cargo.toml has no [package.metadata.toolbox] (DESIGN.md 15.4)")?;
    let manifest = program_json(&toolbox, &version, &std::env::var("CARGO_PKG_DESCRIPTION").unwrap_or_default(), &std::env::var("CARGO_PKG_REPOSITORY").unwrap_or_default())?;
    let id = manifest["id"].as_str().unwrap_or_default().to_string();
    let exe = manifest["exe"].as_str().unwrap_or_default().to_string();

    // For the code: kit::meta!()
    let dist = dir.join(toolbox.get("dist").and_then(Value::as_str).unwrap_or("dist"));
    println!("cargo:rustc-env=TOOLBOX_ID={id}");
    println!("cargo:rustc-env=TOOLBOX_DIST={}", dist.display());
    println!("cargo:rustc-env=TOOLBOX_MANIFEST={}", Value::Object(manifest.clone()));

    // For build.ps1: next to the exe that cargo builds, named after the package (several programs may share one
    // build folder).
    let out_dir = PathBuf::from(env("OUT_DIR")?);
    if let Some(profile_dir) = out_dir.ancestors().nth(3) {
        let file = profile_dir.join(format!("{package}.program.json"));
        std::fs::write(&file, readable(&manifest)).map_err(|e| format!("{}: {e}", file.display()))?;
    }

    // For Windows: icon, name and version inside the exe.
    let icon_path = match toolbox.get("icon").and_then(Value::as_str) {
        Some(p) => Some(dir.join(p)),
        None => Some(dir.join("assets").join("icon.ico")).filter(|p| p.exists()),
    };
    let icon = match &icon_path {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", p.display());
            Some(std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
        }
        None => None,
    };
    let name = text_in(&manifest["name"], "en");
    let info = res::Version { description: &name, product: &name, company: "Windows Toolbox", file_name: &exe, version: &version };
    let res_file = out_dir.join("program.res");
    std::fs::write(&res_file, res::build(icon.as_deref(), &info)?).map_err(|e| format!("{}: {e}", res_file.display()))?;
    println!("cargo:rustc-link-arg-bins={}", res_file.display());
    Ok(())
}

/// Asks cargo for the package as it sees it – `cargo metadata` prints every `Cargo.toml` as JSON, including the
/// tables under `[package.metadata]` that cargo itself ignores. No TOML reader of our own is needed.
fn cargo_metadata(dir: &Path, package: &str) -> Result<Value, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let all: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = all["packages"].as_array().cloned().unwrap_or_default();
    packages.into_iter().find(|p| p["name"] == package).ok_or(format!("cargo metadata does not list the package {package}"))
}

/// The manifest as a file a person can read: the fields in the order of DESIGN.md 15.3, not alphabetically.
fn readable(manifest: &Map<String, Value>) -> String {
    const ORDER: [&str; 11] = ["id", "name", "version", "category", "description", "exe", "repository", "update_key", "protocol", "background", "button"];
    let first = ORDER.iter().filter_map(|k| manifest.get_key_value(*k));
    let rest = manifest.iter().filter(|(k, _)| !ORDER.contains(&k.as_str()));
    let fields: Vec<String> = first
        .chain(rest)
        .map(|(key, value)| {
            let value = serde_json::to_string_pretty(value).unwrap_or_default().replace('\n', "\n  ");
            format!("  {}: {value}", Value::from(key.as_str()))
        })
        .collect();
    format!("{{\n{}\n}}\n", fields.join(",\n"))
}

/// A text of the manifest: a string, or one per language.
fn text_in(value: &Value, language: &str) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(m) => m.get(language).or_else(|| m.values().next()).and_then(Value::as_str).unwrap_or_default().to_string(),
        _ => String::new(),
    }
}

/// `program.json` of a kit program (DESIGN.md 15.3) from `[package.metadata.toolbox]` and `[package]`.
fn program_json(toolbox: &Map<String, Value>, version: &str, description: &str, repository: &str) -> Result<Map<String, Value>, String> {
    let need = |key: &str| toolbox.get(key).cloned().ok_or(format!("[package.metadata.toolbox] has no `{key}` (DESIGN.md 15.4)"));
    let id = need("id")?;
    let id_text = id.as_str().unwrap_or_default();
    if id_text.is_empty() || !id_text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(format!("the id {id} may only hold a–z, 0–9 and _"));
    }
    let mut m = Map::new();
    m.insert("id".into(), id);
    m.insert("name".into(), need("name")?);
    m.insert("version".into(), json!(version));
    m.insert("category".into(), need("category")?);
    m.insert("description".into(), toolbox.get("description").cloned().unwrap_or(json!(description)));
    m.insert("exe".into(), need("exe")?);
    // "https://github.com/Windows-Toolbox/Hardware-Monitor" -> "Windows-Toolbox/Hardware-Monitor"
    let short = repository.trim_end_matches('/').trim_end_matches(".git").rsplit('/').take(2).collect::<Vec<_>>();
    if let [name, owner] = short[..] {
        if repository.contains("github.com") {
            m.insert("repository".into(), json!(format!("{owner}/{name}")));
            // Updates are only accepted if they are signed with this key (15.6). Somebody who releases an own
            // program with the kit names the own key in `[package.metadata.toolbox]`.
            m.insert("update_key".into(), toolbox.get("update_key").cloned().unwrap_or(json!(crate::UPDATE_KEY)));
        }
    }
    m.insert("protocol".into(), json!(crate::PROTOCOL));
    m.insert("background".into(), toolbox.get("background").cloned().unwrap_or(json!(false)));
    if let Some(button) = toolbox.get("button") {
        m.insert("button".into(), button.clone());
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_of_the_design() {
        let toolbox = json!({
            "id": "hardware_monitor", "exe": "HardwareMonitor.exe", "background": true,
            "category": {"de": "Allgemein", "en": "General"},
            "name": {"de": "Hardware-Monitor", "en": "Hardware Monitor"},
        });
        let m = program_json(toolbox.as_object().unwrap(), "2.0.0", "CPU and GPU temperature …", "https://github.com/Windows-Toolbox/Hardware-Monitor").unwrap();
        assert_eq!(m["version"], "2.0.0");
        assert_eq!(m["repository"], "Windows-Toolbox/Hardware-Monitor");
        assert_eq!(m["update_key"], crate::UPDATE_KEY);
        assert_eq!(m["description"], "CPU and GPU temperature …");
        assert_eq!(m["protocol"], crate::PROTOCOL);
        assert_eq!(text_in(&m["name"], "en"), "Hardware Monitor");
        assert_eq!(text_in(&m["name"], "fr"), "Hardware-Monitor");
        // The file starts with the id and is valid JSON with the same content.
        let file = readable(&m);
        assert!(file.starts_with("{\n  \"id\": \"hardware_monitor\",\n  \"name\": {\n    \"de\": \"Hardware-Monitor\","), "{file}");
        assert_eq!(serde_json::from_str::<Value>(&file).unwrap(), Value::Object(m));
    }

    #[test]
    fn what_is_missing_is_named() {
        let no_exe = json!({"id": "x", "name": "X", "category": "General"});
        assert!(program_json(no_exe.as_object().unwrap(), "1.0.0", "", "").unwrap_err().contains("`exe`"));
        let bad_id = json!({"id": "Bad Id", "name": "X", "category": "General", "exe": "X.exe"});
        assert!(program_json(bad_id.as_object().unwrap(), "1.0.0", "", "").is_err());
        let no_repo = json!({"id": "x", "name": "X", "category": "General", "exe": "X.exe"});
        let m = program_json(no_repo.as_object().unwrap(), "1.0.0", "", "").unwrap();
        assert!(!m.contains_key("repository") && !m.contains_key("update_key"), "no repository, no updates, no key");
    }
}
