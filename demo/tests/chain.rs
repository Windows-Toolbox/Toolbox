//! The conversation, tested without a window (DESIGN.md 13): start the demo, talk to its pipe, look at the saved
//! file. Everything runs in a sandbox – its own pipe names and its own data folder – so a demo or a Toolbox that
//! is really in use is never touched.

use std::path::PathBuf;
use std::process::{Child, Command as Process};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use kit::ipc::{self, Conn};
use kit::page::Block;
use kit::proto::{AgentCommand, Command, State, ToolboxHello, Want};

const ID: &str = "kit_demo";

/// One program id, one pipe: the tests of this file take turns.
fn turn() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    static SANDBOX: OnceLock<()> = OnceLock::new();
    SANDBOX.get_or_init(|| unsafe { std::env::set_var("TOOLBOX_SANDBOX", format!("chain{}", std::process::id())) });
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Demo {
    child: Child,
    data: PathBuf,
}

impl Demo {
    /// `workers`: a double-click (true) or the Toolbox asking for the page of a stopped program (false).
    fn start(name: &str, workers: bool) -> Demo {
        let data = std::env::temp_dir().join(format!("kit-demo-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        Demo::start_in(data, workers)
    }

    fn start_in(data: PathBuf, workers: bool) -> Demo {
        assert!(!ipc::exists(ID), "a demo of an earlier test is still running");
        let mut cmd = Process::new(env!("CARGO_BIN_EXE_kit-demo"));
        cmd.env(kit::app::data_dir_variable(ID), &data);
        if !workers {
            cmd.arg(kit::app::WORKERS_OFF);
        }
        Demo { child: cmd.spawn().unwrap(), data }
    }

    fn connect(&self, language: &str, want: &[Want]) -> Arc<Conn> {
        let c = ipc::connect_wait(ID, Duration::from_secs(5)).expect("the demo's pipe never appeared");
        c.send(&Command::Hello(ToolboxHello { language: language.into(), want: want.to_vec(), ..Default::default() })).unwrap();
        let State::Hello { id, pid, protocol, .. } = next(&c) else { panic!("the first answer must be hello") };
        assert_eq!((id.as_str(), pid, protocol), (ID, self.child.id(), 1));
        c
    }

    fn settings(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.data.join("settings.json")).unwrap()).unwrap()
    }

    fn wait_exit(&mut self, within: Duration) {
        let t = Instant::now();
        while self.child.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < within, "the demo did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(self.child.try_wait().unwrap().unwrap().success());
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn next(c: &Conn) -> State {
    c.read().expect("the pipe closed")
}

/// Reads states until `pick` finds what it waits for.
fn until<T>(c: &Conn, mut pick: impl FnMut(&State) -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        if let Some(found) = pick(&next(c)) {
            return found;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "waited too long");
    }
}

fn find<'a>(blocks: &'a [Block], pick: &dyn Fn(&Block) -> bool) -> Option<&'a Block> {
    blocks.iter().find_map(|b| match b {
        b if pick(b) => Some(b),
        Block::Section { blocks, .. } => find(blocks, pick),
        _ => None,
    })
}

fn slider(state: &State) -> Option<f64> {
    let State::Page(p) = state else { return None };
    match find(&p.blocks, &|b| matches!(b, Block::Slider { .. })) {
        Some(Block::Slider { value, .. }) => Some(*value),
        _ => None,
    }
}

fn switch_label(state: &State) -> Option<String> {
    let State::Page(p) = state else { return None };
    match find(&p.blocks, &|b| matches!(b, Block::Switch { .. })) {
        Some(Block::Switch { label, .. }) => Some(label.clone()),
        _ => None,
    }
}

/// The page of a stopped program: `set` comes back at once, a drag writes the file once, a wrong value is
/// refused, a second start hands `start` over, `quit` ends cleanly.
#[test]
fn the_page_of_a_stopped_program() {
    let _turn = turn();
    let mut demo = Demo::start("page", false);
    let c = demo.connect("en", &[Want::Status, Want::Page]);
    let State::Status(st) = next(&c) else { panic!("status comes first") };
    assert!(!st.running, "the workers must be off");
    assert_eq!(slider(&next(&c)), Some(1.0), "then the page, with the default");

    // A drag: ten steps. Each one is pushed back at once; the file is written once, after the drag.
    for i in 1..=10 {
        c.send(&Command::Set { key: "rate".into(), value: (0.5 * i as f64).into() }).unwrap();
    }
    until(&c, |s| (slider(s) == Some(5.0)).then_some(()));
    assert!(!demo.data.join("settings.json").exists(), "written during the drag");
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(demo.settings()["rate"], 5.0);

    // A refused set comes back as a toast plus the unchanged page.
    c.send(&Command::Set { key: "rate".into(), value: "fast".into() }).unwrap();
    assert!(matches!(next(&c), State::Toast { .. }));
    assert_eq!(slider(&next(&c)), Some(5.0));
    c.send(&Command::Set { key: "no_such_setting".into(), value: 1.into() }).unwrap();
    assert!(matches!(next(&c), State::Toast { .. }));
    assert_eq!(slider(&next(&c)), Some(5.0));

    // An action: its result as a toast.
    c.send(&Command::Action { name: "reset".into() }).unwrap();
    assert_eq!(until(&c, |s| if let State::Toast { text } = s { Some(text.clone()) } else { None }), "Counter reset");

    // A second start = the single-instance lock: it sends `start` to the running one and ends.
    let second = Process::new(env!("CARGO_BIN_EXE_kit-demo")).env(kit::app::data_dir_variable(ID), &demo.data).status().unwrap();
    assert!(second.success());
    until(&c, |s| matches!(s, State::Status(st) if st.running).then_some(()));
    // Now it measures: the status text changes by itself.
    until(&c, |s| matches!(s, State::Status(st) if st.text.starts_with("Value")).then_some(()));

    c.send(&Command::Quit).unwrap();
    until(&c, |s| matches!(s, State::Bye).then_some(()));
    assert_eq!(c.read_line(), None, "after bye the pipe closes");
    demo.wait_exit(Duration::from_secs(3));
}

