//! `program.json` – what a program tells the Toolbox about itself (DESIGN.md 4.1, 15.3). Written by hand only
//! for scripts and external exes; a kit program's is generated from its `Cargo.toml` (`kit::build`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::i18n::FALLBACK;

/// A text of the manifest: one string, or one per language (`{"de": "…", "en": "…"}`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum Text {
    One(String),
    PerLanguage(BTreeMap<String, String>),
}

impl Default for Text {
    fn default() -> Self {
        Text::One(String::new())
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Text::One(s.to_string())
    }
}

impl Text {
    /// In this language – else in English – else in the first language there is.
    pub fn get(&self, language: &str) -> &str {
        match self {
            Text::One(s) => s,
            Text::PerLanguage(m) => m.get(language).or_else(|| m.get(FALLBACK)).or_else(|| m.values().next()).map_or("", String::as_str),
        }
    }

    /// The same in every language of the program: what groups programs into one tab, whatever the Toolbox's
    /// language is.
    pub fn key(&self) -> &str {
        self.get(FALLBACK)
    }

    pub fn is_empty(&self) -> bool {
        self.get(FALLBACK).is_empty()
    }
}

/// A choice on a script's page (`options` in the manifest).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default, from = "RawOption")]
pub struct ScriptOption {
    pub values: Vec<Value>,
    /// Without it the first value.
    pub default: Value,
    pub label: Option<Text>,
    /// Label per value, keyed by the value as text.
    pub value_labels: BTreeMap<String, Text>,
}

/// In the file an option is the object above, or just the list of its values (the classic Toolbox's short form).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawOption {
    Values(Vec<Value>),
    Full {
        values: Vec<Value>,
        #[serde(default)]
        default: Value,
        #[serde(default)]
        label: Option<Text>,
        #[serde(default)]
        value_labels: BTreeMap<String, Text>,
    },
}

impl From<RawOption> for ScriptOption {
    fn from(raw: RawOption) -> Self {
        let (values, default, label, value_labels) = match raw {
            RawOption::Values(values) => (values, Value::Null, None, BTreeMap::new()),
            RawOption::Full { values, default, label, value_labels } => (values, default, label, value_labels),
        };
        let default = if default.is_null() { values.first().cloned().unwrap_or(Value::Null) } else { default };
        ScriptOption { values, default, label, value_labels }
    }
}

/// A value as the text it is shown and keyed with: a string without its quotes, anything else as JSON.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The three kinds of programs (DESIGN.md 4.2) – and the programs of the Python Toolbox, as long as there are any.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `run`: a script, started without a window by the agent.
    Script,
    /// `exe` + `protocol`: serves its pipe – page, status, tray.
    Kit,
    /// `exe` alone: start it, see whether it runs.
    Exe,
    /// Neither `run` nor `exe`: a program of the classic Toolbox (`starter`, `page`, `port`).
    Classic,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub name: Text,
    pub version: String,
    pub category: Text,
    pub description: Text,
    /// What Start runs – an `.exe` relative to the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// A `.ps1` relative to the manifest, started without a window by the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// An address or a path that is opened after `run` was started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, ScriptOption>,
    /// Explanation above a script's options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<Text>,
    /// Label of the start button; without it "Start".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<Text>,
    /// The program serves its pipe; this is the newest protocol it speaks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// Keeps running without a window: its menu sits under the Toolbox's tray icon, "quit all" includes it.
    pub background: bool,
    /// `owner/name` on GitHub – for updates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_key: Option<String>,
    /// Fields this kit does not know: those of a newer Toolbox, and those of the classic one (`starter`, `port`,
    /// `task`, `page` …).
    #[serde(flatten)]
    pub other: BTreeMap<String, Value>,
}

/// A manifest and the folder it lies in – paths in the manifest are relative to that folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub manifest: Manifest,
    pub home: PathBuf,
}

impl Manifest {
    pub fn kind(&self) -> Kind {
        match (&self.run, &self.exe, self.protocol) {
            (Some(_), _, _) => Kind::Script,
            (None, Some(_), Some(_)) => Kind::Kit,
            (None, Some(_), None) => Kind::Exe,
            (None, None, _) => Kind::Classic,
        }
    }

