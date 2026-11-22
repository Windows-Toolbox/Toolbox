//! The Toolbox's own settings and its list of programs (DESIGN.md 4.2): which folders were added, what their
//! manifests say, how a folder is remembered so that moving everything together keeps working.
//! Plain data and files – no pipes, no windows; the agent holds it, the window gets a copy.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use kit::autostart::{self, State as Autostart};
use kit::i18n;
use kit::manifest::{Kind, Manifest, Text, value_text};
use kit::page::{Block, ChoiceOption, Page};
use kit::process::quote;
use kit::proto::Health;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `data\settings.json` of the Toolbox.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Applies to the Toolbox and – through `hello` – to every kit program.
    pub language: String,
    /// `dark`, `light` or `system`.
    pub theme: String,
    /// 1.0 = 100 %; changed with Ctrl + mouse wheel.
    pub zoom: f32,
    /// The folders added with "Add program…", relative to the Toolbox's folder where possible.
    pub programs: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { language: i18n::system_language().to_string(), theme: "dark".into(), zoom: 1.0, programs: vec![] }
    }
}

impl Settings {
    /// Anything invalid gets its default.
    pub fn checked(mut self) -> Self {
        let default = Settings::default();
        if !i18n::is_language(&self.language) {
            self.language = default.language;
        }
        if !kit::ui::THEMES.contains(&self.theme.as_str()) {
            self.theme = default.theme;
        }
        if !(0.5..=3.0).contains(&self.zoom) {
            self.zoom = default.zoom;
        }
        self
    }
}

/// Whether a program runs, as far as the Toolbox can tell.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Run {
    /// `None`: not looked at yet.
    pub running: Option<bool>,
    pub state: Health,
    /// The program's status line.
    pub text: String,
    /// Its pipe closed without `bye` (section 8).
    pub crashed: bool,
}

/// One program on the list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    /// The folder as it stands in the settings …
    pub stored: String,
    /// … and where that is.
    pub folder: PathBuf,
    /// Where the manifest lies (`folder` or `folder\dist`); its paths are relative to this.
    pub home: PathBuf,
    pub manifest: Manifest,
    pub kind: Kind,
    /// A script's options with their current values.
    pub options: BTreeMap<String, Value>,
    pub run: Run,
    /// What Windows has noted for "Start with Windows" – only for background kit programs, `None` for the others.
    #[serde(default)]
    pub autostart: Option<autostart::Entry>,
}

impl Entry {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn exe(&self) -> Option<PathBuf> {
        self.manifest.exe.as_ref().map(|e| self.home.join(e))
    }

    pub fn script(&self) -> Option<PathBuf> {
        self.manifest.run.as_ref().map(|r| self.home.join(r))
    }

    /// The file a script reads its options from: `data\settings.json` next to it.
    pub fn options_file(&self) -> PathBuf {
        self.home.join("data").join("settings.json")
    }

    /// "Start with Windows" belongs to programs that keep running without a window (DESIGN.md 4.4).
    pub fn has_autostart(&self) -> bool {
        self.kind == Kind::Kit && self.manifest.background
    }

    /// Reads what Windows has noted (direct registry calls, cheap). `true` if it changed.
    pub fn read_autostart(&mut self) -> bool {
        let now = self.has_autostart().then(|| autostart::read(self.id()));
        let changed = now != self.autostart;
        self.autostart = now;
        changed
    }

    /// On means: Windows starts **this** copy. An entry that starts another copy, or one Windows switched off,
    /// is off here – switching on makes it start this one.
    pub fn autostart_on(&self) -> bool {
        self.autostart.as_ref().is_some_and(|a| a.state == Autostart::On && self.autostart_is_mine())
    }

    /// The entry names this copy's exe – or an exe that is gone (a moved folder, repaired at the next start).
    pub fn autostart_is_mine(&self) -> bool {
        match (self.autostart.as_ref().and_then(|a| a.target.as_ref()), self.exe()) {
            (Some(target), Some(exe)) => !target.exists() || same_file(target, &exe),
            _ => false,
        }
    }

