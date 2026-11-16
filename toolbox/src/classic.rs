//! The programs of the classic (Python) Toolbox, shown in the new one "as they are" until each is ported
//! (DESIGN.md 11, M1): started through their starter exe, status and Stop through their TCP port, texts from the
//! `i18n.json` next to their manifest, and "Settings" opens the classic Toolbox on their page.
//!
//! This whole file goes away with the last Python program.

use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kit::ipc::legacy;
use kit::manifest::{Found, Kind, Text, value_text};
use kit::process;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ANSWER: Duration = Duration::from_millis(600);

/// What the new Toolbox needs to know about a program that still lives in the classic one.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Classic {
    /// The classic Toolbox's main folder (the program lies in its `programs\<id>\`), if it is there.
    pub root: Option<PathBuf>,
    /// The exe that starts the program (`"starter"`).
    pub starter: Option<PathBuf>,
    /// Its TCP port on 127.0.0.1 (`"port"`): `status`, `quit`.
    pub port: Option<u16>,
    /// The scheduled task that starts it with admin rights (`"task"`, the Hardware Monitor).
    pub task: Option<String>,
    /// It has a page in the classic Toolbox (`"page"`).
    pub page: bool,
}

type Table = BTreeMap<String, BTreeMap<String, String>>;

/// If the manifest belongs to the classic Toolbox – by its kind, or by the `i18n.json` next to it – this reads
/// what is needed from its old fields and puts the texts of `i18n.json` into the manifest, so everything after
/// this sees one format.
pub fn inspect(found: &mut Found) -> Option<Classic> {
    let texts: Option<Table> = std::fs::read_to_string(found.home.join("i18n.json")).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok());
    if found.manifest.kind() != Kind::Classic && texts.is_none() {
        return None;
    }
    let m = &mut found.manifest;
    if let Some(table) = &texts {
        let text = |id: &str| table.get(id).filter(|row| !row.is_empty()).map(|row| Text::PerLanguage(row.clone()));
        if let Some(t) = text("program.name") {
            m.name = t;
        }
        if let Some(t) = text("program.description") {
            m.description = t;
        }
        m.hint = m.hint.take().or_else(|| text("page.hint"));
        m.button = m.button.take().or_else(|| text("page.button"));
        for (name, option) in m.options.iter_mut() {
            option.label = option.label.take().or_else(|| text(&format!("option.{name}")));
            for value in &option.values {
                let value = value_text(value);
                if let Some(t) = text(&format!("option.{name}.{value}")) {
                    option.value_labels.entry(value).or_insert(t);
                }
            }
        }
    }
    // programs\<id>\ -> the main folder, recognised by the classic Toolbox's own manifest.
    let root = found.home.parent().and_then(Path::parent).filter(|r| r.join("toolbox").join("program.json").exists()).map(PathBuf::from);
    Some(Classic {
        root,
        starter: m.other.get("starter").and_then(Value::as_str).map(|s| found.home.join(s)),
        port: m.other.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()),
        task: m.other.get("task").and_then(Value::as_str).map(String::from),
        page: m.other.contains_key("page"),
    })
}

/// Why a classic program did not start.
#[derive(Debug, PartialEq)]
pub enum StartError {
    /// The starter exe is missing (Setup.exe of the classic Toolbox creates it).
    NoStarter,
    /// Windows asked for admin rights and the answer was no.
    Declined,
    Failed(String),
}

impl Classic {
    /// Does it run? Only a program with a port can tell.
    pub fn running(&self) -> Option<bool> {
        self.port.map(legacy::listening)
    }

    /// Through its scheduled task if it has one that is enabled and points to this starter (no prompt) –
    /// otherwise, for a program that needs admin rights, directly with a prompt – otherwise just the starter.
    pub fn start(&self) -> Result<(), StartError> {
        let starter = self.starter.as_ref().filter(|s| s.exists()).ok_or(StartError::NoStarter)?;
        let Some(task) = &self.task else {
            return process::spawn(starter, &[]).map(|_| ()).map_err(|e| StartError::Failed(e.to_string()));
        };
        if task_fits(task, starter) && schtasks(&["/run", "/tn", task]).is_some() {
            return Ok(());
        }
        if process::run_elevated(starter) { Ok(()) } else { Err(StartError::Declined) }
    }

