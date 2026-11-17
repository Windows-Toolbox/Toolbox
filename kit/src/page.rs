//! A settings page as data (DESIGN.md 4.4, 15.2): the program builds it from its settings, the Toolbox – or the
//! program's own settings window – draws it (`ui::PageView`).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::proto::Health;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Page {
    pub blocks: Vec<Block>,
}

/// The building blocks. A `key` is a field of the program's settings struct; `value` is its current value.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Section {
        title: String,
        blocks: Vec<Block>,
    },
    Text {
        text: String,
    },
    /// A line with a coloured dot: ok / warn / bad.
    Status {
        text: String,
        state: Health,
    },
    Switch {
        label: String,
        key: String,
        value: bool,
    },
    Slider {
        label: String,
        key: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        unit: String,
    },
    Choice {
        label: String,
        key: String,
        value: Value,
        options: Vec<ChoiceOption>,
    },
    /// A line of text to type in. `secret`: shown as dots (a password, a client secret).
    Entry {
        label: String,
        key: String,
        value: String,
        #[serde(default)]
        secret: bool,
    },
    Buttons {
        buttons: Vec<Button>,
    },
    /// A block this kit does not know yet – drawn as "needs a newer Toolbox".
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChoiceOption {
    pub value: Value,
    pub label: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Button {
    pub label: String,
    pub action: String,
}

/// Builds a page. It knows the settings as JSON, so a block looks its value up by key.
pub struct Builder<'a> {
    settings: &'a Value,
    blocks: Vec<Block>,
    /// Every key named so far – for the test "every key is a settings field" (DESIGN.md 13).
    keys: Vec<String>,
}

impl<'a> Builder<'a> {
    pub fn new(settings: &'a Value) -> Self {
        Self { settings, blocks: vec![], keys: vec![] }
    }

    fn value(&mut self, key: &str) -> Value {
        self.keys.push(key.to_string());
        self.settings.get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn section(&mut self, title: impl Into<String>, f: impl FnOnce(&mut Builder)) {
        let mut inner = Builder::new(self.settings);
        f(&mut inner);
        self.keys.append(&mut inner.keys);
        self.blocks.push(Block::Section { title: title.into(), blocks: inner.blocks });
    }

    pub fn text(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Text { text: text.into() });
    }

    pub fn status(&mut self, text: impl Into<String>, state: Health) {
        self.blocks.push(Block::Status { text: text.into(), state });
    }

    pub fn switch(&mut self, label: impl Into<String>, key: &str) {
        let value = self.value(key).as_bool().unwrap_or(false);
        self.blocks.push(Block::Switch { label: label.into(), key: key.into(), value });
    }

    pub fn slider(&mut self, label: impl Into<String>, key: &str, min: f64, max: f64, step: f64, unit: &str) {
        let value = self.value(key).as_f64().unwrap_or(min);
        self.blocks.push(Block::Slider { label: label.into(), key: key.into(), value, min, max, step, unit: unit.into() });
    }

    /// One button per option: `p.choice("Refresh", "rate", &[(0.5, "0.5 s"), (1.0, "1 s")])`.
    pub fn choice<V: Serialize, L: AsRef<str>>(&mut self, label: impl Into<String>, key: &str, options: &[(V, L)]) {
        let value = self.value(key);
        let options = options
            .iter()
            .map(|(v, l)| ChoiceOption { value: serde_json::to_value(v).unwrap_or(Value::Null), label: l.as_ref().into() })
            .collect();
        self.blocks.push(Block::Choice { label: label.into(), key: key.into(), value, options });
    }

    /// A field for a line of text. The Toolbox sends `set` when the field is left or Enter is pressed – not per
    /// keystroke.
    pub fn entry(&mut self, label: impl Into<String>, key: &str) {
        let value = self.value(key).as_str().unwrap_or_default().to_string();
        self.blocks.push(Block::Entry { label: label.into(), key: key.into(), value, secret: false });
    }

    /// Like `entry`, but what is typed shows as dots.
    pub fn secret(&mut self, label: impl Into<String>, key: &str) {
        let value = self.value(key).as_str().unwrap_or_default().to_string();
        self.blocks.push(Block::Entry { label: label.into(), key: key.into(), value, secret: true });
    }

    /// A row of buttons: `p.buttons(|b| b.button("Open log", "open_log"));`
    pub fn buttons(&mut self, f: impl FnOnce(&mut ButtonRow)) {
        let mut row = ButtonRow { buttons: vec![] };
        f(&mut row);
        self.blocks.push(Block::Buttons { buttons: row.buttons });
    }

    pub fn finish(self) -> Page {
        Page { blocks: self.blocks }
    }
}