    pub fn readme(&self) -> Option<PathBuf> {
        [&self.home, &self.folder].into_iter().map(|dir| dir.join("README.md")).find(|f| f.exists())
    }

    pub fn log(&self) -> PathBuf {
        self.home.join("data").join("log.txt")
    }

    /// What Start would run, if it is there – for the hint "… is missing".
    pub fn start_target(&self) -> Option<PathBuf> {
        match self.kind {
            Kind::Script => self.script(),
            Kind::Kit | Kind::Exe => self.exe(),
        }
    }

    fn read_options(&mut self) {
        let saved: BTreeMap<String, Value> = std::fs::read_to_string(self.options_file()).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or_default();
        self.options = self.manifest.options.iter().map(|(name, option)| {
            let value = saved.get(name).filter(|v| option.values.contains(v)).cloned().unwrap_or_else(|| option.default.clone());
            (name.clone(), value)
        }).collect();
    }

    /// Changes one option and writes the file the script reads. Whatever else stands in the file is kept.
    pub fn set_option(&mut self, name: &str, value: Value) -> Result<(), String> {
        let option = self.manifest.options.get(name).ok_or(format!("unknown option '{name}'"))?;
        if !option.values.contains(&value) {
            return Err(format!("'{name}' cannot be {}", value_text(&value)));
        }
        let file = self.options_file();
        let mut all: serde_json::Map<String, Value> = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or_default();
        all.insert(name.to_string(), value.clone());
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", file.display()))?;
        self.options.insert(name.to_string(), value);
        Ok(())
    }
}

/// A script's page (DESIGN.md 4.4): its hint and its options as the same building blocks a kit program sends, so
/// one renderer draws both. A `set` from this page is an option to save (`Entry::set_option`).
pub fn script_page(entry: &Entry, language: &str) -> Page {
    let mut blocks = vec![];
    if let Some(hint) = entry.manifest.hint.as_ref().map(|h| h.get(language)).filter(|h| !h.is_empty()) {
        blocks.push(Block::Text { text: hint.to_string() });
    }
    for (name, option) in &entry.manifest.options {
        let label = option.label.as_ref().map(|l| l.get(language)).filter(|l| !l.is_empty()).unwrap_or(name).to_string();
        let options = option
            .values
            .iter()
            .map(|value| {
                let text = value_text(value);
                let label = option.value_labels.get(&text).map(|l| l.get(language).to_string()).filter(|l| !l.is_empty()).unwrap_or(text);
                ChoiceOption { value: value.clone(), label, state: None }
            })
            .collect();
        let value = entry.options.get(name).cloned().unwrap_or_else(|| option.default.clone());
        blocks.push(Block::Choice { label, key: name.clone(), value, options });
    }
    Page { blocks }
}