    pub fn parse(json: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        if m.id.is_empty() || !m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!("the id \"{}\" may only hold a–z, 0–9 and _", m.id));
        }
        if m.name.is_empty() {
            return Err("the name is missing".into());
        }
        Ok(m)
    }

    /// The program in this folder: `dist\program.json` if there is one (a built program), otherwise
    /// `program.json` in the folder itself (a script, or a program unpacked from its release zip).
    /// `Ok(None)` if the folder holds no manifest.
    pub fn read(folder: &Path) -> Result<Option<Found>, String> {
        for home in [folder.join("dist"), folder.to_path_buf()] {
            let file = home.join("program.json");
            match std::fs::read_to_string(&file) {
                Ok(json) => return Manifest::parse(&json).map(|manifest| Some(Found { manifest, home })).map_err(|e| format!("{}: {e}", file.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", file.display())),
            }
        }
        Ok(None)
    }

    /// For **Add program…**: the program in the folder – or, if it holds none itself, every program one level
    /// below it (a folder of programs). The second list names what could not be read.
    pub fn find(folder: &Path) -> (Vec<(PathBuf, Found)>, Vec<String>) {
        let (mut found, mut problems) = (vec![], vec![]);
        match Manifest::read(folder) {
            Ok(Some(f)) => found.push((folder.to_path_buf(), f)),
            Err(e) => problems.push(e),
            Ok(None) => {
                let mut below: Vec<PathBuf> = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                below.sort();
                for sub in below {
                    match Manifest::read(&sub) {
                        Ok(Some(f)) => found.push((sub, f)),
                        Ok(None) => {}
                        Err(e) => problems.push(e),
                    }
                }
            }
        }
        (found, problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAYMAN: &str = r#"{
      "id": "rayman_legends",
      "name": "Rayman Legends",
      "version": "1.1",
      "category": { "de": "Spiele", "en": "Games" },
      "description": { "de": "Startet Rayman Legends rahmenlos …", "en": "Starts Rayman Legends borderless …" },
      "run": "rayman_borderless.ps1",
      "launch": "uplay://launch/410/0",
      "button": { "de": "Spielen", "en": "Play" },
      "options": {
        "Pos": {
          "values": ["Left", "Center", "Right"],
          "default": "Center",
          "label": { "de": "Position", "en": "Position" },
          "value_labels": { "Left": { "de": "Links", "en": "Left" }, "Center": { "de": "Mitte", "en": "Center" } }
        }
      }
    }"#;

    #[test]
    fn the_script_example_of_the_design() {
        let m = Manifest::parse(RAYMAN).unwrap();
        assert_eq!(m.kind(), Kind::Script);
        assert_eq!((m.name.get("de"), m.category.get("de"), m.category.key()), ("Rayman Legends", "Spiele", "Games"));
        assert_eq!(m.category.get("fr"), "Games", "a missing language falls back to English");
        let pos = &m.options["Pos"];
        assert_eq!((pos.values.len(), &pos.default), (3, &Value::from("Center")));
        assert_eq!(pos.value_labels["Left"].get("de"), "Links");
        assert_eq!(m.button.as_ref().map(|b| b.get("en")), Some("Play"));
    }

    #[test]
    fn kinds() {
        let kit = Manifest::parse(r#"{"id":"hardware_monitor","name":"Hardware Monitor","exe":"HardwareMonitor.exe","protocol":1,"background":true,"later":{"x":1}}"#).unwrap();
        assert_eq!(kit.kind(), Kind::Kit);
        assert!(kit.background);
        assert_eq!(kit.other["later"]["x"], 1, "unknown fields are kept");
        assert_eq!(Manifest::parse(r#"{"id":"last_bastion","name":"Last Bastion","exe":"LastBastion.exe"}"#).unwrap().kind(), Kind::Exe);
        // The classic Toolbox's manifest: its fields are kept as they are, and the short form of options works.
        let classic = Manifest::parse(r#"{"id":"discord_plus","name":"Discord Plus","page":"discord_plus_gui.py","starter":"DiscordPlus.exe","port":47817}"#).unwrap();
        assert_eq!(classic.kind(), Kind::Classic);
        assert_eq!(classic.other["port"], 47817);
        let short = Manifest::parse(r#"{"id":"x","name":"X","run":"x.ps1","options":{"Pos":["Left","Right"]}}"#).unwrap();
        assert_eq!(short.options["Pos"].default, "Left");
    }

    #[test]
    fn what_is_wrong_is_refused() {
        assert!(Manifest::parse(r#"{"id":"Has Space","name":"X"}"#).is_err());
        assert!(Manifest::parse(r#"{"id":"x"}"#).is_err());
        assert!(Manifest::parse("not json").is_err());
    }

    #[test]
    fn found_in_a_folder_and_one_level_below() {
        let root = std::env::temp_dir().join(format!("kit-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        };
        write("Script/program.json", r#"{"id":"script","name":"Script","run":"a.ps1"}"#);
        write("Built/dist/program.json", r#"{"id":"built","name":"Built","exe":"Built.exe","protocol":1}"#);
        write("Built/program.json", r#"{"id":"wrong","name":"the one in dist wins"}"#);
        write("Broken/program.json", "{");
        write("Nothing/readme.txt", "");

        let built = Manifest::read(&root.join("Built")).unwrap().unwrap();
        assert_eq!((built.manifest.id.as_str(), built.home), ("built", root.join("Built").join("dist")));
        assert_eq!(Manifest::read(&root.join("Nothing")), Ok(None));

        let (found, problems) = Manifest::find(&root);
        let ids: Vec<_> = found.iter().map(|(_, f)| f.manifest.id.as_str()).collect();
        assert_eq!(ids, ["built", "script"]);
        assert_eq!(problems.len(), 1);
        let (single, _) = Manifest::find(&root.join("Script"));
        assert_eq!(single.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