pub struct ButtonRow {
    buttons: Vec<Button>,
}
impl ButtonRow {
    pub fn button(&mut self, label: impl Into<String>, action: &str) -> &mut Self {
        self.buttons.push(Button { label: label.into(), action: action.into() });
        self
    }
}

/// Applies `set key value` to a settings struct – through JSON, so every field works without code per field.
/// Refused (`Err` with the reason) if the key is no field or the value does not fit the field's type.
pub fn apply_set<T: Serialize + DeserializeOwned>(settings: &T, key: &str, value: Value) -> Result<T, String> {
    let mut v = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    let obj = v.as_object_mut().ok_or("settings are no object")?;
    if !obj.contains_key(key) {
        return Err(format!("unknown setting '{key}'"));
    }
    // A slider sends a float; an integer field takes it when it is whole.
    let value = match (&obj[key], &value) {
        (Value::Number(old), Value::Number(new)) if (old.is_i64() || old.is_u64()) && new.is_f64() => {
            let f = new.as_f64().unwrap_or(0.0);
            if f.fract() == 0.0 { Value::from(f as i64) } else { value }
        }
        _ => value,
    };
    obj.insert(key.to_string(), value);
    serde_json::from_value(v).map_err(|e| format!("'{key}': {e}"))
}

/// The test every program calls (DESIGN.md 13): every key the page names is a field of the settings –
/// a typo would otherwise be a switch that does nothing.
pub fn check_keys<T: Serialize + Default>(page: impl FnOnce(&mut Builder)) -> Result<(), String> {
    let v = serde_json::to_value(T::default()).map_err(|e| e.to_string())?;
    let mut b = Builder::new(&v);
    page(&mut b);
    let missing: Vec<_> = b.keys.iter().filter(|k| v.get(k.as_str()).is_none()).cloned().collect();
    if missing.is_empty() { Ok(()) } else { Err(format!("the page names keys that are no settings field: {missing:?}")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, Default)]
    #[serde(default)]
    struct S {
        on: bool,
        level: u32,
        rate: f32,
    }

    #[test]
    fn a_page_is_plain_data() {
        let v = serde_json::json!({"on": true, "level": 3, "rate": 2.0});
        let mut b = Builder::new(&v);
        b.section("A", |p| {
            p.text("hello");
            p.switch("On", "on");
            p.slider("Level", "level", 0.0, 10.0, 1.0, "x");
            p.choice("Rate", "rate", &[(0.5, "half"), (2.0, "two")]);
            p.buttons(|r| {
                r.button("Go", "go").button("Stop", "stop");
            });
        });
        let json = serde_json::to_value(b.finish()).unwrap();
        let blocks = &json["blocks"][0]["blocks"];
        assert_eq!(json["blocks"][0]["type"], "section");
        assert_eq!(blocks[1], serde_json::json!({"type": "switch", "label": "On", "key": "on", "value": true}));
        assert_eq!(blocks[2]["value"], 3.0);
        assert_eq!(blocks[3]["options"][1], serde_json::json!({"value": 2.0, "label": "two"}));
        assert_eq!(blocks[4]["buttons"][1]["action"], "stop");

        let v = serde_json::json!({"name": "Anna", "token": "s3cret"});
        let mut b = Builder::new(&v);
        b.entry("Name", "name");
        b.secret("Token", "token");
        let json = serde_json::to_value(b.finish()).unwrap();
        assert_eq!(json["blocks"][0], serde_json::json!({"type": "entry", "label": "Name", "key": "name", "value": "Anna", "secret": false}));
        assert_eq!(json["blocks"][1]["secret"], true);
    }

    #[test]
    fn an_unknown_block_does_not_break_the_page() {
        let json = r#"{"blocks":[{"type":"text","text":"a"},{"type":"graph","points":[1,2]}]}"#;
        let p: Page = serde_json::from_str(json).unwrap();
        assert_eq!(p.blocks[1], Block::Unknown);
    }

    #[test]
    fn set_converts_and_refuses() {
        let s = S::default();
        assert_eq!(apply_set(&s, "level", serde_json::json!(3.0)).unwrap().level, 3);
        assert!(apply_set(&s, "level", serde_json::json!(2.5)).is_err());
        assert!(apply_set(&s, "level", serde_json::json!(-1.0)).is_err());
        assert!(apply_set(&s, "nope", serde_json::json!(1)).is_err());
        assert!(apply_set(&s, "on", serde_json::json!(true)).unwrap().on);
        assert_eq!(apply_set(&s, "rate", serde_json::json!(1)).unwrap().rate, 1.0);
    }

    #[test]
    fn keys_are_checked() {
        assert!(check_keys::<S>(|p| p.switch("x", "on")).is_ok());
        assert!(check_keys::<S>(|p| p.section("s", |p| p.switch("x", "onn"))).is_err());
    }
}