/// Language and look arrive with `hello`: the page comes in that language, a later `hello` switches it for every
/// connection, and the program remembers it.
#[test]
fn language_and_look() {
    let _turn = turn();
    let mut demo = Demo::start("language", false);
    let c = demo.connect("de", &[Want::Page]);
    assert_eq!(switch_label(&next(&c)).as_deref(), Some("Ab 60 warnen"));
    // A second viewer in English: both get the English page.
    let c2 = demo.connect("en", &[Want::Page]);
    assert_eq!(switch_label(&next(&c2)).as_deref(), Some("Warn from 60"));
    assert_eq!(switch_label(&next(&c)).as_deref(), Some("Warn from 60"));
    // The same connection changes its mind – and wants the tray as well.
    c.send(&Command::Hello(ToolboxHello { language: "de".into(), theme: "light".into(), zoom: 1.25, want: vec![Want::Page, Want::Tray], ..Default::default() })).unwrap();
    assert_eq!(switch_label(&next(&c)).as_deref(), Some("Ab 60 warnen"));
    let State::Tray(tray) = next(&c) else { panic!("the tray was asked for") };
    assert_eq!(tray.menu[0].label, "Zähler zurücksetzen");
    assert_eq!(switch_label(&next(&c2)).as_deref(), Some("Ab 60 warnen"));
    // An unknown language leaves the language alone.
    c.send(&Command::Hello(ToolboxHello { language: "xx".into(), theme: "light".into(), zoom: 1.25, want: vec![Want::Page], ..Default::default() })).unwrap();

    c.send(&Command::Quit).unwrap();
    until(&c, |s| matches!(s, State::Bye).then_some(()));
    demo.wait_exit(Duration::from_secs(3));
    assert_eq!(demo.settings()["_toolbox"], serde_json::json!({"language": "de", "theme": "light", "zoom": 1.25}));

    // Started again, it speaks German from the first message on – before any Toolbox said hello with a language.
    let mut again = Demo::start_in(demo.data.clone(), false);
    let c = ipc::connect_wait(ID, Duration::from_secs(5)).unwrap();
    c.send_line(r#"{"type":"hello","protocol":1,"want":["page"]}"#).unwrap();
    assert!(matches!(next(&c), State::Hello { .. }));
    // (A hello without a language means "en" – the remembered one only counts until the first hello.)
    assert_eq!(switch_label(&next(&c)).as_deref(), Some("Warn from 60"));
    c.send(&Command::Quit).unwrap();
    again.wait_exit(Duration::from_secs(3));
}

/// With the workers off, only somebody who wants the page keeps the program alive (found in M0): the agent,
/// connected for status and tray, must not.
#[test]
fn a_stopped_program_ends_when_nobody_looks_at_its_page() {
    let _turn = turn();
    let mut demo = Demo::start("idle", false);
    let agent = demo.connect("en", &[Want::Status, Want::Tray]);
    let page = demo.connect("en", &[Want::Page]);
    std::thread::sleep(Duration::from_millis(3000));
    assert!(demo.child.try_wait().unwrap().is_none(), "the page viewer keeps it alive");
    drop(page);
    until(&agent, |s| matches!(s, State::Bye).then_some(()));
    demo.wait_exit(Duration::from_secs(3));
}

/// A program that starts its workers tells the agent (`started`), so the agent never has to poll; and `stop`
/// switches the workers off without ending the process while its page is shown.
#[test]
fn started_is_announced_and_stop_keeps_the_page() {
    let _turn = turn();
    let agent = ipc::Server::bind(ipc::TOOLBOX).unwrap();
    let mut demo = Demo::start("started", true);
    let announced: AgentCommand = agent.accept().unwrap().read().unwrap();
    assert_eq!(announced, AgentCommand::Started { id: ID.into() });

    let c = demo.connect("en", &[Want::Status, Want::Page]);
    until(&c, |s| matches!(s, State::Status(st) if st.running).then_some(()));
    c.send(&Command::Stop).unwrap();
    until(&c, |s| matches!(s, State::Status(st) if !st.running).then_some(()));
    std::thread::sleep(Duration::from_millis(2500));
    assert!(demo.child.try_wait().unwrap().is_none(), "stopped, but its page is still shown");
    // Start again: announced again.
    c.send(&Command::Start).unwrap();
    assert_eq!(agent.accept().unwrap().read::<AgentCommand>().unwrap(), AgentCommand::Started { id: ID.into() });
    // A connection that does not start with hello is closed.
    let rude = ipc::connect(ID).unwrap();
    rude.send(&Command::Quit).unwrap();
    assert_eq!(rude.read_line(), None);
    assert!(demo.child.try_wait().unwrap().is_none(), "a quit without hello is not obeyed");
    c.send(&Command::Quit).unwrap();
    demo.wait_exit(Duration::from_secs(3));
}
