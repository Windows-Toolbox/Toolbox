//! Gear → Export / Import all settings (DESIGN.md 8, 15.8): one zip with the Toolbox's `data\settings.json` and the
//! whole `data\` of every program on the list – the way to take everything a program keeps to another PC or another
//! copy, instead of copying each `data\` by hand. Logs stay behind.

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
/// A program's file in `data\` above this size is left out (a program's data is a few kilobytes).
const MAX_FILE: usize = 32 << 20;

/// A program's files: (path inside `data\` with `/`, content).
pub type Files = Vec<(String, Vec<u8>)>;

/// Where a program's `rel` (a path inside its `data\`, with `/`) lies in the zip.
fn program_file(id: &str, rel: &str) -> String {
    format!("programs/{id}/{rel}")
}

/// Logs and half-written files stay behind; everything else in `data\` is the program's.
fn kept(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !(lower.ends_with(".log") || lower.ends_with(".tmp") || lower == "log.txt" || lower == "log.old.txt")
}

/// A path from the zip that stays inside `data\`: plain names joined by `/`, none of them leading out.
fn safe(rel: &str) -> bool {
    !rel.is_empty()
        && rel.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':']))
        && rel.rsplit('/').next().is_some_and(kept)
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

/// The program's `data\` – what is exported of it.
pub fn data_dir(entry: &Entry) -> PathBuf {
    settings_file(entry).parent().map(PathBuf::from).unwrap_or_default()
}

/// Every file in `dir` worth taking along, as (path with `/`, content), sorted.
fn files_in(dir: &Path) -> Result<Files, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Files) -> Result<(), String> {
        let Ok(read) = std::fs::read_dir(dir) else { return Ok(()) };
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let path = item.path();
            let Ok(kind) = item.file_type() else { continue };
            if kind.is_dir() {
                walk(&path, &rel, out)?;
            } else if kind.is_file() && safe(&rel) && item.metadata().is_ok_and(|m| m.len() as usize <= MAX_FILE) {
                out.push((rel, std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?));
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(dir, "", &mut out)?;
    out.sort();
    Ok(out)
}

/// Packs the Toolbox's settings and every program's `data\` that holds something. The number of programs in it.
pub fn export(toolbox: &Path, programs: &[Entry], version: &str) -> Result<(Vec<u8>, usize), String> {
    let read = |file: &Path| std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()));
    let mut files = vec![(TOOLBOX.to_string(), read(toolbox)?)];
    let mut index = Index { version: version.to_string(), programs: vec![] };
    for entry in programs {
        let found = files_in(&data_dir(entry))?;
        if found.is_empty() {
            continue;
        }
        files.extend(found.into_iter().map(|(rel, bytes)| (program_file(entry.id(), &rel), bytes)));
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
    /// Each program's files, by id, with its name.
    pub programs: BTreeMap<String, (Text, Files)>,
}

/// Reads a zip of exported settings. Every `settings.json` must be a JSON object; anything outside the index's
/// programs is ignored. (A zip of Toolbox 2.1.0 holds only `settings.json` per program – the same layout.)
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
        let prefix = program_file(&id, "");
        let mut found = vec![];
        for (path, bytes) in files.range(prefix.clone()..) {
            let Some(rel) = path.strip_prefix(&prefix) else { break };
            if !safe(rel) {
                continue;
            }
            if rel == "settings.json" {
                object(path, bytes)?;
            }
            found.push((rel.to_string(), bytes.clone()));
        }
        if !found.is_empty() {
            contents.programs.insert(id, (name, found));
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
    /// Programs on the list whose `data\` it puts back.
    pub programs: Vec<Text>,
    /// Programs in the file that are not on the list here: left out.
    pub skipped: Vec<Text>,
}

/// Writes one file the way `kit::settings` does: aside first, then renamed – never half a file.
pub fn write_file(file: &Path, bytes: &[u8]) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", file.display());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(fail)?;
    std::fs::rename(&tmp, file).map_err(fail)
}

