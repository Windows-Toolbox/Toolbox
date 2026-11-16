//! Texts in several languages (DESIGN.md 5). The table is `assets\i18n.json` of the program – one block per id,
//! inside it one line per language – and the code only holds the id: `t!("page.status")`.
//!
//! ```json
//! { "status.running": { "de": "Läuft", "en": "Running" },
//!   "toast.zoom":     { "de": "Zoom {percent} %", "en": "Zoom {percent} %" } }
//! ```
//!
//! `t!("toast.zoom", percent = 110)` fills `{percent}`. A missing translation falls back to English, a missing id
//! to the id itself – nothing crashes over an empty cell; the test `check_ids` finds both before a user does.
//! Release builds compile the file in; debug builds read it from `assets\` at start, so a text can be changed
//! without compiling.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{LazyLock, RwLock};

/// Code and name (in the language itself) of every language – the Toolbox builds its buttons from this list.
/// A new language is one line here and one line per id in every `i18n.json`.
pub const LANGUAGES: &[(&str, &str)] = &[("de", "Deutsch"), ("en", "English")];
/// The language used when a translation is missing.
pub const FALLBACK: &str = "en";

type Table = BTreeMap<String, BTreeMap<String, String>>;

struct State {
    language: String,
    table: Table,
}

static STATE: LazyLock<RwLock<State>> = LazyLock::new(|| {
    let mut table = Table::new();
    merge(&mut table, include_str!("../assets/i18n.json"));
    RwLock::new(State { language: system_language().to_string(), table })
});

fn merge(table: &mut Table, json: &str) {
    if let Ok(more) = serde_json::from_str::<Table>(json.trim_start_matches('\u{feff}')) {
        table.extend(more);
    }
}

/// Adds the program's table to the kit's own. Called through `kit::texts!()`.
pub fn init(compiled: &str, file: &str) {
    let on_disk = if cfg!(debug_assertions) { std::fs::read_to_string(file).ok() } else { None };
    load(on_disk.as_deref().unwrap_or(compiled));
}

/// Adds a table (the content of an `i18n.json`).
pub fn load(json: &str) {
    merge(&mut STATE.write().unwrap_or_else(|e| e.into_inner()).table, json);
}

/// Loads `assets\i18n.json` of the program: `kit::texts!();` first thing in `main`.
#[macro_export]
macro_rules! texts {
    () => {
        $crate::i18n::init(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/i18n.json")),
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/i18n.json"),
        )
    };
}

/// The text with this id in the current language: `t!("id")`, or `t!("id", name = value)` to fill `{name}`.
#[macro_export]
macro_rules! t {
    ($id:expr) => {
        $crate::i18n::text($id)
    };
    ($id:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::format($id, &[$((stringify!($name), ($value).to_string())),+])
    };
}

pub fn is_language(code: &str) -> bool {
    LANGUAGES.iter().any(|(c, _)| *c == code)
}

/// The Windows display language if it is one of `LANGUAGES`, else `FALLBACK`.
pub fn system_language() -> &'static str {
    // The low ten bits are the language without the country: 0x07 German, 0x09 English.
    let primary = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff;
    let code = match primary {
        0x07 => "de",
        _ => FALLBACK,
    };
    if is_language(code) { code } else { FALLBACK }
}

pub fn language() -> String {
    STATE.read().unwrap_or_else(|e| e.into_inner()).language.clone()
}

/// Switches the language for this process. `true` if it changed.
pub fn set_language(language: &str) -> bool {
    let mut s = STATE.write().unwrap_or_else(|e| e.into_inner());
    if !is_language(language) || s.language == language {
        return false;
    }
    s.language = language.to_string();
    true
}

pub fn has(id: &str) -> bool {
    STATE.read().unwrap_or_else(|e| e.into_inner()).table.contains_key(id)
}

pub fn text(id: &str) -> String {
    let s = STATE.read().unwrap_or_else(|e| e.into_inner());
    let row = s.table.get(id);
    row.and_then(|r| r.get(&s.language).filter(|t| !t.is_empty()).or_else(|| r.get(FALLBACK))).cloned().unwrap_or_else(|| id.to_string())
}

pub fn format(id: &str, values: &[(&str, String)]) -> String {
    let mut text = text(id);
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The test every program calls (DESIGN.md 13), with `env!("CARGO_MANIFEST_DIR")`: every id used as `t!("…")` in
/// `src\` exists in `assets\i18n.json`, and every id there has a text in every language.
pub fn check_ids(crate_dir: &str) -> Result<(), String> {
    let dir = Path::new(crate_dir);
    let file = dir.join("assets").join("i18n.json");
    let json = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let table: Table = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut problems = vec![];
    for (id, row) in &table {
        for (code, _) in LANGUAGES {
            if row.get(*code).is_none_or(|t| t.is_empty()) {
                problems.push(format!("'{id}' has no text for '{code}'"));
            }
        }
    }
    let mut sources = vec![];
    collect_sources(&dir.join("src"), &mut sources);
    let mut kit = Table::new();
    merge(&mut kit, include_str!("../assets/i18n.json"));
    for source in sources {
        let code = std::fs::read_to_string(&source).unwrap_or_default();
        for id in used_ids(&code) {
            if !table.contains_key(&id) && !kit.contains_key(&id) {
                problems.push(format!("'{id}' is used in {} but is not in i18n.json", source.display()));
            }
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
}

fn collect_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every `"id"` that directly follows `t!(` – in the code itself: comments (their examples) and the tests at the
/// end of a file are left out.
fn used_ids(code: &str) -> Vec<String> {
    let mut ids = vec![];
    for line in code.lines().take_while(|l| !l.contains("#[cfg(test)]")).filter(|l| !l.trim_start().starts_with("//")) {
        let mut rest = line;
        while let Some(pos) = rest.find("t!(") {
            // `t!(` must be the whole macro name, not the end of another one (`format!(`, `assert!(`) and not
            // the inside of a string.
            let before = rest[..pos].chars().next_back();
            rest = &rest[pos + 3..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"') {
                continue;
            }
            let Some(start) = rest.trim_start().strip_prefix('"') else { continue };
            if let Some(end) = start.find('"') {
                ids.push(start[..end].to_string());
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_and_placeholders() {
        load(r#"{"test.only_en": {"en": "English only"}, "test.zoom": {"de": "Zoom {percent} % ({percent})", "en": "Zoom {percent} %"}}"#);
        set_language("de");
        assert_eq!(t!("test.only_en"), "English only");
        assert_eq!(t!("test.zoom", percent = 110), "Zoom 110 % (110)");
        assert_eq!(t!("test.missing"), "test.missing");
        assert!(!set_language("xx"), "an unknown language is refused");
        assert_eq!(language(), "de");
    }

    #[test]
    fn ids_are_found_in_code() {
        let code = "let a = t!(\"one\"); let b = kit::t!( \"two\", n = 1); assert!(\"x\"); format!(\"{}\", t!(\"three\")); let c = t!(variable);\n\
                    // an example in a comment: t!(\"comment\")\n\
                    let d = rest.find(\"t!(\");\n\
                    #[cfg(test)]\n\
                    let e = t!(\"in_a_test\");";
        assert_eq!(used_ids(code), ["one", "two", "three"]);
    }

    #[test]
    fn the_kit_s_own_texts_are_complete() {
        check_ids(env!("CARGO_MANIFEST_DIR")).unwrap();
    }
}
