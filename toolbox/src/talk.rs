//! What the two roles of `Toolbox.exe` say to each other over the agent's pipe (`windows-toolbox.toolbox`).
//!
//! The first three messages to the agent are public – any program may send them (DESIGN.md 15.1). Everything else
//! is private to this exe: window and agent are always the same build, so these messages may change with any
//! version and are deliberately not part of the protocol programs rely on.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backup::Plan;
use crate::catalog::Offer;
use crate::library::{Added, Library};
use crate::system::Trace;

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
    /// "Update" on a program's page – `toolbox` (`library::OWN_ID`) is the Toolbox itself.
    Update { id: String },
    /// "Update all" in the gear: every program that can be updated, the Toolbox last.
    UpdateAll,
    /// "Get programs": load the signed list from GitHub.
    Catalog,
    /// "Get" on that list: install this program into `programs\<id>\` and put it on the list.
    Get { id: String },
    /// "Restart after a crash" switched on or off for a program.
    RestartAfterCrash { id: String, on: bool },
    /// The Toolbox's own "Start with Windows".
    OwnAutostart { on: bool },
    /// "Show in the Start menu".
    StartMenu { on: bool },
    /// "Export all settings" into this zip.
    Export { file: PathBuf },
    /// "Import settings": what would this zip do? (The answer is `Tell::Plan`.)
    ImportCheck { file: PathBuf },
    /// … and now do it.
    Import { file: PathBuf },
    /// "Clean up" (`everything: false`) or "Uninstall all": what is there? (The answer is `Tell::Traces`.)
    Traces { everything: bool },
    /// Remove these traces – what Clean up listed.
    CleanUp { traces: Vec<Trace> },
    /// Uninstall all: remove every trace, end every program and the Toolbox, open the Toolbox's folder.
    Uninstall,
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
    /// The list of "Get programs" – or why it could not be loaded.
    Catalog { offers: Result<Vec<Offer>, String> },
    /// Come to the front – on this program's page if given.
    Show {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// What an import would do – or why the file cannot be imported.
    Plan(Result<Plan, String>),
    /// The settings were imported: the window takes language, theme and zoom from the next library again.
    Imported,
    /// What Clean up or Uninstall all found.
    Traces { everything: bool, traces: Vec<Trace> },
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
        let library = Library { settings: Default::default(), programs: vec![], missing: vec![], folder: "C:\\T".into(), data: "C:\\T\\data".into(), updating: Default::default(), own: Default::default() };
        let line = serde_json::to_string(&Tell::Library(library.clone())).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<Tell>(&line).unwrap(), Tell::Library(library));
    }
}
