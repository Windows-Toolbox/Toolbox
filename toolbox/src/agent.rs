//! The agent role (DESIGN.md 8): the one tray icon with every program's menu and status, the list of programs,
//! and the pipe `windows-toolbox.toolbox`. It sleeps in the Windows message loop until a click or a message
//! arrives; only while a window is open it looks once a second at what cannot report by itself (scripts and plain
//! exes).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kit::ipc::{self, Conn};
use kit::autostart;
use kit::manifest::Kind;
use kit::process::{self, Job};
use kit::proto::{Command, Health, PROTOCOL, State, ToolboxHello, Tray as ProgramTray, Want};
use kit::settings::Settings;
use kit::tray::{self, Event, Icon, Item, Tray, Waker};
use kit::{app, i18n, t, update};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

use crate::catalog::{self, Offer};
use crate::library::{self, Entry, Library, Missing, Run, Target, Updatable, Updating};
use crate::talk::{Ask, Tell};

/// Fixed identities of the tray icons – Windows remembers "show this icon" per exe path and uid (DESIGN.md 6).
const ICON_TOOLBOX: u32 = 1;
const ICON_FIRST_LIVE: u32 = 2;
/// "Any process may come to the front" – for `AllowSetForegroundWindow`.
const ANY_PROCESS: u32 = u32::MAX;

/// A kit program the agent is connected to.
struct Link {
    conn: Arc<Conn>,
    tray: ProgramTray,
}

/// A connected window. Its lines are written by a thread of its own, so a window that has stopped reading can
/// never make the agent wait.
struct WindowLink {
    conn: Arc<Conn>,
    tx: SyncSender<String>,
}

/// What an entry of the tray menu does.
#[derive(Debug, Clone, PartialEq)]
enum Choice {
    Open(Option<String>),
    Start(String),
    Stop(String),
    Restart(String),
    /// An entry the program put into its menu: program id, action name.
    Program(String, String),
    Quit { all: bool },
    /// The Toolbox ends and starts anew; the programs keep running.
    RestartToolbox,
}

struct Agent {
    settings: Settings<library::Settings>,
    /// The Toolbox's folder; stored program folders are relative to it.
    base: PathBuf,
    programs: Vec<Entry>,
    missing: Vec<Missing>,
    links: BTreeMap<String, Link>,
    windows: Vec<WindowLink>,
    /// A window process was started and has not connected yet.
    window_started: Option<Instant>,
    /// `show` arrived while the window was starting: where it should go once it is there.
    pending_show: Option<Option<String>>,
    /// The entries of the tray menu as it was last shown; `Event::Chosen` carries the position + 1.
    menu: Vec<Choice>,
    /// Somebody asked the agent to end (`quit` on the pipe); the tray thread does it.
    quit: Option<bool>,
    /// … and to start anew right after (`restart_toolbox`).
    relaunch: bool,
    waker: Option<Waker>,
    /// The library as it was last sent to the window – unchanged, it is not sent again.
    last_sent: String,
    /// The thread that looks at scripts and plain exes once a second runs (only while a window is open).
    watching: bool,
    /// Updates under way, by program id (`library::OWN_ID`: the Toolbox) – "Get" shows its steps here too.
    updating: BTreeMap<String, Updating>,
    /// The list of "Get programs" as it was last loaded and checked; "Get" takes a program only from it.
    catalog: Vec<Offer>,
}

type Shared = Arc<Mutex<Agent>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Agent> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

impl Agent {
    fn entry(&self, id: &str) -> Option<&Entry> {
        self.programs.iter().find(|e| e.id() == id)
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.programs.iter_mut().find(|e| e.id() == id)
    }

    fn hello(&self) -> ToolboxHello {
        let s = &self.settings.value;
        ToolboxHello { protocol: PROTOCOL, language: s.language.clone(), theme: s.theme.clone(), zoom: s.zoom, want: vec![Want::Status, Want::Tray] }
    }

    fn library(&self) -> Library {
        Library { settings: self.settings.value.clone(), programs: self.programs.clone(), missing: self.missing.clone(), folder: self.base.clone(), data: app::data_dir(), updating: self.updating.clone() }
    }

    fn tell(&self, msg: &Tell) {
        if let Ok(line) = serde_json::to_string(msg) {
            for w in &self.windows {
                let _ = w.tx.try_send(line.clone());
            }
        }
    }

    /// Something the window or the tray shows may have changed.
    fn changed(&mut self) {
        if !self.windows.is_empty() {
            if let Ok(line) = serde_json::to_string(&Tell::Library(self.library())) {
                if line != self.last_sent {
                    for w in &self.windows {
                        let _ = w.tx.try_send(line.clone());
                    }
                    self.last_sent = line;
                }
            }
        }
        if let Some(waker) = self.waker {
            waker.wake();
        }
    }

    /// A message for the window's bottom bar. What went wrong also goes to the log – there may be no window.
    fn toast(&self, text: String, error: bool) {
        if error {
            app::log(&text);
        }
        self.tell(&Tell::Toast { text, error });
    }

    fn save(&mut self) {
        if let Err(e) = self.settings.save() {
            app::log(&format!("the settings could not be saved: {e}"));
        }
    }