/// Puts a program's exported files back into its `data\`. Files that are there and not in the zip stay.
pub fn put_back(entry: &Entry, files: &Files) -> Result<(), String> {
    let dir = data_dir(entry);
    for (rel, bytes) in files {
        let file = rel.split('/').fold(dir.clone(), |path, part| path.join(part));
        write_file(&file, bytes)?;
    }
    Ok(())
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
        write_file(&toolbox, br#"{"language":"de","programs":["..\\Zones"]}"#).unwrap();
        let zones = entry(&dir.join("Zones"), r#"{"id":"zones","name":"Zones","exe":"Zones.exe","protocol":1}"#);
        write_file(&settings_file(&zones), br#"{"entries":[1,2]}"#).unwrap();
        // Everything else in data\ goes along – a login, a backup in a subfolder – but no logs.
        write_file(&data_dir(&zones).join("login.json"), b"{}").unwrap();
        write_file(&data_dir(&zones).join("backup").join("a b.url"), b"url").unwrap();
        for log in ["log.txt", "log.old.txt", "monitor.log", "x.tmp"] {
            write_file(&data_dir(&zones).join(log), b"log").unwrap();
        }
        let script = entry(&dir.join("Script"), r#"{"id":"script","name":"Script","run":"s.ps1"}"#);
        write_file(&script.options_file(), br#"{"Pos":"Left"}"#).unwrap();
        // Never started: nothing in data\, nothing to take along.
        let fresh = entry(&dir.join("Fresh"), r#"{"id":"fresh","name":"Fresh","exe":"F.exe","protocol":1}"#);

        let (bytes, count) = export(&toolbox, &[zones.clone(), script, fresh], "2.1.1").unwrap();
        assert_eq!(count, 2);
        let contents = read(&bytes).unwrap();
        assert_eq!(contents.toolbox.unwrap()["language"], "de");
        assert_eq!(contents.programs.keys().collect::<Vec<_>>(), ["script", "zones"]);
        let names: Vec<&str> = contents.programs["zones"].1.iter().map(|(rel, _)| rel.as_str()).collect();
        assert_eq!(names, ["backup/a b.url", "login.json", "settings.json"]);

        // Put back into another copy: the subfolder is made, what was there and not in the zip stays.
        let other = entry(&dir.join("Other"), r#"{"id":"zones","name":"Zones","exe":"Zones.exe","protocol":1}"#);
        write_file(&data_dir(&other).join("mine.json"), b"mine").unwrap();
        put_back(&other, &contents.programs["zones"].1).unwrap();
        assert_eq!(std::fs::read(data_dir(&other).join("backup").join("a b.url")).unwrap(), b"url");
        assert_eq!(std::fs::read(settings_file(&other)).unwrap(), br#"{"entries":[1,2]}"#);
        assert!(data_dir(&other).join("mine.json").exists());

        // A zip of 2.1.0 (settings.json only) reads the same.
        let old = [
            (INDEX.to_string(), br#"{"version":"2.1.0","programs":[{"id":"zones","name":"Zones"}]}"#.to_vec()),
            (program_file("zones", "settings.json"), b"{}".to_vec()),
        ];
        assert_eq!(read(&zip::write(&old)).unwrap().programs["zones"].1, [("settings.json".to_string(), b"{}".to_vec())]);

        // Not ours, broken, or with an id or a path that would leave its folder.
        assert!(read(&zip::write(&[("a.txt".into(), b"x".to_vec())])).is_err());
        let index = |id: &str| (INDEX.to_string(), format!(r#"{{"version":"2","programs":[{{"id":"{id}","name":"X"}}]}}"#).into_bytes());
        assert!(read(&zip::write(&[index("x"), (program_file("x", "settings.json"), b"[1]".to_vec())])).is_err());
        let sneaky = read(&zip::write(&[index("../x"), ("programs/../x/settings.json".into(), b"{}".to_vec())])).unwrap();
        assert!(sneaky.programs.is_empty() && sneaky.toolbox.is_none());
        let paths = ["../a.json", "a/../../b.json", "C:/a.json", "a\\..\\b.json", "/a.json", "log.txt"];
        let mut files = vec![index("x")];
        files.extend(paths.iter().map(|p| (program_file("x", p), b"{}".to_vec())));
        assert!(read(&zip::write(&files)).unwrap().programs.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
