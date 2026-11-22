//! Gear → Export / Import all settings (DESIGN.md 8): one zip with the `data\settings.json` of the Toolbox and of
//! every program on the list – the way to take your settings to another PC or another copy, instead of copying
//! each `data\` by hand. Only settings: no logs, no other files a program keeps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kit::manifest::{Kind, Text};
use kit::update::zip;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::Entry;

/// The zip's table of contents – what is in it, with the names to show before importing.
const INDEX: &str = "windows-toolbox-settings.json";
const TOOLBOX: &str = "toolbox/settings.json";
/// No settings file is anywhere near this big.
const MAX_FILE: usize = 4 << 20;

fn program_file(id: &str) -> String {
    format!("programs/{id}/settings.json")
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
struct Index {
    /// The Toolbox's version that wrote it.
    version: String,
    programs: Vec<Named>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Named {
    pub id: String,
    pub name: Text,
}

/// The file a program keeps its settings in: a kit program's (and any exe's) `data\` next to its exe, a script's
/// `data\` next to its manifest.
pub fn settings_file(entry: &Entry) -> PathBuf {
    match (entry.kind, entry.exe()) {
        (Kind::Kit | Kind::Exe, Some(exe)) => exe.parent().unwrap_or(&entry.home).join("data").join("settings.json"),
        _ => entry.options_file(),
    }
}

/// Packs the Toolbox's settings and every program's that has a settings file. The number of programs in it.
pub fn export(toolbox: &Path, programs: &[Entry], version: &str) -> Result<(Vec<u8>, usize), String> {
    let read = |file: &Path| std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()));
    let mut files = vec![(TOOLBOX.to_string(), read(toolbox)?)];
    let mut index = Index { version: version.to_string(), programs: vec![] };
    for entry in programs {
        let file = settings_file(entry);
        if !file.is_file() {
            continue;
        }
        files.push((program_file(entry.id()), read(&file)?));
        index.programs.push(Named { id: entry.id().to_string(), name: entry.manifest.name.clone() });
    }
    let count = index.programs.len();
    files.insert(0, (INDEX.to_string(), serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?));
    Ok((zip::write(&files), count))
}

/// What a zip of exported settings holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Contents {
    /// The Toolbox's settings, as they were saved.
    pub toolbox: Option<Value>,
    /// Each program's settings file, by id, with its name.
    pub programs: BTreeMap<String, (Text, Vec<u8>)>,
}

/// Reads a zip of exported settings. Every file must be a JSON object; anything else in the zip is ignored.
pub fn read(bytes: &[u8]) -> Result<Contents, String> {
    let files: BTreeMap<String, Vec<u8>> = zip::read(bytes, MAX_FILE)?.into_iter().collect();
    let index: Index = files.get(INDEX).and_then(|b| serde_json::from_slice(b).ok()).ok_or("this zip holds no exported settings of the Windows Toolbox")?;
    let object = |name: &str, bytes: &[u8]| match serde_json::from_slice::<Value>(bytes) {
        Ok(v) if v.is_object() => Ok(v),
        _ => Err(format!("{name} in the zip is not a settings file")),
    };
    let mut contents = Contents { toolbox: files.get(TOOLBOX).map(|b| object(TOOLBOX, b)).transpose()?, ..Default::default() };
    for Named { id, name } in index.programs {
        // An id names a folder inside the zip; only the characters an id may have.
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            continue;
        }
        if let Some(bytes) = files.get(&program_file(&id)) {
            object(&program_file(&id), bytes)?;
            contents.programs.insert(id, (name, bytes.clone()));
        }
    }
    Ok(contents)
}

/// What an import will do, shown before it is done.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Plan {
    pub file: PathBuf,
    /// The Toolbox's own settings: look, language, the switches – and the programs it lists that are found here.
    pub toolbox: bool,
    /// Programs on the list whose settings it replaces.
    pub programs: Vec<Text>,
    /// Programs in the file that are not on the list here: left out.
    pub skipped: Vec<Text>,
}

/// Writes one settings file the way `kit::settings` does: aside first, then renamed – never half a file.
pub fn write_settings(file: &Path, bytes: &[u8]) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", file.display());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(fail)?;
    std::fs::rename(&tmp, file).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::manifest::Manifest;

    fn entry(home: &Path, json: &str) -> Entry {
        let manifest = Manifest::parse(json).unwrap();
        Entry { stored: String::new(), folder: home.into(), home: home.into(), kind: manifest.kind(), manifest, options: Default::default(), run: Default::default(), autostart: None }
    }

    #[test]
    fn exported_and_read_back() {
        let dir = std::env::temp_dir().join(format!("toolbox-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let toolbox = dir.join("Toolbox/data/settings.json");
        write_settings(&toolbox, br#"{"language":"de","programs":["..\\Zones"]}"#).unwrap();
        let zones = entry(&dir.join("Zones"), r#"{"id":"zones","name":"Zones","exe":"Zones.exe","protocol":1}"#);
        write_settings(&settings_file(&zones), br#"{"entries":[1,2]}"#).unwrap();
        let script = entry(&dir.join("Script"), r#"{"id":"script","name":"Script","run":"s.ps1"}"#);
        write_settings(&script.options_file(), br#"{"Pos":"Left"}"#).unwrap();
        // Never started: no settings file, nothing to take along.
        let fresh = entry(&dir.join("Fresh"), r#"{"id":"fresh","name":"Fresh","exe":"F.exe","protocol":1}"#);

        let (bytes, count) = export(&toolbox, &[zones, script, fresh], "2.1.0").unwrap();
        assert_eq!(count, 2);
        let contents = read(&bytes).unwrap();
        assert_eq!(contents.toolbox.unwrap()["language"], "de");
        assert_eq!(contents.programs.keys().collect::<Vec<_>>(), ["script", "zones"]);
        assert_eq!(contents.programs["zones"].1, br#"{"entries":[1,2]}"#);

        // Not ours, broken, or with an id that would leave its folder.
        assert!(read(&zip::write(&[("a.txt".into(), b"x".to_vec())])).is_err());
        let index = |id: &str| (INDEX.to_string(), format!(r#"{{"version":"2","programs":[{{"id":"{id}","name":"X"}}]}}"#).into_bytes());
        assert!(read(&zip::write(&[index("x"), (program_file("x"), b"[1]".to_vec())])).is_err());
        let sneaky = read(&zip::write(&[index("../x"), ("programs/../x/settings.json".into(), b"{}".to_vec())])).unwrap();
        assert!(sneaky.programs.is_empty() && sneaky.toolbox.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