    /// Asks the program to quit and waits for its process to end (a direct restart would fail otherwise).
    /// `true` if it is gone afterwards.
    pub fn stop(&self) -> bool {
        let Some(port) = self.port else { return false };
        let Some(status) = legacy::command(port, "status", ANSWER) else { return !legacy::listening(port) };
        legacy::command(port, "quit", ANSWER);
        if let Some(pid) = status["pid"].as_u64() {
            if process::wait_exit(pid as u32, Duration::from_secs(10)) {
                return true;
            }
        }
        let t = Instant::now();
        while legacy::listening(port) {
            if t.elapsed() > Duration::from_secs(5) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// Shows the program's page in the classic Toolbox: `show <id>` to its port – started first if it does not
    /// answer. Blocks for up to 15 s, so call it on a thread.
    pub fn open_settings(&self, id: &str) -> bool {
        let Some(root) = &self.root else { return false };
        // One at a time: a second click while the classic Toolbox is still starting waits here and then finds it
        // running – instead of starting a second one (seen in the M1 hand test: two classic windows).
        static OPENING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one_at_a_time = OPENING.lock().unwrap_or_else(|e| e.into_inner());
        let manifest: Option<Value> = std::fs::read_to_string(root.join("toolbox").join("program.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
        let Some(port) = manifest.and_then(|m| m["port"].as_u64()).and_then(|p| u16::try_from(p).ok()) else { return false };
        let show = || legacy::listening(port) && legacy::command(port, &format!("show {id}"), ANSWER).is_some();
        // The classic Toolbox may bring its window to the front although the click happened in ours.
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX) };
        if show() {
            return true;
        }
        let Some(exe) = ["ToolboxClassic.exe", "Toolbox.exe"].iter().map(|name| root.join(name)).find(|p| p.exists()) else { return false };
        if process::spawn(&exe, &[]).is_err() {
            return false;
        }
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(250));
            if show() {
                return true;
            }
        }
        false
    }
}