/// The command line that runs a script without a window (DESIGN.md 15.3).
pub fn script_command(entry: &Entry) -> Result<String, String> {
    let script = entry.script().ok_or("the manifest names no script")?;
    if !script.extension().is_some_and(|e| e.eq_ignore_ascii_case("ps1")) {
        return Err(format!("{}: only .ps1 scripts can be started", script.display()));
    }
    if !script.exists() {
        return Err(format!("{} is missing", script.display()));
    }
    // The script reads its options from its `data\settings.json`.
    Ok(format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File {}", quote(&script.to_string_lossy())))
}

/// Why a folder on the list shows no program.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "problem", content = "detail", rename_all = "snake_case")]
pub enum Problem {
    /// The folder (or its `program.json`) is not there.
    NotFound,
    /// The manifest cannot be read: the reason.
    Broken(String),
    /// Another folder on the list holds a program with the same id: that program's name.
    SameId(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Missing {
    pub stored: String,
    pub folder: PathBuf,
    pub problem: Problem,
}

/// Everything the window shows: sent by the agent on every change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Library {
    pub settings: Settings,
    pub programs: Vec<Entry>,
    pub missing: Vec<Missing>,
    /// The Toolbox's folder and its `data\`.
    pub folder: PathBuf,
    pub data: PathBuf,
    /// Updates under way, by program id – the Toolbox's own under `OWN_ID`.
    #[serde(default)]
    pub updating: BTreeMap<String, Updating>,
}

/// The Toolbox's own id: its manifest lies in its `dist\`, but it does not list itself.
pub const OWN_ID: &str = "toolbox";

// ---------- updates ----------

/// How far an update is (DESIGN.md 9) – shown on the program's page instead of its Update button.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "step", content = "version", rename_all = "snake_case")]
pub enum Updating {
    /// Asking GitHub for the latest release.
    Checking,
    /// Loading this version and checking its signature.
    Downloading(String),
    /// Swapping the files.
    Installing(String),
}

/// Whether a program can be updated from here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Updatable {
    /// Its manifest names no repository or no key: it cannot be updated (a script copied by hand, Last Bastion).
    No,
    /// It is built on this PC (a `dist\` below its sources): the hint "run build.ps1" instead of the button.
    Built,
    Yes,
}

/// What an update needs to know about a program – for one on the list and for the Toolbox itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub id: String,
    pub name: Text,
    /// Where its manifest lies: the zip is unpacked into this folder.
    pub home: PathBuf,
    pub version: String,
    pub repository: Option<String>,
    pub key: Option<String>,
}

impl Target {
    pub fn of(manifest: &Manifest, home: &Path) -> Target {
        Target { id: manifest.id.clone(), name: manifest.name.clone(), home: home.to_path_buf(), version: manifest.version.clone(), repository: manifest.repository.clone(), key: manifest.update_key.clone() }
    }

    pub fn updatable(&self) -> Updatable {
        if self.repository.is_none() || self.key.is_none() {
            Updatable::No
        } else if kit::update::developer_copy(&self.home) {
            Updatable::Built
        } else {
            Updatable::Yes
        }
    }
}

impl Entry {
    pub fn target(&self) -> Target {
        Target::of(&self.manifest, &self.home)
    }
}

// ---------- remembering folders ----------

fn same(a: &Component, b: &Component) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// How `folder` is written into the settings: relative to the Toolbox's folder if the two share a folder
/// somewhere above them – then moving that shared folder, with everything in it, keeps the list valid.
/// Otherwise (another drive, or nothing in common but the drive) the full path.
pub fn store_path(base: &Path, folder: &Path) -> String {
    let (b, f): (Vec<Component>, Vec<Component>) = (base.components().collect(), folder.components().collect());
    let common = b.iter().zip(&f).take_while(|(x, y)| same(x, y)).count();
    if !b[..common].iter().any(|c| matches!(c, Component::Normal(_))) {
        return folder.to_string_lossy().into_owned();
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for part in &f[common..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() { ".".into() } else { out.to_string_lossy().into_owned() }
}

/// The folder a stored path means.
pub fn resolve(base: &Path, stored: &str) -> PathBuf {
    let path = Path::new(stored);
    let full = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    // `absolute` also takes the `..` out, without asking whether the folders exist.
    std::path::absolute(&full).unwrap_or(full)
}

/// The same file, however the two paths are written.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => same_folder(a, b),
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let clean = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    clean(a) == clean(b)
}

// ---------- the list ----------

/// Reads every folder on the list. `old` hands the run state of the programs that were known before over to
/// the new list.
pub fn load(settings: &Settings, base: &Path, old: &[Entry]) -> (Vec<Entry>, Vec<Missing>) {
    let (mut programs, mut missing): (Vec<Entry>, Vec<Missing>) = (vec![], vec![]);
    for stored in &settings.programs {
        let folder = resolve(base, stored);
        let lost = |problem| Missing { stored: stored.clone(), folder: folder.clone(), problem };
        match Manifest::read(&folder) {
            Ok(Some(found)) => {
                if let Some(first) = programs.iter().find(|e| e.id() == found.manifest.id) {
                    missing.push(lost(Problem::SameId(first.manifest.name.get(i18n::FALLBACK).to_string())));
                    continue;
                }
                let kind = found.manifest.kind();
                let run = old.iter().find(|e| e.id() == found.manifest.id).map(|e| e.run.clone()).unwrap_or_default();
                let mut entry = Entry { stored: stored.clone(), folder: folder.clone(), home: found.home, manifest: found.manifest, kind, options: BTreeMap::new(), run, autostart: None };
                entry.read_options();
                entry.read_autostart();
                programs.push(entry);
            }
            Ok(None) => missing.push(lost(Problem::NotFound)),
            Err(e) => missing.push(lost(Problem::Broken(e))),
        }
    }
    (programs, missing)
}

/// Two folders hold a program with the same id; only one can be on the list (4.2).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Conflict {
    pub name: Text,
    pub listed: PathBuf,
    pub new: PathBuf,
}

