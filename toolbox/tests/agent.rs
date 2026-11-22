//! The agent, tested without a window and without a tray icon (DESIGN.md 13): start it in the background, talk
//! to its pipe the way the window does, look at what it answers and at the files it writes. Everything runs in a
//! sandbox – its own pipe and job names, its own data folder – so a Toolbox that is really in use is never touched.

use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use kit::ipc::{self, Conn};
use kit::proto::{Command, Health, State, Status, Want};
use serde_json::{Value, json};

/// One agent pipe: the tests of this file take turns.
fn turn() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    static SANDBOX: OnceLock<()> = OnceLock::new();
    SANDBOX.get_or_init(|| unsafe { std::env::set_var("TOOLBOX_SANDBOX", format!("agent{}", std::process::id())) });
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Toolbox {
    child: Arc<Mutex<Child>>,
    /// A folder of its own: `Toolbox\data\` for the agent, the test's programs next to it.
    root: PathBuf,
    /// The test is over (told to the watchdog).
    done: Arc<AtomicBool>,
}

impl Toolbox {
    fn start(name: &str) -> Toolbox {
        assert!(!ipc::exists(ipc::TOOLBOX), "the agent of an earlier test is still running");
        let root = std::env::temp_dir().join(format!("toolbox-agent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // English, whatever language this PC speaks.
        let data = root.join("Toolbox").join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("settings.json"), r#"{"language": "en"}"#).unwrap();
        let agent = Process::new(env!("CARGO_BIN_EXE_toolbox")).arg("--background").env(kit::app::data_dir_variable(ipc::TOOLBOX), &data).spawn().unwrap();
        let toolbox = Toolbox { child: Arc::new(Mutex::new(agent)), root, done: Arc::default() };
        // A test waits for the agent's answers without a time limit of its own. Should an answer never come, this
        // ends the agent after a minute: the pipe closes and the waiting test fails instead of hanging forever.
        let (child, done) = (toolbox.child.clone(), toolbox.done.clone());
        std::thread::spawn(move || {
            for _ in 0..600 {
                std::thread::sleep(Duration::from_millis(100));
                if done.load(Ordering::SeqCst) {
                    return;
                }
            }
            eprintln!("the test took too long – ending its agent");
            let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
            // Whatever still waits for something else (a client that never connects) is ended the hard way.
            std::thread::sleep(Duration::from_secs(5));
            if !done.load(Ordering::SeqCst) {
                std::process::exit(3);
            }
        });
        toolbox
    }

    fn write(&self, path: &str, text: &str) {
        let file = self.root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    /// Connects the way the window does; the first thing that comes back is the library.
    fn window(&self) -> (Arc<Conn>, Value) {
        let conn = ipc::connect_wait(ipc::TOOLBOX, Duration::from_secs(5)).expect("the agent's pipe never appeared");
        conn.send(&json!({"type": "window"})).unwrap();
        let library = library(&conn, |_| true);
        (conn, library)
    }

    fn settings(&self) -> Value {
        read_json(&self.root.join("Toolbox").join("data").join("settings.json"))
    }

    fn wait_exit(&mut self) {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let t = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < Duration::from_secs(5), "the agent did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child.try_wait().unwrap().unwrap().success());
    }
}

impl Drop for Toolbox {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn read_json(file: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

/// Reads what the agent tells until `pick` finds what it waits for.
fn until<T>(conn: &Conn, mut pick: impl FnMut(&Value) -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        let told: Value = conn.read().expect("the agent closed the pipe");
        if let Some(found) = pick(&told) {
            return found;
        }
        assert!(t.elapsed() < Duration::from_secs(15), "waited too long; last: {told}");
    }
}

/// The next library that fits.
fn library(conn: &Conn, fits: impl Fn(&Value) -> bool) -> Value {
    until(conn, |told| (told["type"] == "library" && fits(told)).then(|| told.clone()))
}

/// A script on the list: added through its parent folder, its option saved where it reads it, started in its
/// job, seen running, stopped as a whole, taken off the list again. And the Toolbox's own settings.
#[test]
fn a_script_from_add_to_remove() {
    let _turn = turn();
    let mut toolbox = Toolbox::start("script");
    toolbox.write("Programs/Sleeper/program.json", r#"{"id":"sleeper","name":"Sleeper","category":"Games","run":"sleep.ps1","options":{"Pos":{"values":["Left","Right"]}}}"#);
    toolbox.write("Programs/Sleeper/sleep.ps1", "Start-Sleep 120");
    toolbox.write("Programs/Notes/readme.txt", "not a program");
    let (window, first) = toolbox.window();
    assert_eq!(first["programs"], json!([]), "a fresh Toolbox lists nothing");
    assert_eq!(first["settings"]["theme"], "dark");

    // "Add program…" on the folder above: the one program below it is found.
    window.send(&json!({"type": "add", "folder": toolbox.root.join("Programs")})).unwrap();
    let added = until(&window, |told| (told["type"] == "added").then(|| told.clone()));
    assert_eq!((&added["added"], &added["known"]), (&json!(["Sleeper"]), &json!(0)));
    let listed = library(&window, |l| l["programs"][0]["manifest"]["id"] == "sleeper");
    let program = &listed["programs"][0];
    assert_eq!((&program["kind"], &program["run"]["running"], &program["options"]["Pos"]), (&json!("script"), &json!(false), &json!("Left")));
    assert_eq!(toolbox.settings()["programs"], json!([r"..\Programs\Sleeper"]), "remembered relative to the Toolbox's folder");

    // An option: saved in the file the script reads; a value it does not have is refused.
    window.send(&json!({"type": "set_option", "id": "sleeper", "name": "Pos", "value": "Right"})).unwrap();
    library(&window, |l| l["programs"][0]["options"]["Pos"] == "Right");
    assert_eq!(read_json(&toolbox.root.join("Programs/Sleeper/data/settings.json")), json!({"Pos": "Right"}));
    window.send(&json!({"type": "set_option", "id": "sleeper", "name": "Pos", "value": "Up"})).unwrap();
    assert_eq!(until(&window, |told| (told["type"] == "toast").then(|| told["error"].clone())), true);

    // Start and stop.
    window.send(&json!({"type": "start", "id": "sleeper"})).unwrap();
    library(&window, |l| l["programs"][0]["run"]["running"] == true);
    window.send(&json!({"type": "stop", "id": "sleeper"})).unwrap();
    library(&window, |l| l["programs"][0]["run"]["running"] == false);

    // Language and look: saved, and back in the library.
    window.send(&json!({"type": "look", "language": "de", "theme": "light", "zoom": 1.25})).unwrap();
    library(&window, |l| l["settings"]["theme"] == "light");
    let saved = toolbox.settings();
    assert_eq!((&saved["language"], &saved["theme"], &saved["zoom"]), (&json!("de"), &json!("light"), &json!(1.25)));
    // What is invalid changes nothing.
    window.send(&json!({"type": "look", "language": "xx", "theme": "pink", "zoom": 99.0})).unwrap();

    // Off the list: the folder stays.
    window.send(&json!({"type": "remove", "stored": r"..\Programs\Sleeper"})).unwrap();
    library(&window, |l| l["programs"] == json!([]));
    assert!(toolbox.root.join("Programs/Sleeper/sleep.ps1").exists());
    let saved = toolbox.settings();
    assert_eq!((&saved["programs"], &saved["theme"]), (&json!([]), &json!("light")));

    // `quit` – a public message, sent by anybody: the window is told, the agent ends.
    ipc::connect(ipc::TOOLBOX).unwrap().send(&json!({"type": "quit"})).unwrap();
    until(&window, |told| (told["type"] == "bye").then_some(()));
    toolbox.wait_exit();
}

/// A kit program – played by this test: the agent connects when the program is there, shows its status, passes
/// language and commands on, notices a crash (the pipe closes without `bye`) and a clean end.
#[test]
fn a_kit_program_is_followed() {
    let _turn = turn();
    const ID: &str = "fake_kit";
    let program = ipc::Server::bind(ID).unwrap();
    let mut toolbox = Toolbox::start("kit");
    toolbox.write("Fake/program.json", r#"{"id":"fake_kit","name":"Fake","exe":"Fake.exe","protocol":1,"background":true}"#);
    let (window, _) = toolbox.window();
    window.send(&json!({"type": "add", "folder": toolbox.root.join("Fake")})).unwrap();

    // The program's pipe exists, so the agent connects: hello first, asking for status and tray.
    let hello = |conn: &Conn| match conn.read::<Command>().expect("the agent hung up") {
        Command::Hello(hello) => hello,
        other => panic!("the first message must be hello, not {other:?}"),
    };
    let status = |conn: &Conn, text: &str| conn.send(&State::Status(Status { running: true, state: Health::Warn, text: text.into() })).unwrap();
    let link = program.accept().unwrap();
    let said = hello(&link);
    assert_eq!((said.language.as_str(), said.want), ("en", vec![Want::Status, Want::Tray]));
    status(&link, "42 °C");
    let listed = library(&window, |l| l["programs"][0]["run"]["running"] == true);
    assert_eq!(listed["programs"][0]["run"], json!({"running": true, "state": "warn", "text": "42 °C", "crashed": false, "gave_up": false}));

    // A new language reaches the running program at once.
    window.send(&json!({"type": "look", "language": "de", "theme": "dark", "zoom": 1.0})).unwrap();
    assert_eq!(hello(&link).language, "de");
    // Stop from the window is the program's `stop`.
    window.send(&json!({"type": "stop", "id": ID})).unwrap();
    assert_eq!(link.read::<Command>(), Some(Command::Stop));

    // The pipe closes without `bye`: crashed.
    drop(link);
    library(&window, |l| l["programs"][0]["run"]["crashed"] == true && l["programs"][0]["run"]["running"] == false);

    // Started again, the program announces itself; the agent connects, and "crashed" is gone with the first status.
    ipc::connect(ipc::TOOLBOX).unwrap().send(&json!({"type": "started", "id": ID})).unwrap();
    let link = program.accept().unwrap();
    assert_eq!(hello(&link).language, "de");
    status(&link, "43 °C");
    library(&window, |l| l["programs"][0]["run"]["text"] == "43 °C" && l["programs"][0]["run"]["crashed"] == false);

    // "Restart after a crash" (DESIGN.md 8): switched on, a crash starts the program again – here that is this
    // test's pipe, so "start" arrives at it. Three times in ten minutes, then the agent gives up and says so.
    window.send(&json!({"type": "restart_after_crash", "id": ID, "on": true})).unwrap();
    library(&window, |l| l["settings"]["restart_after_crash"] == json!([ID]));
    assert_eq!(toolbox.settings()["restart_after_crash"], json!([ID]));
    let mut link = link;
    for n in 1..=3 {
        drop(link);
        let text = until(&window, |told| (told["type"] == "toast").then(|| told["text"].as_str().unwrap_or_default().to_string()));
        assert_eq!(text, format!("Fake ist abgestürzt und wurde neu gestartet ({n} von 3)."));
        link = program.accept().unwrap();
        hello(&link);
        assert_eq!(link.read::<Command>(), Some(Command::Start));
        status(&link, "back");
        library(&window, |l| l["programs"][0]["run"]["text"] == "back");
    }
    drop(link);
    let gave_up = library(&window, |l| l["programs"][0]["run"]["gave_up"] == true);
    assert_eq!(gave_up["programs"][0]["run"]["crashed"], true);
    // The program comes back by itself: what it reports clears "gave up".
    ipc::connect(ipc::TOOLBOX).unwrap().send(&json!({"type": "started", "id": ID})).unwrap();
    let link = program.accept().unwrap();
    hello(&link);
    status(&link, "44 °C");
    library(&window, |l| l["programs"][0]["run"]["gave_up"] == false && l["programs"][0]["run"]["text"] == "44 °C");

    // "Toolbox and all programs": the program is asked to quit. It says `bye` – that is no crash.
    window.send(&json!({"type": "quit", "all": true})).unwrap();
    assert_eq!(link.read::<Command>(), Some(Command::Quit));
    link.send(&State::Bye).unwrap();
    until(&window, |told| (told["type"] == "bye").then_some(()));
    toolbox.wait_exit();
}

/// "Start with Windows" for a background kit program: switched on and off from the window, an entry of another copy
/// left alone, and taken away with the program when it leaves the list. In the sandbox the entries lie where
/// Windows does not look.
#[test]
fn start_with_windows() {
    use kit::autostart::{self, State as Autostart};
    let _turn = turn();
    const ID: &str = "fake_autostart";
    let mut toolbox = Toolbox::start("autostart");
    toolbox.write("Plus/program.json", r#"{"id":"fake_autostart","name":"Plus","exe":"Plus.exe","protocol":1,"background":true}"#);
    toolbox.write("Plus/Plus.exe", "");
    toolbox.write("Game/program.json", r#"{"id":"fake_game","name":"Game","exe":"Game.exe"}"#);
    let exe = toolbox.root.join("Plus").join("Plus.exe");
    let _ = autostart::set(ID, None);
    let (window, _) = toolbox.window();
    window.send(&json!({"type": "add", "folder": toolbox.root.join("Plus")})).unwrap();
    window.send(&json!({"type": "add", "folder": toolbox.root.join("Game")})).unwrap();
    let listed = library(&window, |l| l["programs"].as_array().is_some_and(|p| p.len() == 2));
    assert_eq!(listed["programs"][0]["autostart"], json!({"state": "off", "target": null}));
    assert_eq!(listed["programs"][1]["autostart"], json!(null), "a plain exe has no switch");

    window.send(&json!({"type": "autostart", "id": ID, "on": true})).unwrap();
    library(&window, |l| l["programs"][0]["autostart"]["state"] == "on");
    assert_eq!(autostart::read(ID), autostart::Entry { state: Autostart::On, target: Some(exe.clone()) });
    window.send(&json!({"type": "autostart", "id": ID, "on": false})).unwrap();
    library(&window, |l| l["programs"][0]["autostart"]["state"] == "off");

    // Another copy holds the entry (set behind the agent's back – it looks once a second while a window is open):
    // switching off here leaves it, switching on takes it over.
    let other = std::env::current_exe().unwrap();
    autostart::set(ID, Some(&other)).unwrap();
    library(&window, |l| l["programs"][0]["autostart"]["state"] == "on");
    window.send(&json!({"type": "autostart", "id": ID, "on": false})).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(autostart::read(ID).target, Some(other.clone()), "not this copy's entry");
    window.send(&json!({"type": "autostart", "id": ID, "on": true})).unwrap();
    library(&window, |l| l["programs"][0]["autostart"]["target"] == json!(exe));

    // Off the list: the entry goes too.
    window.send(&json!({"type": "remove", "stored": r"..\Plus"})).unwrap();
    library(&window, |l| l["programs"].as_array().is_some_and(|p| p.len() == 1));
    assert_eq!(autostart::state(ID), Autostart::Off);
    // The sandbox's keys are not needed any more.
    let _ = Process::new("reg").args(["delete", &format!(r"HKCU\Software\WindowsToolbox\Sandbox{}", ipc::sandbox()), "/f"]).output();

    window.send(&json!({"type": "quit"})).unwrap();
    toolbox.wait_exit();
}

/// A second start while an agent runs only asks it to open the window – it must end at once and never become a
/// second agent. (With `--background` it does not even ask, so no window appears in this test.)
#[test]
fn a_second_start_ends_at_once() {
    let _turn = turn();
    let mut toolbox = Toolbox::start("second");
    let (window, _) = toolbox.window();
    let second = Process::new(env!("CARGO_BIN_EXE_toolbox")).arg("--background").env(kit::app::data_dir_variable(ipc::TOOLBOX), toolbox.root.join("Other").join("data")).status().unwrap();
    assert!(second.success());
    assert!(!toolbox.root.join("Other").exists(), "the second start must not write settings of its own");
    window.send(&json!({"type": "quit"})).unwrap();
    toolbox.wait_exit();
}

/// What the Toolbox itself puts into Windows (DESIGN.md 8, 9): its own "Start with Windows" and the Start menu
/// shortcut, switched on and off from the gear; Clean up lists only what belongs to no program any more, and a
/// moved program's entry is pointed at its new place instead. In the sandbox the Run key lies where Windows does
/// not look and the shortcut lies in the test's folder.
#[test]
fn the_toolbox_in_windows() {
    use kit::autostart::{self, State as Autostart};
    let _turn = turn();
    let mut toolbox = Toolbox::start("windows");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_toolbox"));
    let shortcut = toolbox.root.join("Toolbox").join("data").join(format!("start-menu{}", ipc::sandbox())).join("Windows Toolbox.lnk");
    for id in ["toolbox", "fake_moved", "fake_deleted"] {
        let _ = autostart::set(id, None);
    }
    // A program on the list whose folder was moved (its entry names the old place), and one that was deleted.
    toolbox.write("Moved/program.json", r#"{"id":"fake_moved","name":"Moved","exe":"Moved.exe","protocol":1,"background":true}"#);
    toolbox.write("Moved/Moved.exe", "");
    autostart::set("fake_moved", Some(Path::new(r"C:\Old Place\Moved.exe"))).unwrap();
    autostart::set("fake_deleted", Some(Path::new(r"C:\Deleted\Gone.exe"))).unwrap();

    let (window, first) = toolbox.window();
    assert_eq!(first["own"]["autostart"], json!({"state": "off", "target": null}));
    window.send(&json!({"type": "own_autostart", "on": true})).unwrap();
    library(&window, |l| l["own"]["autostart"]["state"] == "on");
    // The Toolbox's exe by its name (`Toolbox.exe`) – the same file as cargo's `toolbox.exe`.
    let lower = |v: &Value| v.as_str().unwrap_or_default().to_lowercase();
    let exe_text = exe.to_string_lossy().to_lowercase();
    assert_eq!(autostart::state("toolbox"), Autostart::On);
    assert_eq!(autostart::read("toolbox").target.unwrap().to_string_lossy().to_lowercase(), exe_text);
    window.send(&json!({"type": "start_menu", "on": true})).unwrap();
    let listed = library(&window, |l| !l["own"]["shortcut"].is_null());
    assert!(shortcut.is_file(), "the shortcut lies in the sandbox's folder");
    assert_eq!(lower(&listed["own"]["shortcut"]), exe_text);

    // Clean up: the deleted program's entry – not the Toolbox's, and not the moved one's, which is repaired.
    window.send(&json!({"type": "add", "folder": toolbox.root.join("Moved")})).unwrap();
    library(&window, |l| l["programs"].as_array().is_some_and(|p| p.len() == 1));
    window.send(&json!({"type": "traces", "everything": false})).unwrap();
    let found = until(&window, |told| (told["type"] == "traces").then(|| told.clone()));
    assert_eq!(found["traces"], json!([{"kind": "autostart", "id": "fake_deleted", "target": r"C:\Deleted\Gone.exe"}]));
    assert_eq!(autostart::read("fake_moved").target, Some(toolbox.root.join("Moved").join("Moved.exe")), "pointed at the new place");
    window.send(&json!({"type": "clean_up", "traces": found["traces"]})).unwrap();
    let text = until(&window, |told| (told["type"] == "toast").then(|| told["text"].clone()));
    assert_eq!(text, "Removed: 1.");
    assert_eq!(autostart::state("fake_deleted"), Autostart::Off);
    assert_eq!(autostart::state("toolbox"), Autostart::On, "Clean up leaves what belongs to a program");

    // Uninstall all lists everything; then both switches off again from the gear.
    window.send(&json!({"type": "traces", "everything": true})).unwrap();
    let all = until(&window, |told| (told["type"] == "traces").then(|| told["traces"].clone()));
    let kinds: Vec<String> = all.as_array().unwrap().iter().map(|t| format!("{}:{}", t["kind"].as_str().unwrap(), t["id"].as_str().unwrap_or("lnk"))).collect();
    assert!(kinds.contains(&"autostart:toolbox".into()) && kinds.contains(&"autostart:fake_moved".into()) && kinds.contains(&"shortcut:lnk".into()), "{kinds:?}");
    window.send(&json!({"type": "own_autostart", "on": false})).unwrap();
    library(&window, |l| l["own"]["autostart"]["state"] == "off");
    window.send(&json!({"type": "start_menu", "on": false})).unwrap();
    library(&window, |l| l["own"]["shortcut"].is_null());
    assert!(!shortcut.exists());

    // Uninstall all: every entry out of Windows, and the agent ends (in the sandbox it opens no folder).
    window.send(&json!({"type": "uninstall"})).unwrap();
    until(&window, |told| (told["type"] == "bye").then_some(()));
    toolbox.wait_exit();
    assert!(autostart::all().is_empty(), "nothing of ours is left: {:?}", autostart::all());
    // Then it says which folders can be deleted now (in the sandbox the message box goes to the log).
    let log = std::fs::read_to_string(toolbox.root.join("Toolbox").join("data").join("log.txt")).unwrap();
    let folder = toolbox.root.join("Toolbox").display().to_string();
    let said = log.split("simply delete these folders:").nth(1).unwrap_or_else(|| panic!("no farewell in the log: {log}"));
    assert!(said.lines().nth(1).is_some_and(|l| l == format!("• {folder}")), "{said}");
    let _ = Process::new("reg").args(["delete", &format!(r"HKCU\Software\WindowsToolbox\Sandbox{}", ipc::sandbox()), "/f"]).output();
}

/// Export and import all settings (DESIGN.md 8): one zip with the Toolbox's settings and each program's; the import
/// is shown first, then the files are put back – and a program that is not on the list is left out.
#[test]
fn settings_export_and_import() {
    let _turn = turn();
    let mut toolbox = Toolbox::start("backup");
    toolbox.write("Script/program.json", r#"{"id":"fake_script","name":"Script","run":"s.ps1","options":{"Pos":{"values":["Left","Right"],"default":"Right"}}}"#);
    toolbox.write("Script/s.ps1", "");
    let (window, _) = toolbox.window();
    window.send(&json!({"type": "add", "folder": toolbox.root.join("Script")})).unwrap();
    library(&window, |l| l["programs"].as_array().is_some_and(|p| p.len() == 1));
    window.send(&json!({"type": "set_option", "id": "fake_script", "name": "Pos", "value": "Left"})).unwrap();
    window.send(&json!({"type": "look", "language": "en", "theme": "light", "zoom": 1.5})).unwrap();
    library(&window, |l| l["settings"]["theme"] == "light" && l["programs"][0]["options"]["Pos"] == "Left");

    // More than the settings: a program's other files in data\ go along (a log does not).
    toolbox.write("Script/data/backup/list.txt", "before");
    toolbox.write("Script/data/log.txt", "log");
    let zip = toolbox.root.join("Backup.zip");
    window.send(&json!({"type": "export", "file": zip})).unwrap();
    let text = until(&window, |told| (told["type"] == "toast").then(|| told["text"].as_str().unwrap_or_default().to_string()));
    assert!(text.starts_with("Settings saved:") && text.ends_with("(the Toolbox + 1 programs)"), "{text}");

    // Everything changed afterwards …
    window.send(&json!({"type": "set_option", "id": "fake_script", "name": "Pos", "value": "Right"})).unwrap();
    window.send(&json!({"type": "look", "language": "en", "theme": "dark", "zoom": 1.0})).unwrap();
    library(&window, |l| l["settings"]["theme"] == "dark" && l["programs"][0]["options"]["Pos"] == "Right");
    toolbox.write("Script/data/backup/list.txt", "after");

    // … the plan first: nothing is changed by it.
    window.send(&json!({"type": "import_check", "file": zip})).unwrap();
    let plan = until(&window, |told| (told["type"] == "plan").then(|| told.clone()));
    assert_eq!(plan["Ok"]["toolbox"], true, "{plan}");
    assert_eq!(plan["Ok"]["programs"], json!(["Script"]));
    assert_eq!(toolbox.settings()["theme"], "dark");
    // … then the import puts it back.
    window.send(&json!({"type": "import", "file": zip})).unwrap();
    until(&window, |told| (told["type"] == "imported").then_some(()));
    let saved = toolbox.settings();
    assert_eq!((&saved["theme"], &saved["zoom"]), (&json!("light"), &json!(1.5)));
    assert_eq!(read_json(&toolbox.root.join("Script/data/settings.json"))["Pos"], "Left");
    assert_eq!(std::fs::read_to_string(toolbox.root.join("Script/data/backup/list.txt")).unwrap(), "before");

    // Not ours: said so, nothing changed.
    std::fs::write(toolbox.root.join("Other.zip"), b"not a zip").unwrap();
    window.send(&json!({"type": "import_check", "file": toolbox.root.join("Other.zip")})).unwrap();
    let plan = until(&window, |told| (told["type"] == "plan").then(|| told.clone()));
    assert!(plan["Err"].is_string(), "{plan}");

    // Off the list, the program is left out of an import.
    window.send(&json!({"type": "remove", "stored": r"..\Script"})).unwrap();
    library(&window, |l| l["programs"] == json!([]));
    std::fs::write(toolbox.root.join("Toolbox/data/settings.json"), r#"{"language": "en"}"#).unwrap();
    window.send(&json!({"type": "import_check", "file": zip})).unwrap();
    let plan = until(&window, |told| (told["type"] == "plan").then(|| told.clone()));
    // The Toolbox's list in the file names the script's folder, which exists: it comes back with the import.
    assert_eq!(plan["Ok"]["programs"], json!(["Script"]), "{plan}");

    window.send(&json!({"type": "quit"})).unwrap();
    toolbox.wait_exit();
}