/// Runs schtasks.exe without a window; its output if it succeeded.
fn schtasks(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("schtasks.exe").args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Is the task enabled, and does it start exactly this exe (and not the place the folder was at before it was
/// moved)? Asked as XML, which reads the same in every Windows language.
fn task_fits(task: &str, starter: &Path) -> bool {
    schtasks(&["/query", "/tn", task, "/xml"]).is_some_and(|xml| task_xml_fits(&xml, starter))
}

fn task_xml_fits(xml: &str, starter: &Path) -> bool {
    let between = |text: &str, tag: &str| -> Option<String> {
        let start = text.find(&format!("<{tag}>"))? + tag.len() + 2;
        let end = text[start..].find(&format!("</{tag}>"))? + start;
        Some(text[start..end].trim().to_string())
    };
    // <Enabled> also exists per trigger; the one that switches the whole task is in <Settings>. Missing means on.
    let enabled = between(xml, "Settings").and_then(|s| between(&s, "Enabled")).is_none_or(|e| e.eq_ignore_ascii_case("true"));
    let command = between(xml, "Command").unwrap_or_default().replace("&amp;", "&");
    let has_arguments = between(xml, "Arguments").is_some_and(|a| !a.is_empty());
    enabled && !has_arguments && command.trim_matches('"').eq_ignore_ascii_case(&starter.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::manifest::Manifest;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("toolbox-classic-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(file: PathBuf, text: &str) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    #[test]
    fn a_python_program_and_a_classic_script() {
        let root = temp("inspect");
        write(root.join("toolbox/program.json"), r#"{"id":"toolbox","name":"Toolbox","port":47816}"#);
        write(root.join("programs/hardware_monitor/program.json"), r#"{"id":"hardware_monitor","name":"Hardware Monitor","category":"General","page":"gui.py","starter":"HardwareMonitor.exe","port":47815,"task":"HardwareMonitor"}"#);
        write(root.join("programs/hardware_monitor/i18n.json"), r#"{"program.name":{"de":"Hardware-Monitor","en":"Hardware Monitor"},"program.description":{"de":"Temperaturen …","en":"Temperatures …"}}"#);
        write(root.join("programs/rayman/program.json"), r#"{"id":"rayman","name":"Rayman","run":"r.ps1","options":{"Pos":{"values":["Left","Center"],"default":"Center"}}}"#);
        write(root.join("programs/rayman/i18n.json"), r#"{"page.hint":{"de":"Hinweis","en":"Hint"},"page.button":{"de":"Spielen","en":"Play"},"option.Pos":{"de":"Position","en":"Position"},"option.Pos.Left":{"de":"Links","en":"Left"}}"#);
        write(root.join("New/program.json"), r#"{"id":"new","name":"New","run":"n.ps1"}"#);

        let mut monitor = Manifest::read(&root.join("programs/hardware_monitor")).unwrap().unwrap();
        let c = inspect(&mut monitor).unwrap();
        assert_eq!((monitor.manifest.name.get("de"), monitor.manifest.description.get("en")), ("Hardware-Monitor", "Temperatures …"));
        assert_eq!((c.port, c.task.as_deref(), c.page), (Some(47815), Some("HardwareMonitor"), true));
        assert_eq!(c.starter.unwrap(), root.join("programs/hardware_monitor").join("HardwareMonitor.exe"));
        assert_eq!(c.root.unwrap(), root);

        // A script in the classic layout stays a script; only its texts come from i18n.json.
        let mut rayman = Manifest::read(&root.join("programs/rayman")).unwrap().unwrap();
        let c = inspect(&mut rayman).unwrap();
        let m = &rayman.manifest;
        assert_eq!((m.kind(), c.page, c.starter), (Kind::Script, false, None));
        assert_eq!((m.hint.as_ref().unwrap().get("de"), m.button.as_ref().unwrap().get("en")), ("Hinweis", "Play"));
        assert_eq!(m.options["Pos"].label.as_ref().unwrap().get("de"), "Position");
        assert_eq!(m.options["Pos"].value_labels["Left"].get("de"), "Links");
        assert!(!m.options["Pos"].value_labels.contains_key("Center"));

        // A script in the new layout has nothing to do with all this.
        assert!(inspect(&mut Manifest::read(&root.join("New")).unwrap().unwrap()).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_task_must_be_enabled_and_point_here() {
        let xml = |enabled: &str, command: &str, arguments: &str| {
            format!("<Task><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Settings><Enabled>{enabled}</Enabled></Settings><Actions><Exec><Command>{command}</Command>{arguments}</Exec></Actions></Task>")
        };
        let here = Path::new(r"E:\Tools & More\programs\hardware_monitor\HardwareMonitor.exe");
        assert!(task_xml_fits(&xml("true", r"e:\tools &amp; more\programs\hardware_monitor\HardwareMonitor.exe", ""), here));
        assert!(!task_xml_fits(&xml("false", r"E:\Tools &amp; More\programs\hardware_monitor\HardwareMonitor.exe", ""), here), "disabled");
        assert!(!task_xml_fits(&xml("true", r"D:\Old Place\HardwareMonitor.exe", ""), here), "points elsewhere");
        assert!(!task_xml_fits(&xml("true", r"E:\Tools &amp; More\programs\hardware_monitor\HardwareMonitor.exe", "<Arguments>-x</Arguments>"), here));
    }

    /// Status and Stop through the port, against a stand-in for a Python program.
    #[test]
    fn stop_through_the_port() {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let program = std::thread::spawn(move || {
            let mut commands = vec![];
            loop {
                let (mut conn, _) = listener.accept().unwrap();
                let mut line = String::new();
                std::io::BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
                // A pid that does not exist: "its process has ended" is true at once.
                conn.write_all(b"{\"ok\": true, \"pid\": 4294967280}\n").unwrap();
                commands.push(line.trim().to_string());
                if line.trim() == "quit" {
                    return commands;
                }
            }
        });
        let c = Classic { port: Some(port), ..Default::default() };
        assert_eq!(c.running(), Some(true));
        assert!(c.stop());
        assert_eq!(program.join().unwrap(), ["status", "quit"]);
        assert_eq!(c.running(), Some(false));
        assert!(c.stop(), "stopping what does not run is fine");
        assert_eq!(Classic::default().running(), None);
        assert_eq!(Classic::default().start(), Err(StartError::NoStarter));
    }
}
