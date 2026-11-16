//! `Settings<T>`: a settings struct in a JSON file (DESIGN.md 5). Missing fields get their default, invalid ones
//! fall back one by one, unknown ones are kept on saving (a newer version of the program may have written them).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub struct Settings<T> {
    pub path: PathBuf,
    pub value: T,
    /// Did the file exist when it was loaded?
    pub existed: bool,
    unknown: Map<String, Value>,
}

impl<T: Serialize + DeserializeOwned + Default> Settings<T> {
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let text = std::fs::read_to_string(&path).ok();
        let existed = text.is_some();
        // A file written by an editor may start with a byte order mark.
        let file: Map<String, Value> = text.and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or_default();
        let (value, unknown) = Self::from_map(file);
        Self { path, value, existed, unknown }
    }

    fn from_map(file: Map<String, Value>) -> (T, Map<String, Value>) {
        let mut good = match serde_json::to_value(T::default()) {
            Ok(Value::Object(m)) => m,
            _ => return (T::default(), file),
        };
        let mut unknown = Map::new();
        for (k, v) in file {
            if !good.contains_key(&k) {
                unknown.insert(k, v);
                continue;
            }
            let old = good.insert(k.clone(), v);
            if serde_json::from_value::<T>(Value::Object(good.clone())).is_err() {
                // This field's value does not fit: keep the default for it.
                if let Some(old) = old {
                    good.insert(k, old);
                }
            }
        }
        (serde_json::from_value(Value::Object(good)).unwrap_or_default(), unknown)
    }

    /// Written aside and renamed: a crash in the middle never leaves half a file.
    pub fn save(&self) -> std::io::Result<()> {
        let mut m = match serde_json::to_value(&self.value) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        for (k, v) in &self.unknown {
            m.entry(k.clone()).or_insert(v.clone());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(m))?)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// A value next to the struct's fields, kept in the same file – the kit remembers the Toolbox's language and
    /// look there (`_toolbox`).
    pub fn extra(&self, key: &str) -> Option<&Value> {
        self.unknown.get(key)
    }

    pub fn set_extra(&mut self, key: &str, value: Value) {
        self.unknown.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    #[serde(default)]
    struct S {
        a: u32,
        b: bool,
        c: String,
    }
    impl Default for S {
        fn default() -> Self {
            Self { a: 7, b: false, c: "x".into() }
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kit-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_unknown_invalid() {
        let dir = temp("a");
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"a": "nope", "b": true, "future": [1, 2]}"#).unwrap();
        let mut s = Settings::<S>::load(&path);
        assert!(s.existed);
        assert_eq!(s.value, S { a: 7, b: true, c: "x".into() });
        assert_eq!(s.extra("future"), Some(&serde_json::json!([1, 2])));
        s.set_extra("_toolbox", serde_json::json!({"language": "de"}));
        s.save().unwrap();
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["future"], serde_json::json!([1, 2]));
        assert_eq!(back["_toolbox"]["language"], "de");
        assert_eq!(back["a"], 7);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_file_and_a_broken_file_give_the_defaults() {
        let dir = temp("b");
        let s = Settings::<S>::load(dir.join("none.json"));
        assert!(!s.existed);
        assert_eq!(s.value, S::default());
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        assert_eq!(Settings::<S>::load(dir.join("broken.json")).value, S::default());
        std::fs::write(dir.join("bom.json"), "\u{feff}{\"a\": 9}").unwrap();
        assert_eq!(Settings::<S>::load(dir.join("bom.json")).value.a, 9);
        std::fs::remove_dir_all(&dir).ok();
    }
}