/// What "Add program…" did.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Added {
    /// Names of the programs that are new on the list.
    pub added: Vec<Text>,
    /// Programs that were on the list already (the same folder).
    pub known: usize,
    pub conflicts: Vec<Conflict>,
    /// Manifests that could not be read.
    pub broken: Vec<String>,
}

/// Adds the program in `folder` – or, if the folder holds none itself, every program one level below it.
pub fn add(settings: &mut Settings, programs: &[Entry], base: &Path, folder: &Path) -> Added {
    let (found, broken) = Manifest::find(folder);
    let mut result = Added { broken, ..Default::default() };
    // Id and folder of everything on the list – including what this very call adds.
    let mut listed: Vec<(String, PathBuf)> = programs.iter().map(|e| (e.id().to_string(), e.folder.clone())).collect();
    for (folder, found) in found {
        if found.manifest.id == OWN_ID {
            continue;
        }
        match listed.iter().find(|(id, _)| id == &found.manifest.id) {
            Some((_, there)) if same_folder(there, &folder) => result.known += 1,
            Some((_, there)) => result.conflicts.push(Conflict { name: found.manifest.name, listed: there.clone(), new: folder }),
            None => {
                settings.programs.push(store_path(base, &folder));
                listed.push((found.manifest.id, folder));
                result.added.push(found.manifest.name);
            }
        }
    }
    result
}