    /// Reads the folders on the list again; what is known about running programs is carried over.
    fn reload(&mut self) {
        let (programs, missing) = library::load(&self.settings.value, &self.base, &self.programs);
        self.programs = programs;
        self.missing = missing;
        // A program that left the list is no longer followed.
        let gone: Vec<String> = self.links.keys().filter(|id| !self.programs.iter().any(|e| e.id() == id.as_str())).cloned().collect();
        for id in gone {
            if let Some(link) = self.links.remove(&id) {
                link.conn.close();
            }
        }
    }

    /// Looks at what does not report by itself: scripts (their job), plain exes (their process), and "Start with
    /// Windows" – Task Manager can switch it off at any time. Kit programs push their status. `true` if something
    /// changed.
    fn refresh(&mut self) -> bool {
        let mut changed = false;
        for e in &mut self.programs {
            changed |= e.read_autostart();
            let running = match e.kind {
                // Followed: its status is pushed. Not followed: its workers do not run.
                Kind::Kit if self.links.contains_key(e.id()) => continue,
                Kind::Kit => Some(false),
                Kind::Script => Some(script_running(e.id())),
                Kind::Exe => Some(e.exe().is_some_and(|exe| !process::running(&exe).is_empty())),
            };
            if e.run.running != running {
                // "Crashed" stays until the program runs again.
                e.run = Run { running, crashed: e.run.crashed && running != Some(true), ..Default::default() };
                changed = true;
            }
        }
        changed
    }
}

fn script_running(id: &str) -> bool {
    Job::open(id).is_some_and(|job| job.active_processes() > 0)
}

// ---------- kit programs ----------

/// Connects to a kit program and follows its status and tray data on a thread of its own. If the agent follows it
/// already, only `first` is sent.
fn follow(shared: &Shared, id: &str, first: Option<Command>) {
    {
        let a = lock(shared);
        if a.entry(id).is_none_or(|e| e.kind != Kind::Kit) {
            return;
        }
        if let Some(link) = a.links.get(id) {
            if let Some(cmd) = &first {
                let _ = link.conn.send(cmd);
            }
            return;
        }
    }
    let Ok(conn) = ipc::connect(id) else { return };
    {
        let mut a = lock(shared);
        if let Some(link) = a.links.get(id) {
            // Somebody else was quicker (the program announced itself twice).
            if let Some(cmd) = &first {
                let _ = link.conn.send(cmd);
            }
            return;
        }
        if conn.send(&Command::Hello(a.hello())).is_err() {
            return;
        }
        if let Some(cmd) = &first {
            let _ = conn.send(cmd);
        }
        a.links.insert(id.to_string(), Link { conn: conn.clone(), tray: ProgramTray::default() });
    }
    let (shared, id) = (shared.clone(), id.to_string());
    std::thread::spawn(move || {
        let mut bye = false;
        while let Some(state) = conn.read::<State>() {
            let mut a = lock(&shared);
            match state {
                State::Status(s) => {
                    if let Some(e) = a.entry_mut(&id) {
                        e.run = Run { running: Some(s.running), state: s.state, text: s.text, crashed: false };
                    }
                }
                State::Tray(tray) => {
                    if let Some(link) = a.links.get_mut(&id) {
                        link.tray = tray;
                    }
                }
                State::Bye => {
                    bye = true;
                    continue;
                }
                _ => continue,
            }
            a.changed();
        }
        let mut a = lock(&shared);
        // Not if the agent itself let go of it (the program was taken off the list).
        if a.links.get(&id).is_some_and(|l| Arc::ptr_eq(&l.conn, &conn)) {
            a.links.remove(&id);
            if let Some(e) = a.entry_mut(&id) {
                // A pipe that closes without `bye`: the program did not end on purpose (DESIGN.md 8).
                e.run = Run { running: Some(false), crashed: !bye, ..Default::default() };
            }
            if !bye {
                app::log(&format!("{id} ended without saying bye – crashed?"));
            }
            a.changed();
        }
    });
}

/// Connects to every kit program on the list whose process is there. Called at the start and when the list
/// changes; afterwards programs announce themselves (`started`), so nothing is polled.
fn follow_running(shared: &Shared) {
    let ids: Vec<String> = lock(shared).programs.iter().filter(|e| e.kind == Kind::Kit).map(|e| e.id().to_string()).collect();
    for id in ids {
        if ipc::exists(&id) {
            follow(shared, &id, None);
        }
    }
}

// ---------- start, stop, restart ----------

/// Some of these wait (a restart waits until the program has ended), so each runs on a thread of its own.
fn act(shared: &Shared, what: fn(&Shared, &str), id: String) {
    let shared = shared.clone();
    std::thread::spawn(move || {
        what(&shared, &id);
        // What was started or stopped needs a moment until it shows.
        std::thread::sleep(Duration::from_millis(400));
        let mut a = lock(&shared);
        if a.refresh() {
            a.changed();
        }
    });
}

fn name_of(entry: &Entry) -> String {
    entry.manifest.name.get(&i18n::language()).to_string()
}

