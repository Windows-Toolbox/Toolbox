//! What the two roles of `Toolbox.exe` say to each other over the agent's pipe (`windows-toolbox.toolbox`).
//!
//! The first three messages to the agent are public – any program may send them (DESIGN.md 15.1). Everything else
//! is private to this exe: window and agent are always the same build, so these messages may change with any
//! version and are deliberately not part of the protocol programs rely on.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::{Added, Library};

/// To the agent.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ask {
    // ---- public (DESIGN.md 15.1; the same messages as `kit::proto::AgentCommand`) ----
    /// Open the window – on this program's page if given.
    Show {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// A program announces that its workers run.
    Started { id: String },
    /// End the agent – with `all`, every background program first.
    Quit {
        #[serde(default)]
        all: bool,
    },

    // ---- private: the window ----
    /// This connection is the window: send it the library now and on every change.
    Window,
    /// Language, theme or zoom were changed in the window.
    Look { language: String, theme: String, zoom: f32 },
    /// "Add program…": the chosen folder.
    Add { folder: PathBuf },
    /// The answer to a conflict: this folder replaces the listed one that holds the same program.
    Replace { folder: PathBuf },
    /// Take a folder off the list (as it stands in the settings).
    Remove { stored: String },
    Start { id: String },
    Stop { id: String },
    Restart { id: String },
    /// "Start with Windows" switched on or off.
    Autostart { id: String, on: bool },
    /// One option of a script was changed on its page.
    SetOption { id: String, name: String, value: Value },
    /// "Restart the Toolbox" in the gear: the agent ends and starts anew; the programs keep running.
    RestartToolbox,
    #[serde(other)]
    Unknown,
}

/// From the agent to the window.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tell {
    /// Everything the window shows; sent again on every change.
    Library(Library),
    Toast {
        text: String,
        #[serde(default)]
        error: bool,
    },
    /// What "Add program…" did.
    Added(Added),
    /// Come to the front – on this program's page if given.
    Show {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// The agent ends; so does the window.
    Bye,
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::proto::AgentCommand;

    /// What programs send (the kit's `AgentCommand`) is what the agent reads.
    #[test]
    fn the_public_messages_are_the_kit_s() {
        let read = |c: AgentCommand| serde_json::from_str::<Ask>(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(read(AgentCommand::Show { id: None }), Ask::Show { id: None });
        assert_eq!(read(AgentCommand::Show { id: Some("x".into()) }), Ask::Show { id: Some("x".into()) });
        assert_eq!(read(AgentCommand::Started { id: "x".into() }), Ask::Started { id: "x".into() });
        assert_eq!(read(AgentCommand::Quit { all: true }), Ask::Quit { all: true });
        assert_eq!(serde_json::from_str::<Ask>(r#"{"type":"quit"}"#).unwrap(), Ask::Quit { all: false });
        assert_eq!(serde_json::from_str::<Ask>(r#"{"type":"something_newer","x":1}"#).unwrap(), Ask::Unknown);
    }

    #[test]
    fn the_library_travels_as_one_line() {
        let library = Library { settings: Default::default(), programs: vec![], missing: vec![], folder: "C:\\T".into(), data: "C:\\T\\data".into() };
        let line = serde_json::to_string(&Tell::Library(library.clone())).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<Tell>(&line).unwrap(), Tell::Library(library));
    }
}