/// The answer to a conflict: this folder takes the place of the one that holds the same program.
pub fn replace(settings: &mut Settings, programs: &[Entry], base: &Path, folder: &Path) -> Result<(), String> {
    let found = Manifest::read(folder)?.ok_or("no program.json")?;
    if let Some(listed) = programs.iter().find(|e| e.id() == found.manifest.id) {
        settings.programs.retain(|s| s != &listed.stored);
    }
    settings.programs.push(store_path(base, folder));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("toolbox-library-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(file: PathBuf, text: &str) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    /// "Start with Windows" is on only for an entry that starts this copy (or an exe that is gone – a moved
    /// folder, repaired at the program's next start) and that Windows has not switched off.
    #[test]
    fn whose_autostart_entry_it_is() {
        let dir = temp("autostart");
        write(dir.join("P.exe"), "");
        let manifest = Manifest::parse(r#"{"id":"plus","name":"Plus","exe":"P.exe","protocol":1,"background":true}"#).unwrap();
        let mut e = Entry { stored: String::new(), folder: dir.clone(), home: dir.clone(), kind: manifest.kind(), manifest, options: BTreeMap::new(), run: Run::default(), autostart: None };
        assert!(e.has_autostart());
        let noted = |state, target: PathBuf| Some(autostart::Entry { state, target: Some(target) });
        assert!(!e.autostart_on() && !e.autostart_is_mine(), "nothing noted");
        // Written another way: the same file.
        e.autostart = noted(Autostart::On, dir.join(".").join("P.exe"));
        assert!(e.autostart_on());
        e.autostart = noted(Autostart::DisabledByWindows, dir.join("P.exe"));
        assert!(!e.autostart_on() && e.autostart_is_mine());
        e.autostart = noted(Autostart::On, PathBuf::from(r"C:\No Such Folder\P.exe"));
        assert!(e.autostart_on(), "an exe that is gone counts as this one");
        e.autostart = noted(Autostart::On, std::env::current_exe().unwrap());
        assert!(!e.autostart_on() && !e.autostart_is_mine(), "another copy's entry");
        // Only background kit programs have the switch.
        let script = Manifest::parse(r#"{"id":"s","name":"S","run":"s.ps1"}"#).unwrap();
        let mut s = Entry { kind: script.kind(), manifest: script, autostart: None, ..e.clone() };
        assert!(!s.has_autostart());
        s.read_autostart();
        assert_eq!(s.autostart, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folders_are_remembered_relative_where_it_helps() {
        let base = Path::new(r"E:\Software\Windows Toolbox\Toolbox\dist");
        let cases = [
            (r"E:\Software\Windows Toolbox\Toolbox\programs\hardware_monitor", r"..\programs\hardware_monitor"),
            (r"E:\Software\Windows Toolbox\Last Bastion", r"..\..\Last Bastion"),
            (r"e:\software\Windows Toolbox\Toolbox\dist\programs\x", r"programs\x"),
            (r"E:\Software\Elsewhere\Program", r"..\..\..\Elsewhere\Program"),
            (r"E:\Games\Somewhere", r"E:\Games\Somewhere"),
            (r"D:\Other Drive\Program", r"D:\Other Drive\Program"),
        ];
        for (folder, stored) in cases {
            assert_eq!(store_path(base, Path::new(folder)), stored);
            assert!(same_folder(&resolve(base, stored), Path::new(folder)), "{stored} must lead back to {folder}");
        }
        // Nothing shared but the drive: moving the Toolbox must not make this point somewhere else.
        assert_eq!(store_path(Path::new(r"E:\Toolbox"), Path::new(r"E:\Games\X")), r"E:\Games\X");
    }

    #[test]
    fn invalid_settings_get_their_defaults() {
        let s = Settings { language: "xx".into(), theme: "pink".into(), zoom: 9.0, programs: vec!["a".into()] }.checked();
        assert_eq!((s.theme.as_str(), s.zoom, s.programs.len()), ("dark", 1.0, 1));
        assert!(i18n::is_language(&s.language));
    }

    #[test]
    fn add_remove_and_the_same_program_twice() {
        let root = temp("add");
        let base = root.join("Toolbox").join("dist");
        write(root.join("Script/program.json"), r#"{"id":"script","name":"Script","run":"a.ps1","options":{"Pos":{"values":["Left","Right"],"default":"Right"}}}"#);
        write(root.join("Game/dist/program.json"), r#"{"id":"game","name":{"de":"Spiel","en":"Game"},"exe":"Game.exe"}"#);
        write(root.join("Toolbox/dist/program.json"), r#"{"id":"toolbox","name":"Windows Toolbox","exe":"Toolbox.exe"}"#);
        write(root.join("Copy/program.json"), r#"{"id":"game","name":"Game (copy)","exe":"Game.exe"}"#);
        write(root.join("Broken/program.json"), "{");

        // One click on the folder of programs: everything below it, except the Toolbox itself.
        let mut settings = Settings::default();
        let added = add(&mut settings, &[], &base, &root);
        // "Copy" and "Game" hold the same id: the first one found is added, the other one is a conflict to ask about.
        assert_eq!(settings.programs, [r"..\..\Copy", r"..\..\Script"]);
        assert_eq!((added.added.len(), added.conflicts.len(), added.broken.len()), (2, 1, 1));
        assert!(same_folder(&added.conflicts[0].listed, &root.join("Copy")));
        let (programs, missing) = load(&settings, &base, &[]);
        assert_eq!(programs.iter().map(Entry::id).collect::<Vec<_>>(), ["game", "script"]);
        assert!(missing.is_empty());

        // Should both ever stand in the file, the second one shows as a problem instead of a second entry.
        let mut both = settings.clone();
        both.programs.push(r"..\..\Game".into());
        assert_eq!(load(&both, &base, &[]).1[0].problem, Problem::SameId("Game (copy)".into()));

        // Adding the other copy by itself is the same conflict; the answer replaces the listed one.
        let added = add(&mut settings, &programs, &base, &root.join("Game"));
        assert_eq!((added.added.len(), added.conflicts.len()), (0, 1));
        replace(&mut settings, &programs, &base, &added.conflicts[0].new).unwrap();
        let (programs, missing) = load(&settings, &base, &programs);
        assert!(missing.is_empty());
        let game = programs.iter().find(|e| e.id() == "game").unwrap();
        assert_eq!((game.manifest.name.get("de"), game.kind), ("Spiel", Kind::Exe));
        assert_eq!(game.exe().unwrap(), root.join("Game").join("dist").join("Game.exe"));
        // Adding what is there already changes nothing.
        assert_eq!(add(&mut settings, &programs, &base, &root.join("Game")).known, 1);

        // A folder that disappears stays on the list, as a problem that can be removed.
        std::fs::remove_dir_all(root.join("Game")).unwrap();
        let (programs, missing) = load(&settings, &base, &programs);
        assert_eq!((programs.len(), &missing[0].problem), (1, &Problem::NotFound));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_script_s_options() {
        let root = temp("options");
        write(root.join("Script/program.json"), r#"{"id":"script","name":"Script","run":"a.ps1","options":{"Pos":{"values":["Left","Right"],"default":"Right"}}}"#);
        write(root.join("Script/data/settings.json"), r#"{"Pos": "Nowhere", "other": 1}"#);
        let settings = Settings { programs: vec![root.join("Script").to_string_lossy().into_owned()], ..Default::default() };
        let (mut programs, _) = load(&settings, &root, &[]);
        let script = &mut programs[0];
        assert_eq!(script.options["Pos"], "Right", "an invalid saved value falls back to the default");
        assert!(script.set_option("Pos", "Up".into()).is_err());
        assert!(script.set_option("Size", "Left".into()).is_err());
        script.set_option("Pos", "Left".into()).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(root.join("Script/data/settings.json")).unwrap()).unwrap();
        assert_eq!(saved, serde_json::json!({"Pos": "Left", "other": 1}));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A script's page as data, and how the script is started.
    #[test]
    fn a_script_s_page_and_command() {
        let root = temp("script");
        let manifest = r#"{"id":"rayman","name":"Rayman","run":"r.ps1","hint":{"de":"Erst im Spiel einstellen","en":"Set it in the game first"},
            "options":{"Pos":{"values":["Left","Center"],"default":"Center","label":{"de":"Position","en":"Position"},"value_labels":{"Left":{"de":"Links","en":"Left"}}}}}"#;
        write(root.join("New Layout/program.json"), manifest);
        write(root.join("New Layout/r.ps1"), "");

        let new = Settings { programs: vec![root.join("New Layout").to_string_lossy().into_owned()], ..Default::default() };
        let (programs, _) = load(&new, &root, &[]);
        let page = serde_json::to_value(script_page(&programs[0], "de")).unwrap();
        assert_eq!(page["blocks"][0], serde_json::json!({"type": "text", "text": "Erst im Spiel einstellen"}));
        assert_eq!(page["blocks"][1]["label"], "Position");
        assert_eq!(page["blocks"][1]["value"], "Center");
        assert_eq!(page["blocks"][1]["options"], serde_json::json!([{"value": "Left", "label": "Links"}, {"value": "Center", "label": "Center"}]));
        let command = script_command(&programs[0]).unwrap();
        assert!(command.starts_with("powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \""), "{command}");
        assert!(command.ends_with("New Layout\\r.ps1\""), "the script reads its options from its file: {command}");

        // What cannot be started says why.
        let mut gone = programs[0].clone();
        gone.manifest.run = Some("nothing.ps1".into());
        assert!(script_command(&gone).unwrap_err().contains("missing"));
        gone.manifest.run = Some("r.cmd".into());
        assert!(script_command(&gone).unwrap_err().contains(".ps1"));
        std::fs::remove_dir_all(&root).ok();
    }
}