fn start(shared: &Shared, id: &str) {
    let Some(entry) = lock(shared).entry(id).cloned() else { return };
    let failed = |why: String| lock(shared).toast(t!("toast.did_not_start", name = name_of(&entry), why = why), true);
    match entry.kind {
        Kind::Kit => {
            // Its process may be there already, serving its page with the workers off: then `start` goes to that
            // very process instead of launching a second one (DESIGN.md 4.4).
            if lock(shared).links.contains_key(id) || ipc::exists(id) {
                follow(shared, id, Some(Command::Start));
            } else if let Some(exe) = entry.exe() {
                // It announces itself (`started`) once its workers run.
                if let Err(e) = process::spawn(&exe, &[]) {
                    failed(e.to_string());
                }
            }
        }
        Kind::Exe => {
            if let Some(Err(e)) = entry.exe().map(|exe| process::spawn(&exe, &[])) {
                failed(e.to_string());
            }
        }
        Kind::Script => {
            if !script_running(id) {
                let started = library::script_command(&entry).and_then(|cmd| {
                    let job = Job::create(id).map_err(|e| e.to_string())?;
                    job.spawn(&cmd, Some(&entry.home)).map_err(|e| e.to_string())
                });
                if let Err(e) = started {
                    return failed(e);
                }
            }
            if let Some(launch) = &entry.manifest.launch {
                if !process::open(launch) {
                    failed(launch.clone());
                }
            }
        }
    }
}

