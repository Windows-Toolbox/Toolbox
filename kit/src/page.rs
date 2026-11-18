//! A settings page as data (DESIGN.md 4.4, 15.2): the program builds it from its settings, the Toolbox – or the
//! program's own settings window – draws it (`ui::PageView`).

use std::collections::BTreeMap;

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
        /// Open when the page is first shown (the user may fold it either way). Missing = open – pages from before
        /// this field.
        #[serde(default = "yes", skip_serializing_if = "is_yes")]
        open: bool,
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
    /// Many entries of one kind (Screen Zones: the games) – flat or in groups, opening under the row or beside the
    /// list, with a filter field, chips per group and sorting, all worked out by the renderer itself (DESIGN.md 15.2).
    List(List),
    /// A block this kit does not know yet – drawn as "needs a newer Toolbox".
    #[serde(other)]
    Unknown,
}

fn yes() -> bool {
    true
}
fn is_yes(v: &bool) -> bool {
    *v
}

/// The list block. Everything but `items` may stay empty.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct List {
    /// Tells two lists apart – the renderer remembers view, filter, sort and the open entry under it.
    pub key: String,
    pub items: Vec<Item>,
    /// In their order: the sort "active first" goes by it, the chips and the view Group show it.
    pub groups: Vec<Group>,
    /// Sorts of the program's own, besides "active first" (with groups) and A–Z.
    pub sorts: Vec<Sort>,
    /// A button above the list that unfolds these blocks (Screen Zones: the open windows to add).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<Add>,
    /// Shown while the list has no entry at all.
    pub empty: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Item {
    /// Stable across pushes (Screen Zones: the exe) – the open entry and the chosen one are remembered by it.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// The dot in front of the entry …
    pub state: Health,
    /// … and what it means: its tooltip, and the line under the title in the view Detail.
    pub text: String,
    /// The `id` of one of the list's `groups`.
    pub group: String,
    /// Short labels at the right of the row (Screen Zones: the zone); the filter searches them too.
    pub tags: Vec<String>,
    /// One button in the row itself (Screen Zones: Play).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<Button>,
    /// The entry's place in each of the list's `sorts`, by sort id: smaller first; missing = last.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub order: BTreeMap<String, f64>,
    /// What opens under the row – or beside the list with Detail.
    pub blocks: Vec<Block>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Group {
    pub id: String,
    pub label: String,
    /// Open when the list is first shown in groups.
    pub open: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Sort {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Add {
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChoiceOption {
    pub value: Value,
    pub label: String,
    /// A mark on the option: `warn` / `bad` draw a thin frame in that colour around its button (Screen Zones: a key
    /// combination another program has taken). Without it, a plain button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Health>,
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
        lookup(self.settings, key).cloned().unwrap_or(Value::Null)
    }

    pub fn section(&mut self, title: impl Into<String>, f: impl FnOnce(&mut Builder)) {
        let blocks = self.inner(f);
        self.blocks.push(Block::Section { title: title.into(), blocks, open: true });
    }

    /// A section that starts folded – for what is seldom needed.
    pub fn section_closed(&mut self, title: impl Into<String>, f: impl FnOnce(&mut Builder)) {
        let blocks = self.inner(f);
        self.blocks.push(Block::Section { title: title.into(), blocks, open: false });
    }

    /// Blocks built apart (a section's, an entry's) – their keys still count for `check_keys`.
    fn inner(&mut self, f: impl FnOnce(&mut Builder)) -> Vec<Block> {
        let mut inner = Builder::new(self.settings);
        f(&mut inner);
        self.keys.append(&mut inner.keys);
        inner.blocks
    }

    /// A list of entries: `p.list("games", |l| { l.group(..); l.item(Item { .. }, |p| p.switch(..)); })`.
    pub fn list(&mut self, key: &str, f: impl FnOnce(&mut ListBuilder)) {
        let mut l = ListBuilder { page: Builder::new(self.settings), list: List { key: key.into(), ..List::default() } };
        f(&mut l);
        self.keys.append(&mut l.page.keys);
        self.blocks.push(Block::List(l.list));
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
        let marked: Vec<(&V, &L, Option<Health>)> = options.iter().map(|(v, l)| (v, l, None)).collect();
        self.choice_marked(label, key, &marked);
    }

    /// Like `choice`, with a mark per option (`Some(Health::Bad)`: a thin red frame around its button).
    pub fn choice_marked<V: Serialize, L: AsRef<str>>(&mut self, label: impl Into<String>, key: &str, options: &[(V, L, Option<Health>)]) {
        let value = self.value(key);
        let options = options
            .iter()
            .map(|(v, l, state)| ChoiceOption { value: serde_json::to_value(v).unwrap_or(Value::Null), label: l.as_ref().into(), state: *state })
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

pub struct ListBuilder<'a> {
    page: Builder<'a>,
    list: List,
}

impl ListBuilder<'_> {
    pub fn group(&mut self, id: &str, label: impl Into<String>, open: bool) -> &mut Self {
        self.list.groups.push(Group { id: id.into(), label: label.into(), open });
        self
    }

    pub fn sort(&mut self, id: &str, label: impl Into<String>) -> &mut Self {
        self.list.sorts.push(Sort { id: id.into(), label: label.into() });
        self
    }

    pub fn empty(&mut self, text: impl Into<String>) -> &mut Self {
        self.list.empty = text.into();
        self
    }

    /// The button above the list and what it unfolds.
    pub fn add(&mut self, label: impl Into<String>, f: impl FnOnce(&mut Builder)) {
        let blocks = self.page.inner(f);
        self.list.add = Some(Add { label: label.into(), blocks });
    }

    /// One entry; `f` builds what opens with it. `item.blocks` is replaced by them.
    pub fn item(&mut self, item: Item, f: impl FnOnce(&mut Builder)) {
        let blocks = self.page.inner(f);
        self.list.items.push(Item { blocks, ..item });
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

/// The value a key names. A key is a field of the settings – or a path into them, its parts joined by `.`: a
/// field of a field, or an element of a list by its number (`games.0.zone`: the field `zone` of the first entry of
/// the list `games`). So a program with a list of things gives each entry its own switches.
pub fn lookup<'v>(settings: &'v Value, key: &str) -> Option<&'v Value> {
    key.split('.').try_fold(settings, |v, part| match v {
        Value::Array(list) => list.get(part.parse::<usize>().ok()?),
        _ => v.get(part),
    })
}

fn lookup_mut<'v>(settings: &'v mut Value, key: &str) -> Option<&'v mut Value> {
    key.split('.').try_fold(settings, |v, part| match v {
        Value::Array(list) => list.get_mut(part.parse::<usize>().ok()?),
        _ => v.get_mut(part),
    })
}

/// Applies `set key value` to a settings struct – through JSON, so every field works without code per field.
/// Refused (`Err` with the reason) if the key is no field (a path: no field of that entry, or no such entry) or
/// the value does not fit the field's type.
pub fn apply_set<T: Serialize + DeserializeOwned>(settings: &T, key: &str, value: Value) -> Result<T, String> {
    let mut v = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    if !v.is_object() {
        return Err("settings are no object".into());
    }
    let field = lookup_mut(&mut v, key).ok_or_else(|| format!("unknown setting '{key}'"))?;
    // A slider sends a float; an integer field takes it when it is whole.
    let value = match (&*field, &value) {
        (Value::Number(old), Value::Number(new)) if (old.is_i64() || old.is_u64()) && new.is_f64() => {
            let f = new.as_f64().unwrap_or(0.0);
            if f.fract() == 0.0 { Value::from(f as i64) } else { value }
        }
        _ => value,
    };
    *field = value;
    serde_json::from_value(v).map_err(|e| format!("'{key}': {e}"))
}

/// The test every program calls (DESIGN.md 13): every key the page names is a field of the settings –
/// a typo would otherwise be a switch that does nothing.
pub fn check_keys<T: Serialize + Default>(page: impl FnOnce(&mut Builder)) -> Result<(), String> {
    check_keys_with(&T::default(), page)
}

/// The same for given settings – for a page whose keys point into a list (`games.0.zone`): with an empty list
/// the default settings would not show them.
pub fn check_keys_with<T: Serialize>(settings: &T, page: impl FnOnce(&mut Builder)) -> Result<(), String> {
    let v = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    let mut b = Builder::new(&v);
    page(&mut b);
    let missing: Vec<_> = b.keys.iter().filter(|k| lookup(&v, k).is_none()).cloned().collect();
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
        assert_eq!(blocks[3]["options"][1], serde_json::json!({"value": 2.0, "label": "two"}), "no mark, no field");
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
    fn a_choice_can_mark_an_option() {
        let v = serde_json::json!({"keys": "a"});
        let mut b = Builder::new(&v);
        b.choice_marked("Keys", "keys", &[("a", "A", None), ("b", "B", Some(Health::Bad))]);
        let json = serde_json::to_value(b.finish()).unwrap();
        assert_eq!(json["blocks"][0]["options"][1], serde_json::json!({"value": "b", "label": "B", "state": "bad"}));
        // A page from an older program, without marks, still reads.
        let p: Page = serde_json::from_str(r#"{"blocks":[{"type":"choice","label":"","key":"k","value":1,"options":[{"value":1,"label":"x"}]}]}"#).unwrap();
        assert!(matches!(&p.blocks[0], Block::Choice { options, .. } if options[0].state.is_none()));
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

    #[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
    #[serde(default)]
    struct Game {
        zone: String,
        delay: u32,
    }
    #[derive(Serialize, Deserialize, Default)]
    #[serde(default)]
    struct WithList {
        games: Vec<Game>,
    }

    #[test]
    fn a_key_can_point_into_a_list() {
        let s = WithList { games: vec![Game { zone: "left".into(), delay: 3 }, Game { zone: "right".into(), delay: 0 }] };
        let v = serde_json::to_value(&s).unwrap();
        let mut b = Builder::new(&v);
        b.choice("Zone", "games.1.zone", &[("left", "Left"), ("right", "Right")]);
        assert_eq!(serde_json::to_value(b.finish()).unwrap()["blocks"][0]["value"], "right");

        let s = apply_set(&s, "games.1.zone", serde_json::json!("left")).unwrap();
        assert_eq!(s.games[1].zone, "left");
        // The slider rule holds inside a list too.
        assert_eq!(apply_set(&s, "games.0.delay", serde_json::json!(5.0)).unwrap().games[0].delay, 5);
        assert!(apply_set(&s, "games.2.zone", serde_json::json!("left")).is_err(), "no such entry");
        assert!(apply_set(&s, "games.x.zone", serde_json::json!("left")).is_err());
        assert!(apply_set(&s, "games.0.nope", serde_json::json!(1)).is_err());
        assert!(apply_set(&s, "games.0.delay", serde_json::json!("soon")).is_err());

        assert!(check_keys_with(&s, |p| p.switch("x", "games.1.zone")).is_ok());
        assert!(check_keys_with(&s, |p| p.switch("x", "games.2.zone")).is_err());
        assert!(check_keys::<WithList>(|p| p.switch("x", "games.0.zone")).is_err(), "the default list is empty");
    }

    #[test]
    fn a_list_is_plain_data() {
        let s = WithList { games: vec![Game { zone: "left".into(), delay: 3 }] };
        let v = serde_json::to_value(&s).unwrap();
        let mut b = Builder::new(&v);
        b.list("games", |l| {
            l.group("active", "Active", true).group("other", "Others", false).sort("last", "Last played").empty("None yet");
            l.add("Add", |p| p.text("pick one"));
            let item = Item { id: "a.exe".into(), title: "A".into(), group: "active".into(), tags: vec!["left".into()], order: [("last".to_string(), 1.0)].into(), ..Item::default() };
            l.item(item, |p| p.choice("Zone", "games.0.zone", &[("left", "Left")]));
        });
        b.section_closed("More", |p| p.text("x"));
        let page = b.finish();
        let json = serde_json::to_value(&page).unwrap();
        let list = &json["blocks"][0];
        assert_eq!((list["type"].as_str(), list["key"].as_str()), (Some("list"), Some("games")));
        assert_eq!(list["items"][0]["blocks"][0]["value"], "left", "an entry's blocks know their values");
        assert_eq!(list["items"][0]["order"]["last"], 1.0);
        assert_eq!(json["blocks"][1]["open"], false);
        assert_eq!(serde_json::from_value::<Page>(json).unwrap(), page, "and back");
        // The keys inside entries and inside `add` are checked.
        assert!(check_keys_with(&s, |p| p.list("g", |l| l.item(Item::default(), |p| p.switch("x", "games.1.zone")))).is_err());
        assert!(check_keys_with(&s, |p| p.list("g", |l| l.add("+", |p| p.switch("x", "nope")))).is_err());
        // A section from an older program is open; an open one is sent as before.
        let p: Page = serde_json::from_str(r#"{"blocks":[{"type":"section","title":"S","blocks":[]}]}"#).unwrap();
        assert!(matches!(p.blocks[0], Block::Section { open: true, .. }));
        assert!(serde_json::to_value(&p).unwrap()["blocks"][0].get("open").is_none());
    }

    #[test]
    fn keys_are_checked() {
        assert!(check_keys::<S>(|p| p.switch("x", "on")).is_ok());
        assert!(check_keys::<S>(|p| p.section("s", |p| p.switch("x", "onn"))).is_err());
    }
}
