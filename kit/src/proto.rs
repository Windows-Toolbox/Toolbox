//! The messages on the pipe (DESIGN.md 4.3, 15.1). Every message is one JSON object with a `type`. Unknown types
//! and unknown fields are ignored by both sides, so old and new programs and Toolboxes can talk to each other.
//! Nothing is ever answered: a program sends its state whenever it changes, the other side sends commands.

use serde::{Deserialize, Serialize};

use crate::page::Page;

pub use crate::PROTOCOL;

/// Sent to a program (from the agent or the window).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Hello(ToolboxHello),
    Set { key: String, value: serde_json::Value },
    Action { name: String },
    Start,
    Stop,
    Show,
    Quit,
    #[serde(other)]
    Unknown,
}

/// The first message on a connection; sent again when language, look or `want` change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct ToolboxHello {
    pub protocol: u32,
    pub language: String,
    pub theme: String,
    pub zoom: f32,
    pub want: Vec<Want>,
}
impl Default for ToolboxHello {
    fn default() -> Self {
        Self { protocol: PROTOCOL, language: "en".into(), theme: "dark".into(), zoom: 1.0, want: vec![] }
    }
}

/// The states a connection is interested in.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    Status,
    Page,
    Tray,
    #[serde(other)]
    Unknown,
}

/// Sent by a program: its `hello`, then states – once after `hello` and again on every change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum State {
    Hello {
        protocol: u32,
        id: String,
        version: String,
        pid: u32,
    },
    Status(Status),
    Page(Page),
    Tray(Tray),
    Toast {
        text: String,
    },
    Bye,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Status {
    /// Are the workers switched on (DESIGN.md 5, roles)?
    pub running: bool,
    pub state: Health,
    /// What the Toolbox shows under the name; may be empty.
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    #[default]
    Ok,
    Warn,
    Bad,
    #[serde(other)]
    Unknown,
}

/// What a program wants in the tray (DESIGN.md 6): a status line, its menu entries, optionally live displays.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Tray {
    pub line: String,
    pub menu: Vec<MenuEntry>,
    /// At most 3; each becomes one extra tray icon of the Toolbox.
    pub live: Vec<Live>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct MenuEntry {
    pub label: String,
    pub action: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Live {
    /// At most 3 characters.
    pub text: String,
    pub state: Health,
}

/// Sent to the agent's pipe `windows-toolbox.toolbox` (no `hello` needed).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentCommand {
    /// Open the window – on this program's page if given.
    Show {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// A program announces that its workers run; the agent then connects to it.
    Started { id: String },
    /// End the agent – with `all`, every background program first.
    Quit {
        #[serde(default)]
        all: bool,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_examples_of_the_design() {
        let hello: Command =
            serde_json::from_str(r#"{"type":"hello","protocol":1,"language":"de","theme":"dark","zoom":1.0,"want":["status","page"]}"#).unwrap();
        let Command::Hello(h) = hello else { panic!() };
        assert_eq!((h.language.as_str(), h.want), ("de", vec![Want::Status, Want::Page]));
        assert_eq!(
            serde_json::from_str::<Command>(r#"{"type":"set","key":"icon_interval","value":1.0}"#).unwrap(),
            Command::Set { key: "icon_interval".into(), value: 1.0.into() }
        );
        assert_eq!(serde_json::to_string(&Command::Start).unwrap(), r#"{"type":"start"}"#);
        assert_eq!(serde_json::to_string(&State::Bye).unwrap(), r#"{"type":"bye"}"#);
        assert_eq!(serde_json::from_str::<AgentCommand>(r#"{"type":"show"}"#).unwrap(), AgentCommand::Show { id: None });
        assert_eq!(serde_json::from_str::<AgentCommand>(r#"{"type":"quit"}"#).unwrap(), AgentCommand::Quit { all: false });
    }

    #[test]
    fn what_is_unknown_is_ignored() {
        // A newer Toolbox: an unknown command, an unknown field, an unknown thing to want.
        assert_eq!(serde_json::from_str::<Command>(r#"{"type":"dance","fast":true}"#).unwrap(), Command::Unknown);
        let hello: Command = serde_json::from_str(r#"{"type":"hello","protocol":7,"want":["status","smell"],"extra":1}"#).unwrap();
        let Command::Hello(h) = hello else { panic!() };
        assert_eq!(h.want, vec![Want::Status, Want::Unknown]);
        // A newer program: an unknown state, an unknown health.
        assert_eq!(serde_json::from_str::<State>(r#"{"type":"mood","value":3}"#).unwrap(), State::Unknown);
        let s: State = serde_json::from_str(r#"{"type":"status","running":true,"state":"purple","text":"x","more":1}"#).unwrap();
        assert_eq!(s, State::Status(Status { running: true, state: Health::Unknown, text: "x".into() }));
    }
}