fn stop(shared: &Shared, id: &str) {
    let Some(entry) = lock(shared).entry(id).cloned() else { return };
    match entry.kind {
        // Switches the workers off; the process ends by itself when nobody looks at its page.
        Kind::Kit => {
            if let Some(link) = lock(shared).links.get(id) {
                let _ = link.conn.send(&Command::Stop);
            }
        }
        // The script together with everything it started.
        Kind::Script => {
            if let Some(job) = Job::open(id) {
                job.terminate();
            }
        }
        Kind::Exe => {}
    }
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while !done() {
        if t.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

fn restart(shared: &Shared, id: &str) {
    let Some(entry) = lock(shared).entry(id).cloned() else { return };
    let ended = match entry.kind {
        Kind::Kit => {
            if let Some(link) = lock(shared).links.get(id) {
                let _ = link.conn.send(&Command::Quit);
            }
            wait_until(Duration::from_secs(5), || !ipc::exists(id))
        }
        Kind::Script => {
            stop(shared, id);
            wait_until(Duration::from_secs(5), || !script_running(id))
        }
        Kind::Exe => return,
    };
    if ended {
        start(shared, id);
    } else {
        lock(shared).toast(t!("toast.did_not_stop", name = name_of(&entry)), true);
    }
}

// ---------- updates ----------

/// What one update came to.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    /// Now at this version.
    Updated(String),
    /// The installed version is the latest.
    UpToDate,
    /// The repository has no release yet.
    NoRelease,
    Failed(String),
}

/// The Toolbox itself, as something to update: its manifest is compiled in, its folder is the agent's.
fn own_target(base: &std::path::Path) -> Option<Target> {
    let manifest = kit::manifest::Manifest::parse(crate::META.manifest).ok()?;
    Some(Target::of(&manifest, base))
}

fn target_of(a: &Agent, id: &str) -> Option<Target> {
    if id == library::OWN_ID { own_target(&a.base) } else { a.entry(id).map(Entry::target) }
}

/// Before the files are swapped, a kit program ends – the page-only process too, so the window reconnects to the
/// new exe. Scripts and plain exes are left alone: a running file is put aside and keeps running, the new one
/// counts from their next start (a script is only read when it starts). `true` if its workers ran.
fn make_way(shared: &Shared, id: &str) -> bool {
    let Some(entry) = lock(shared).entry(id).cloned() else { return false };
    if entry.kind != Kind::Kit || !ipc::exists(id) {
        return false;
    }
    let ran = entry.run.running == Some(true);
    let link = lock(shared).links.get(id).map(|l| l.conn.clone());
    match link {
        Some(conn) => {
            let _ = conn.send(&Command::Quit);
        }
        None => {
            if let Ok(conn) = ipc::connect(id) {
                let _ = conn.send(&Command::Quit);
            }
        }
    }
    if !wait_until(Duration::from_secs(5), || !ipc::exists(id)) {
        app::log(&format!("{id} did not end before its update; its files are put aside"));
    }
    ran
}

/// One update, start to end (DESIGN.md 9): ask GitHub, and only if there is a newer release load it, check its
/// signature against the installed version's key, make way, swap the files, start again what ran. The page shows
/// each step.
fn update_one(shared: &Shared, target: &Target) -> Outcome {
    let step = |s: Option<Updating>| set_step(shared, &target.id, s);
    step(Some(Updating::Checking));
    let result = (|| -> Result<Outcome, String> {
        let (Some(repository), Some(key)) = (&target.repository, &target.key) else { return Err(t!("update.not_possible")) };
        let Some(release) = update::latest(repository, &target.id)? else { return Ok(Outcome::NoRelease) };
        if update::compare_versions(&release.version, &target.version) != std::cmp::Ordering::Greater {
            return Ok(Outcome::UpToDate);
        }
        step(Some(Updating::Downloading(release.version.clone())));
        let zip = update::download(&release, key)?;
        step(Some(Updating::Installing(release.version.clone())));
        let own = target.id == library::OWN_ID;
        let ran = !own && make_way(shared, &target.id);
        let installed = update::install(&zip, &target.home, &target.id, Some(&target.version));
        if !own {
            // The new manifest (its version) – or, after a failure, the old files, which are back in place.
            let mut a = lock(shared);
            a.reload();
            a.refresh();
            a.changed();
        }
        if ran {
            start(shared, &target.id);
        }
        Ok(Outcome::Updated(installed?.version))
    })();
    step(None);
    result.unwrap_or_else(Outcome::Failed)
}

/// How far the update (or "Get") of `id` is – `None`: done.
fn set_step(shared: &Shared, id: &str, step: Option<Updating>) {
    let mut a = lock(shared);
    match step {
        Some(s) => a.updating.insert(id.to_string(), s),
        None => a.updating.remove(id),
    };
    a.changed();
}

/// The message for one outcome.
fn outcome_text(target: &Target, outcome: &Outcome) -> (String, bool) {
    let name = target.name.get(&i18n::language()).to_string();
    match outcome {
        Outcome::Updated(version) => (t!("toast.update.updated", name = name, version = version), false),
        Outcome::UpToDate => (t!("toast.update.up_to_date", name = name, version = target.version), false),
        Outcome::NoRelease => (t!("toast.update.no_release", name = name), false),
        Outcome::Failed(why) => (t!("toast.update.failed", name = name, why = why), true),
    }
}

/// The Toolbox was updated: it ends and the new version starts (the window too, if one is open).
fn restart_updated(shared: &Shared) {
    let mut a = lock(shared);
    a.quit = Some(false);
    a.relaunch = true;
    if let Some(waker) = a.waker {
        waker.wake();
    }
}

/// "Update" on a page, on a thread of its own: it waits for GitHub.
fn update_program(shared: &Shared, id: String) {
    let target = {
        let a = lock(shared);
        if !a.updating.is_empty() {
            return a.toast(t!("toast.update.busy"), false);
        }
        match target_of(&a, &id) {
            Some(t) => t,
            None => return,
        }
    };
    let shared = shared.clone();
    std::thread::spawn(move || {
        let outcome = update_one(&shared, &target);
        let (text, error) = outcome_text(&target, &outcome);
        lock(&shared).toast(text, error);
        if target.id == library::OWN_ID && matches!(outcome, Outcome::Updated(_)) {
            restart_updated(&shared);
        }
    });
}

/// "Update all": one program after the other, the Toolbox last (it restarts afterwards). One message at the end.
fn update_all(shared: &Shared) {
    let targets: Vec<Target> = {
        let a = lock(shared);
        if !a.updating.is_empty() {
            return a.toast(t!("toast.update.busy"), false);
        }
        a.programs.iter().map(Entry::target).chain(own_target(&a.base)).filter(|t| t.updatable() == Updatable::Yes).collect()
    };
    if targets.is_empty() {
        return lock(shared).toast(t!("toast.update.nothing"), false);
    }
    let shared = shared.clone();
    std::thread::spawn(move || {
        let (mut updated, mut current, mut problems, mut restart) = (0, 0, vec![], false);
        for target in &targets {
            let outcome = update_one(&shared, target);
            match &outcome {
                Outcome::Updated(_) => {
                    updated += 1;
                    restart |= target.id == library::OWN_ID;
                }
                Outcome::UpToDate => current += 1,
                Outcome::NoRelease | Outcome::Failed(_) => problems.push(outcome_text(target, &outcome).0),
            }
        }
        let mut text = t!("toast.update.all", updated = updated, current = current);
        if !problems.is_empty() {
            text = format!("{text} {}", problems.join(" · "));
        }
        lock(&shared).toast(text, !problems.is_empty());
        if restart {
            restart_updated(&shared);
        }
    });
}

// ---------- get programs ----------

/// "Get programs": loads the signed list (on a thread: it waits for GitHub) and sends it to the window.
fn load_catalog(shared: &Shared) {
    let shared = shared.clone();
    std::thread::spawn(move || {
        let offers = catalog::load();
        if let Err(e) = &offers {
            app::log(&format!("the list of programs could not be loaded: {e}"));
        }
        let mut a = lock(&shared);
        if let Ok(list) = &offers {
            a.catalog = list.clone();
        }
        a.tell(&Tell::Catalog { offers });
    });
}

/// "Get": the program's latest release, checked against the key the signed list names for it, unpacked into
/// `programs\<id>\` next to the Toolbox and put on the list – then its page is shown.
fn get_program(shared: &Shared, id: String) {
    let (offer, base) = {
        let a = lock(shared);
        if !a.updating.is_empty() {
            return a.toast(t!("toast.update.busy"), false);
        }
        if let Some(listed) = a.entry(&id) {
            return a.toast(t!("toast.get.listed", name = name_of(listed)), false);
        }
        match a.catalog.iter().find(|o| o.id == id) {
            Some(offer) => (offer.clone(), a.base.clone()),
            None => return,
        }
    };
    let shared = shared.clone();
    std::thread::spawn(move || {
        let step = |s: Option<Updating>| set_step(&shared, &offer.id, s);
        step(Some(Updating::Checking));
        let folder = catalog::folder(&base, &offer.id);
        let result = (|| -> Result<String, String> {
            let Some(release) = update::latest(&offer.repository, &offer.id)? else { return Err(t!("get.no_release")) };
            step(Some(Updating::Downloading(release.version.clone())));
            let zip = update::download(&release, &offer.update_key)?;
            step(Some(Updating::Installing(release.version.clone())));
            let installed = update::install(&zip, &folder, &offer.id, None)?;
            let mut a = lock(&shared);
            let a = &mut *a;
            let added = library::add(&mut a.settings.value, &a.programs, &a.base, &folder);
            a.save();
            a.reload();
            a.refresh();
            a.changed();
            if added.added.is_empty() {
                return Err(added.broken.first().cloned().unwrap_or_else(|| t!("get.not_added")));
            }
            Ok(installed.version)
        })();
        step(None);
        let name = offer.name.get(&i18n::language()).to_string();
        let a = lock(&shared);
        match result {
            Ok(version) => {
                a.toast(t!("toast.get.done", name = name, version = version), false);
                a.tell(&Tell::Show { id: Some(offer.id.clone()) });
            }
            Err(why) => a.toast(t!("toast.get.failed", name = name, why = why), true),
        }
    });
}

// ---------- the window ----------

/// Opens the window – or brings the open one to the front – on this program's page if given.
fn open_window(a: &mut Agent, id: Option<String>) {
    // The click happened here (tray) or in a second start of the exe; the window is another process and may
    // come to the front for it.
    unsafe { AllowSetForegroundWindow(ANY_PROCESS) };
    if !a.windows.is_empty() {
        return a.tell(&Tell::Show { id });
    }
    if a.window_started.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
        // It is starting: it gets this as soon as it has connected.
        a.pending_show = Some(id);
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut args = vec![crate::WINDOW];
    if let Some(id) = &id {
        args.push(id);
    }
    match process::spawn(&exe, &args) {
        Ok(_) => a.window_started = Some(Instant::now()),
        Err(e) => app::log(&format!("the window could not be started: {e}")),
    }
}

/// While a window is open: once a second, look at what does not report by itself.
fn watch(shared: &Shared) {
    let shared = shared.clone();
    std::thread::spawn(move || {
        loop {
            {
                let mut a = lock(&shared);
                if a.windows.is_empty() {
                    a.watching = false;
                    return;
                }
                if a.refresh() {
                    a.changed();
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn serve(shared: Shared, conn: Arc<Conn>) {
    let mut is_window = false;
    while let Some(ask) = conn.read::<Ask>() {
        if ask == Ask::Window {
            if !is_window {
                is_window = true;
                let (tx, rx) = sync_channel::<String>(64);
                {
                    let conn = conn.clone();
                    std::thread::spawn(move || {
                        for line in rx {
                            if conn.send_line(&line).is_err() {
                                conn.close();
                                break;
                            }
                        }
                    });
                }
                let mut a = lock(&shared);
                a.windows.push(WindowLink { conn: conn.clone(), tx });
                a.window_started = None;
                a.refresh();
                a.last_sent.clear();
                a.changed();
                if let Some(id) = a.pending_show.take() {
                    a.tell(&Tell::Show { id });
                }
                if !a.watching {
                    a.watching = true;
                    watch(&shared);
                }
            }
            continue;
        }
        handle(&shared, ask);
    }
    if is_window {
        lock(&shared).windows.retain(|w| !Arc::ptr_eq(&w.conn, &conn));
    }
}

fn handle(shared: &Shared, ask: Ask) {
    match ask {
        Ask::Show { id } => open_window(&mut lock(shared), id),
        Ask::Started { id } => follow(shared, &id, None),
        Ask::Quit { all } => {
            let mut a = lock(shared);
            a.quit = Some(all);
            if let Some(waker) = a.waker {
                waker.wake();
            }
        }
        Ask::RestartToolbox => {
            let mut a = lock(shared);
            a.quit = Some(false);
            a.relaunch = true;
            if let Some(waker) = a.waker {
                waker.wake();
            }
        }
        Ask::Look { language, theme, zoom } => {
            let mut a = lock(shared);
            // What is invalid leaves that part as it is.
            let now = &a.settings.value;
            let new = library::Settings {
                language: if i18n::is_language(&language) { language } else { now.language.clone() },
                theme: if kit::ui::THEMES.contains(&theme.as_str()) { theme } else { now.theme.clone() },
                zoom: if (0.5..=3.0).contains(&zoom) { zoom } else { now.zoom },
                programs: now.programs.clone(),
            };
            if new == a.settings.value {
                return;
            }
            a.settings.value = new;
            a.save();
            i18n::set_language(&a.settings.value.language);
            // Every running kit program switches along – and remembers it for its own windows (DESIGN.md 4.3).
            let hello = Command::Hello(a.hello());
            for link in a.links.values() {
                let _ = link.conn.send(&hello);
            }
            a.changed();
        }
        Ask::Add { folder } => {
            {
                let mut a = lock(shared);
                let a = &mut *a;
                let added = library::add(&mut a.settings.value, &a.programs, &a.base, &folder);
                a.save();
                a.reload();
                a.refresh();
                a.tell(&Tell::Added(added));
                a.changed();
            }
            follow_running(shared);
        }
        Ask::Replace { folder } => {
            {
                let mut a = lock(shared);
                let a = &mut *a;
                if let Err(e) = library::replace(&mut a.settings.value, &a.programs, &a.base, &folder) {
                    return a.toast(format!("{}: {e}", folder.display()), true);
                }
                a.save();
                a.reload();
                a.refresh();
                a.changed();
            }
            follow_running(shared);
        }
        Ask::Remove { stored } => {
            let mut a = lock(shared);
            // Off the list, a program also stops starting with Windows: the Toolbox would no longer show the switch.
            // Only an entry that starts this copy – another copy's is left alone.
            if let Some(entry) = a.programs.iter().find(|e| e.stored == stored).filter(|e| e.autostart_is_mine()) {
                if let Err(e) = autostart::set(entry.id(), None) {
                    app::log(&format!("the autostart entry of {} could not be removed: {e}", entry.id()));
                }
            }
            a.settings.value.programs.retain(|s| s != &stored);
            a.save();
            a.reload();
            a.changed();
        }
        Ask::Start { id } => act(shared, start, id),
        Ask::Stop { id } => act(shared, stop, id),
        Ask::Restart { id } => act(shared, restart, id),
        Ask::Autostart { id, on } => {
            let mut a = lock(shared);
            let Some(entry) = a.entry(&id).filter(|e| e.has_autostart()).cloned() else { return };
            let result = match entry.exe() {
                Some(exe) if on => autostart::set(&id, Some(&exe)),
                // Off: only an entry that starts this copy.
                _ if !on && entry.autostart_is_mine() => autostart::set(&id, None),
                _ => Ok(()),
            };
            if let Err(e) = result {
                a.toast(t!("toast.autostart_failed", name = name_of(&entry), why = e), true);
            }
            if let Some(e) = a.entry_mut(&id) {
                e.read_autostart();
            }
            // Also after a refusal: the window then shows what holds.
            a.last_sent.clear();
            a.changed();
        }
        Ask::SetOption { id, name, value } => {
            let mut a = lock(shared);
            if let Some(Err(e)) = a.entry_mut(&id).map(|e| e.set_option(&name, value)) {
                a.toast(e, true);
            }
            // Also after a refusal: the window then shows the value that still holds.
            a.last_sent.clear();
            a.changed();
        }
        Ask::Update { id } => update_program(shared, id),
        Ask::UpdateAll => update_all(shared),
        Ask::Catalog => load_catalog(shared),
        Ask::Get { id } => get_program(shared, id),
        Ask::Window | Ask::Unknown => {}
    }
}

// ---------- the tray ----------

fn health_color(state: Health) -> u32 {
    // 0x00BBGGRR – the colours of `kit::ui` (GOOD, WARN, BAD).
    match state {
        Health::Warn => 0x0051_c4f5,
        Health::Bad => 0x0064_64ff,
        _ => 0x0097_dc3d,
    }
}

/// The Toolbox icon with every running program's status line as its tooltip, and the live displays next to it.
/// `live_shown`: how many live displays there were the last time.
fn show_icons(a: &Agent, tray: &mut Tray, live_shown: &mut u32) {
    let language = i18n::language();
    let mut tip = String::from("Windows Toolbox");
    let mut live = vec![];
    for e in &a.programs {
        let Some(link) = a.links.get(e.id()) else { continue };
        if e.run.running != Some(true) {
            continue;
        }
        let line = if link.tray.line.is_empty() { &e.run.text } else { &link.tray.line };
        if !line.is_empty() {
            tip += &format!("\n{}: {line}", e.manifest.name.get(&language));
        }
        live.extend(link.tray.live.iter().take(3).cloned());
    }
    tray.set_icon(ICON_TOOLBOX, Icon::Program, &tip);
    for (i, display) in live.iter().enumerate() {
        let text: String = display.text.chars().take(3).collect();
        tray.set_icon(ICON_FIRST_LIVE + i as u32, Icon::Text(&text, health_color(display.state)), &tip);
    }
    for i in live.len() as u32..*live_shown {
        tray.remove_icon(ICON_FIRST_LIVE + i);
    }
    *live_shown = live.len() as u32;
}

/// The tray menu (DESIGN.md 6): one line per program with its submenu, one level deep. What each entry does is
/// noted in `a.menu`; an entry's id is its position there + 1.
fn build_menu(a: &mut Agent) -> Vec<Item> {
    let language = i18n::language();
    let mut choices: Vec<Choice> = vec![];
    let mut entry = |label: String, choice: Choice| {
        choices.push(choice);
        Item::entry(choices.len() as u32, label)
    };
    let mut items = vec![entry(t!("tray.open"), Choice::Open(None)), Item::Separator];
    for e in &a.programs {
        let id = e.id().to_string();
        let running = e.run.running == Some(true);
        let link = a.links.get(&id);
        let start_label = e.manifest.button.as_ref().map(|b| b.get(&language).to_string()).filter(|b| !b.is_empty()).unwrap_or_else(|| t!("page.start"));
        let mut sub = vec![];
        match e.kind {
            Kind::Kit => {
                if !running {
                    sub.push(entry(start_label, Choice::Start(id.clone())));
                }
                sub.push(entry(t!("tray.settings"), Choice::Open(Some(id.clone()))));
                if running {
                    // What the program wants in its menu.
                    for m in link.iter().flat_map(|l| &l.tray.menu) {
                        sub.push(entry(m.label.clone(), Choice::Program(id.clone(), m.action.clone())));
                    }
                    sub.push(entry(t!("page.restart"), Choice::Restart(id.clone())));
                    sub.push(entry(t!("page.stop"), Choice::Stop(id.clone())));
                }
            }
            Kind::Script => {
                sub.push(if running { entry(t!("page.stop"), Choice::Stop(id.clone())) } else { entry(start_label, Choice::Start(id.clone())) });
            }
            Kind::Exe => {
                sub.push(if running { Item::info(t!("status.running")) } else { entry(start_label, Choice::Start(id.clone())) });
            }
        }
        let dot = if e.run.crashed {
            "⚠"
        } else if running {
            "●"
        } else {
            "○"
        };
        let name = e.manifest.name.get(&language);
        let line = link.map(|l| l.tray.line.as_str()).filter(|l| !l.is_empty() && running).unwrap_or(if running { &e.run.text } else { "" });
        // A tab puts the status line into the menu's right column.
        let label = if line.is_empty() { format!("{dot} {name}") } else { format!("{dot} {name}\t{line}") };
        items.push(Item::Sub { label, items: sub });
    }
    if !a.programs.is_empty() {
        items.push(Item::Separator);
    }
    let quit = vec![
        entry(t!("tray.quit.toolbox"), Choice::Quit { all: false }),
        entry(t!("tray.quit.all"), Choice::Quit { all: true }),
        entry(t!("toolbox.restart"), Choice::RestartToolbox),
    ];
    items.push(Item::Sub { label: t!("tray.quit"), items: quit });
    a.menu = choices;
    items
}

/// Ends the agent's work: with `all`, every background program is asked to quit first.
fn quit_now(a: &mut Agent, all: bool) {
    if all {
        for link in a.links.values() {
            let _ = link.conn.send(&Command::Quit);
        }
    }
    a.tell(&Tell::Bye);
}

/// "Restart the Toolbox": starts the new agent, which waits until this one has ended. A window that is open now
/// opens again; with none, the new agent stays in the tray.
fn relaunch(a: &Agent) {
    let exe = own_exe();
    let pid = std::process::id().to_string();
    let mut args = vec![crate::AFTER, pid.as_str()];
    if a.windows.is_empty() {
        args.push(crate::BACKGROUND);
    }
    if let Err(e) = process::spawn(&exe, &args) {
        app::log(&format!("the Toolbox could not be started again: {e}"));
    }
}

/// The Toolbox's exe by its name – not necessarily the running file: `build.ps1` renames a running exe aside
/// (`Toolbox.<time>.old.exe`) and puts the new build under the name; a restart is meant to start that one.
fn own_exe() -> PathBuf {
    let running = std::env::current_exe().unwrap_or_default();
    let named = kit::manifest::Manifest::parse(crate::META.manifest).ok().and_then(|m| m.exe).and_then(|exe| Some(running.parent()?.join(exe)));
    named.filter(|p| p.is_file()).unwrap_or(running)
}

fn chosen(shared: &Shared, tray: &mut Tray, choice: Choice) {
    match choice {
        Choice::Open(id) => open_window(&mut lock(shared), id),
        Choice::Start(id) => act(shared, start, id),
        Choice::Stop(id) => act(shared, stop, id),
        Choice::Restart(id) => act(shared, restart, id),
        Choice::Program(id, name) => {
            if let Some(link) = lock(shared).links.get(&id) {
                let _ = link.conn.send(&Command::Action { name });
            }
        }
        Choice::Quit { all } => {
            quit_now(&mut lock(shared), all);
            tray.quit();
        }
        Choice::RestartToolbox => {
            let mut a = lock(shared);
            quit_now(&mut a, false);
            relaunch(&a);
            tray.quit();
        }
    }
}

// ---------- the role ----------

/// Runs the agent until it is asked to quit. `open`: also open the window (a double-click) – not when started
/// with Windows.
pub fn run(open: bool) {
    let server = match ipc::Server::bind(ipc::TOOLBOX) {
        Ok(server) => server,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // An agent runs already: the second double-click only opens its window.
            if open {
                unsafe { AllowSetForegroundWindow(ANY_PROCESS) };
                if let Ok(conn) = ipc::connect(ipc::TOOLBOX) {
                    let _ = conn.send(&Ask::Show { id: None });
                }
            }
            return;
        }
        Err(e) => return app::log(&format!("the pipe could not be created: {e}")),
    };

    let base = app::program_dir();
    let mut settings = Settings::<library::Settings>::load(app::data_dir().join("settings.json"));
    settings.value = settings.value.clone().checked();
    if !settings.existed {
        if let Err(e) = settings.save() {
            app::log(&format!("the settings could not be saved: {e}"));
        }
    }
    i18n::set_language(&settings.value.language);
    let (programs, missing) = library::load(&settings.value, &base, &[]);
    let agent = Agent {
        settings,
        base,
        programs,
        missing,
        links: BTreeMap::new(),
        windows: vec![],
        window_started: None,
        pending_show: None,
        menu: vec![],
        quit: None,
        relaunch: false,
        waker: None,
        last_sent: String::new(),
        watching: false,
        updating: BTreeMap::new(),
        catalog: vec![],
    };
    let shared: Shared = Arc::new(Mutex::new(agent));

    let waker = tray::create({
        let shared = shared.clone();
        let mut live_shown = 0;
        move |tray, event| {
            match event {
                Event::Wake => {
                    let mut a = lock(&shared);
                    if let Some(all) = a.quit.take() {
                        quit_now(&mut a, all);
                        if std::mem::take(&mut a.relaunch) {
                            relaunch(&a);
                        }
                        tray.quit();
                    } else {
                        show_icons(&a, tray, &mut live_shown);
                    }
                }
                Event::LeftClick => open_window(&mut lock(&shared), None),
                Event::Menu => {
                    let mut a = lock(&shared);
                    // The menu shows what is true now, also for what does not report by itself.
                    if a.refresh() {
                        a.changed();
                    }
                    return build_menu(&mut a);
                }
                Event::Chosen(n) => {
                    let choice = lock(&shared).menu.get(n as usize - 1).cloned();
                    if let Some(choice) = choice {
                        chosen(&shared, tray, choice);
                    }
                }
            }
            vec![]
        }
    });
    lock(&shared).waker = Some(waker);

    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            loop {
                match server.accept() {
                    Ok(conn) => {
                        let shared = shared.clone();
                        std::thread::spawn(move || serve(shared, conn));
                    }
                    Err(e) => {
                        app::log(&format!("accepting a connection failed: {e}"));
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        });
    }

    follow_running(&shared);
    if open {
        open_window(&mut lock(&shared), None);
    }
    waker.wake();
    tray::run_loop();
    // Give the window's writer a moment to hand over the `bye`.
    std::thread::sleep(Duration::from_millis(150));
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::manifest::Manifest;
    use kit::proto::MenuEntry;

    fn entry(json: &str, running: Option<bool>) -> Entry {
        let manifest = Manifest::parse(json).unwrap();
        let kind = manifest.kind();
        Entry { stored: manifest.id.clone(), folder: PathBuf::new(), home: PathBuf::new(), manifest, kind, options: Default::default(), run: Run { running, ..Default::default() }, autostart: None }
    }

    fn agent(programs: Vec<Entry>) -> Agent {
        i18n::load(include_str!("../assets/i18n.json"));
        i18n::set_language("en");
        Agent {
            settings: Settings::load(std::env::temp_dir().join("toolbox-agent-test-none.json")),
            base: PathBuf::new(),
            programs,
            missing: vec![],
            links: BTreeMap::new(),
            windows: vec![],
            window_started: None,
            pending_show: None,
            menu: vec![],
            quit: None,
            relaunch: false,
            waker: None,
            last_sent: String::new(),
            watching: false,
            updating: BTreeMap::new(),
            catalog: vec![],
        }
    }

    fn labels(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                Item::Entry { label, .. } => label.clone(),
                Item::Sub { label, items } => format!("{label} > {}", labels(items).join(" | ")),
                Item::Separator => "-".into(),
            })
            .collect()
    }

    /// The tray menu as data (DESIGN.md 13): which entries, and what each one does.
    #[test]
    fn the_tray_menu() {
        let mut a = agent(vec![
            entry(r#"{"id":"monitor","name":"Monitor","exe":"M.exe","protocol":1,"background":true}"#, Some(true)),
            entry(r#"{"id":"rayman","name":"Rayman","run":"r.ps1","button":{"de":"Spielen","en":"Play"}}"#, Some(false)),
            entry(r#"{"id":"game","name":"Game","exe":"G.exe"}"#, Some(true)),
        ]);
        a.programs[0].run.text = "52 °C".into();
        // A link without a real pipe is not needed for the menu: the program's entries come from its tray data.
        let menu = build_menu(&mut a);
        assert_eq!(
            labels(&menu),
            [
                "Open Toolbox",
                "-",
                "● Monitor\t52 °C > Settings | Restart | Stop",
                "○ Rayman > Play",
                "● Game > Running",
                "-",
                "Quit > Only the Toolbox | Toolbox and all programs | Restart the Toolbox",
            ]
        );
        assert_eq!(a.menu[0], Choice::Open(None));
        assert_eq!(a.menu[1], Choice::Open(Some("monitor".into())));
        assert_eq!(a.menu[3], Choice::Stop("monitor".into()));
        assert_eq!(a.menu[4], Choice::Start("rayman".into()));
        assert_eq!(a.menu.last(), Some(&Choice::RestartToolbox));
        assert!(own_exe().is_file());
        // The ids are the positions: the fourth entry is "Stop" of the monitor.
        let Item::Sub { items, .. } = &menu[2] else { panic!() };
        assert!(matches!(&items[2], Item::Entry { id: 4, .. }));
        let _ = MenuEntry::default();
    }

    #[test]
    fn a_crashed_program_shows_it() {
        let mut a = agent(vec![entry(r#"{"id":"monitor","name":"Monitor","exe":"M.exe","protocol":1}"#, Some(false))]);
        a.programs[0].run.crashed = true;
        assert_eq!(labels(&build_menu(&mut a))[2], "⚠ Monitor > Start | Settings");
    }
}
