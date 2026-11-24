//! What every exe of ours does first (`init`), and the background role of a kit program (`run`, DESIGN.md 5):
//! single instance through the pipe, `hello`, pushing states to every connection that wants them, applying `set`,
//! saving with a short delay, ending when the workers are off and nobody looks at the page any more.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ipc::{self, Conn};
use crate::page::{self, Builder, Page};
use crate::proto::{AgentCommand, Command, PROTOCOL, State, Status, ToolboxHello, Tray, Want};
use crate::settings::Settings;
use crate::{Meta, i18n, tray};

/// Internal argument between our own processes (never typed by a user): start with the workers off, only to
/// serve the settings page (DESIGN.md 4.4).
pub const WORKERS_OFF: &str = "--workers-off";

/// A kit program: its settings struct, its page, its status – everything else is the kit's job.
pub trait Program: Send + 'static {
    type Settings: Serialize + DeserializeOwned + Default + Send;
    /// `const META: kit::Meta = kit::meta!();`
    const META: Meta;

    /// The settings page (4.4). Called again after every change.
    fn page(&self, p: &mut Builder, s: &Self::Settings);
    /// What the Toolbox shows under the name. `running`: are the workers on?
    fn status(&self, running: bool) -> Status;
    /// What the program wants in the tray: status line, menu entries, live displays (section 6).
    fn tray(&self) -> Tray {
        Tray::default()
    }
    /// Called with the new settings after a `set` – and once at the start with the loaded ones.
    fn applied(&mut self, _s: &Self::Settings) {}
    /// A button of the page or an entry of the tray menu. It may change settings (a reset, a switch in the tray
    /// menu) – they are applied and saved like after a `set`. The returned text is shown as a toast.
    fn action(&mut self, _name: &str, _s: &mut Self::Settings) -> Option<String> {
        None
    }
    /// `show`: open the program's own window and bring it to the front.
    fn show(&mut self) {}
    /// The program is about to end on purpose (`quit`, or nobody needs it any more).
    fn shutdown(&mut self) {}
}

// ---------- what every process does first ----------

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// First thing in `main` of every exe (`run` calls it itself): one application id for all processes of the
/// program, so Windows treats them as one (taskbar, pinning – section 8); where the data lives; a panic goes to
/// `log.txt` instead of nowhere.
pub fn init(meta: &Meta) {
    let aumid = ipc::wide(&format!("WindowsToolbox.{}", meta.id));
    unsafe { windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(aumid.as_ptr()) };
    let _ = DATA_DIR.set(find_data_dir(meta));
    // Started by kit::setup with admin rights: do the job and end – nothing else of the program runs.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == crate::setup::HELPER) {
        crate::setup::helper(Path::new(args.get(2).map_or("", String::as_str)));
    }
    std::panic::set_hook(Box::new(|info| {
        log(&format!("PANIC: {info}"));
        // A program whose thread died would look alive but do nothing. Ending makes the crash visible: the pipe
        // closes without `bye`, and the Toolbox shows "crashed" with a link to this log (section 8).
        std::process::exit(101);
    }));
    // Tests give the program a data folder of their own; that one is always fine.
    if std::env::var_os(data_dir_variable(meta.id)).is_none() {
        if let Some(id) = wrong_place(&program_dir(), &data_dir()) {
            let manifest = crate::manifest::Manifest::parse(meta.manifest).ok();
            crate::i18n::set_language(crate::i18n::system_language());
            let lang = crate::i18n::language();
            let name = manifest.as_ref().map(|m| m.name.get(&lang).to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| meta.id.to_string());
            let exe = manifest.and_then(|m| m.exe).unwrap_or_default();
            let text = crate::i18n::format(id, &[("name", name.clone()), ("exe", exe), ("folder", program_dir().display().to_string())]);
            message_box(&name, &text, true);
            std::process::exit(1);
        }
    }
}

/// The two classic mistakes (DESIGN.md 9), caught before the program runs half-working and loses its settings:
/// started from inside a zip – Windows then runs it from a temporary folder – or from a folder it cannot write to
/// (Program Files). The id of the text that says what to do instead; `None` if all is well.
pub fn wrong_place(folder: &Path, data: &Path) -> Option<&'static str> {
    if in_zip_preview(folder) {
        return Some("kit.start.in_zip");
    }
    if !writable(data) {
        return Some("kit.start.read_only");
    }
    None
}

/// The temporary folders zip programs run a file from: Explorer `Temp1_<name>.zip`, 7-Zip `7zO…`, WinRAR `Rar$EX…`.
fn in_zip_preview(folder: &Path) -> bool {
    folder.components().any(|c| {
        let part = c.as_os_str().to_string_lossy().to_lowercase();
        (part.starts_with("temp") && part.ends_with(".zip")) || part.starts_with("7zo") || part.starts_with("rar$ex")
    })
}

/// Whether the program can keep its settings: `data\` is there or can be made, and a file can be written into it.
fn writable(data: &Path) -> bool {
    let probe = data.join(".write-test");
    let ok = std::fs::create_dir_all(data).is_ok() && std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Internal, for tests – they must never touch the settings that are really in use: the environment variable
/// that gives the program `id` another data folder. One per program, because a started program inherits the
/// variables of whoever started it, and the Toolbox's folder must not become the folder of a program it starts.
pub fn data_dir_variable(id: &str) -> String {
    format!("TOOLBOX_DATA_DIR_{}", id.to_uppercase())
}

fn find_data_dir(meta: &Meta) -> PathBuf {
    if let Some(dir) = std::env::var_os(data_dir_variable(meta.id)) {
        return PathBuf::from(dir);
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    // Started from cargo's build folder (`cargo run`): use the same data as the built program in dist\, so there
    // is one set of settings, not two. Cargo's build folders are recognised by their `.fingerprint` folder.
    let dist = PathBuf::from(meta.dist);
    if exe_dir.join(".fingerprint").is_dir() && dist.parent().is_some_and(|p| p.is_dir()) {
        return dist.join("data");
    }
    exe_dir.join("data")
}

/// Where the program keeps what is yours: `data\` next to the exe – `settings.json`, `log.txt`.
pub fn data_dir() -> PathBuf {
    DATA_DIR.get().cloned().unwrap_or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("data"))).unwrap_or_default())
}

/// The folder the program lies in (the one that holds `data\`).
pub fn program_dir() -> PathBuf {
    data_dir().parent().map(PathBuf::from).unwrap_or_default()
}

/// Appends a line with date and time to `data\log.txt`.
pub fn log(msg: &str) {
    use std::io::Write;
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("log.txt");
    // Keep it small: beyond 1 MB the old log is put aside (and replaces the one put aside before).
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = std::fs::rename(&file, dir.join("log.old.txt"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let t = unsafe {
            let mut t = std::mem::zeroed();
            windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
            t
        };
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {msg}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
}

/// A plain Windows message box – for the few things that must be said when no window can be shown.
pub fn message_box(title: &str, text: &str, error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
    if !ipc::sandbox().is_empty() {
        log(&format!("message box: {title}: {text}"));
        return; // tests show nothing
    }
    let (title, text) = (ipc::wide(title), ipc::wide(text));
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION }) };
}

// ---------- the background role ----------

/// The Toolbox's language and look, as the last `hello` told them. A program remembers them (in its
/// `settings.json`, under `_toolbox`), so a window it opens itself looks like the Toolbox (4.3).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Look {
    pub language: String,
    pub theme: String,
    pub zoom: f32,
}
impl Default for Look {
    fn default() -> Self {
        Self { language: i18n::system_language().to_string(), theme: "dark".into(), zoom: 1.0 }
    }
}

const LOOK_KEY: &str = "_toolbox";

struct Client {
    conn: Arc<Conn>,
    /// Lines for this connection. A thread of its own writes them, so a Toolbox that has stopped reading can
    /// never make the program wait.
    tx: SyncSender<String>,
    want: Vec<Want>,
}

impl Client {
    fn send(&self, state: &State) -> bool {
        match serde_json::to_string(state) {
            Ok(line) => self.tx.try_send(line).is_ok(),
            Err(_) => true,
        }
    }
}

struct Inner<P: Program> {
    program: P,
    settings: Settings<P::Settings>,
    look: Look,
    running: bool,
    clients: Vec<Client>,
    save_due: Option<Instant>,
    last_status: Option<Status>,
    last_page: Option<Page>,
    last_tray: Option<Tray>,
    /// The program shows a tray icon of its own, because its workers run and no Toolbox carries its menu (section 6).
    own_icon: bool,
    /// Counts what should wake a worker from `Runtime::sleep`: a `set`, an action, start and stop.
    nudges: u64,
    /// Wakes the thread of that icon.
    waker: Option<tray::Waker>,
}

/// The running program, as its worker sees it.
pub struct Runtime<P: Program> {
    inner: Arc<(Mutex<Inner<P>>, Condvar)>,
}
impl<P: Program> Clone for Runtime<P> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<P: Program> Runtime<P> {
    fn lock(&self) -> MutexGuard<'_, Inner<P>> {
        self.inner.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Something the program shows has changed: status, page and tray are built again and sent to whoever wants
    /// them – only what really differs from what was sent last.
    pub fn changed(&self) {
        push(&mut self.lock(), false, None);
    }

    /// Are the workers switched on?
    pub fn running(&self) -> bool {
        self.lock().running
    }

    /// The worker has ended its work by itself (a game whose window was closed): the workers are off, as after
    /// `stop`. With nobody looking at the page the program then ends (`bye`); a shown page keeps it, and Start runs
    /// the worker's work again.
    pub fn stop(&self) {
        let mut g = self.lock();
        if g.running {
            g.running = false;
            self.inner.1.notify_all();
            push(&mut g, false, None);
        }
    }

    /// Blocks until the workers are switched on – what a worker does while the program only serves its page.
    pub fn wait_running(&self) {
        let mut g = self.lock();
        while !g.running {
            g = self.inner.1.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// What a worker does between two rounds of its work: waits – without using the CPU – until the time is over,
    /// or until something happened it should look at right away: a setting was changed, a button was pressed, the
    /// workers were switched on or off. So a change shows at once, and Stop never waits for a long pause to end.
    pub fn sleep(&self, time: Duration) {
        let until = Instant::now() + time;
        let mut g = self.lock();
        let (nudges, running) = (g.nudges, g.running);
        while g.nudges == nudges && g.running == running {
            let Some(left) = until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { return };
            g = self.inner.1.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Access to the program and its settings.
    pub fn with<R>(&self, f: impl FnOnce(&mut P, &P::Settings) -> R) -> R {
        let mut g = self.lock();
        let g = &mut *g;
        f(&mut g.program, &g.settings.value)
    }

    /// The program changes a setting itself (not through the page): saved and shown like every other change.
    pub fn update(&self, f: impl FnOnce(&mut P::Settings)) {
        let mut g = self.lock();
        f(&mut g.settings.value);
        let g2 = &mut *g;
        g2.program.applied(&g2.settings.value);
        g.save_due = Some(Instant::now() + SAVE_DELAY);
        push(&mut g, false, None);
    }

    /// The Toolbox's language and look as last heard.
    pub fn look(&self) -> Look {
        self.lock().look.clone()
    }
}

/// Applied at once, written a moment later: a dragged slider writes the file once, not per step (4.3).
const SAVE_DELAY: Duration = Duration::from_millis(700);
/// With the workers off the program ends this long after the last page viewer left.
const IDLE_EXIT: Duration = Duration::from_secs(2);

/// Runs an action of the program. If it changed settings, they are applied and saved like after a `set`.
fn act<P: Program>(g: &mut Inner<P>, name: &str) -> Option<String> {
    let before = serde_json::to_value(&g.settings.value).ok();
    let Inner { program, settings, .. } = g;
    let toast = program.action(name, &mut settings.value);
    if serde_json::to_value(&settings.value).ok() != before {
        program.applied(&settings.value);
        g.save_due = Some(Instant::now() + SAVE_DELAY);
    }
    toast
}

/// Wakes a worker that waits in `Runtime::sleep`: something it should look at has changed.
fn nudge<P: Program>(g: &mut Inner<P>, rt: &Runtime<P>) {
    g.nudges += 1;
    rt.inner.1.notify_all();
}

fn build_page<P: Program>(g: &Inner<P>) -> Page {
    let v = serde_json::to_value(&g.settings.value).unwrap_or_default();
    let mut b = Builder::new(&v);
    g.program.page(&mut b, &g.settings.value);
    b.finish()
}

/// `page_again`: send the page even if it is unchanged (a refused `set`: the control that already moved snaps back).
/// `everything_for`: this connection just said `hello` and gets every state it wants, changed or not.
fn push<P: Program>(g: &mut Inner<P>, page_again: bool, everything_for: Option<&Arc<Conn>>) {
    let wanted = |g: &Inner<P>, w: Want| g.clients.iter().any(|c| c.want.contains(&w));
    // Only what somebody wants is built at all: a program nobody watches does no work for its page.
    let status = wanted(g, Want::Status).then(|| g.program.status(g.running));
    let page = wanted(g, Want::Page).then(|| build_page(g));
    let tray = wanted(g, Want::Tray).then(|| g.program.tray());
    let send_status = status.is_some() && g.last_status != status;
    let send_page = page.is_some() && (page_again || g.last_page != page);
    let send_tray = tray.is_some() && g.last_tray != tray;
    let (s, p, t) = (status.clone().map(State::Status), page.clone().map(State::Page), tray.clone().map(State::Tray));
    g.clients.retain(|c| {
        let mut ok = true;
        let everything = everything_for.is_some_and(|f| Arc::ptr_eq(&c.conn, f));
        for (send, want, state) in [(send_status, Want::Status, &s), (send_page, Want::Page, &p), (send_tray, Want::Tray, &t)] {
            if let (true, true, Some(state)) = (send || everything, c.want.contains(&want), state) {
                ok &= c.send(state);
            }
        }
        if !ok {
            // It has not read for so long that its queue is full: it is gone for us.
            c.conn.close();
        }
        ok
    });
    g.last_status = status;
    g.last_page = page;
    g.last_tray = tray;
    // The program's own icon shows the same line.
    if let (true, Some(waker)) = (g.own_icon, g.waker) {
        waker.wake();
    }
}

/// Without a Toolbox for this long, a running program shows its own tray icon.
const ALONE: Duration = Duration::from_millis(1500);
/// The fixed identity of that icon (Windows remembers "show this icon" per exe path and uid, section 6).
const OWN_ICON: u32 = 1;
/// Menu ids of the own icon; a program's own entries follow from `MENU_PROGRAM` on.
const MENU_SETTINGS: u32 = 1;
const MENU_QUIT: u32 = 2;
const MENU_PROGRAM: u32 = 10;

/// Opens the program's page in the Toolbox, if one runs. `false` if there is none.
pub fn open_in_toolbox(id: &str) -> bool {
    ipc::connect(ipc::TOOLBOX).is_ok_and(|c| c.send(&AgentCommand::Show { id: Some(id.to_string()) }).is_ok())
}

/// The thread of the program's own tray icon (section 6): the same line and menu the Toolbox would show, plus
/// Settings and Quit. It sleeps in the Windows message loop; the icon exists only while `own_icon` is set.
fn own_icon<P: Program>(rt: Runtime<P>) {
    let name = crate::manifest::Manifest::parse(P::META.manifest).map(|m| m.name).unwrap_or_default();
    let waker = tray::create({
        let rt = rt.clone();
        // The actions of the program's entries in the menu as it was last shown.
        let mut actions: Vec<String> = vec![];
        move |icon, event| {
            let name = name.get(&i18n::language()).to_string();
            match event {
                tray::Event::Wake => {
                    let g = rt.lock();
                    if g.own_icon {
                        let line = g.program.tray().line;
                        let tip = if line.is_empty() { name } else { format!("{name}: {line}") };
                        icon.set_icon(OWN_ICON, tray::Icon::Program, &tip);
                    } else {
                        icon.remove_icon(OWN_ICON);
                    }
                }
                tray::Event::LeftClick(_) => settings::<P>(&name),
                tray::Event::Menu => {
                    let tray = rt.lock().program.tray();
                    let mut items = vec![];
                    if !tray.line.is_empty() {
                        items.push(tray::Item::info(tray.line));
                        items.push(tray::Item::Separator);
                    }
                    actions = tray.menu.iter().map(|m| m.action.clone()).collect();
                    items.extend(tray.menu.iter().enumerate().map(|(i, m)| tray::Item::entry(MENU_PROGRAM + i as u32, m.label.clone())));
                    items.push(tray::Item::entry(MENU_SETTINGS, crate::t!("kit.tray.settings")));
                    items.push(tray::Item::Separator);
                    items.push(tray::Item::entry(MENU_QUIT, crate::t!("kit.tray.quit")));
                    return items;
                }
                tray::Event::Chosen(MENU_SETTINGS) => settings::<P>(&name),
                tray::Event::Chosen(MENU_QUIT) => finish(&mut rt.lock()),
                tray::Event::Chosen(n) => {
                    if let Some(action) = actions.get((n - MENU_PROGRAM) as usize) {
                        let mut g = rt.lock();
                        // What the action answers would be a toast in the Toolbox; here nobody could show it.
                        let _ = act(&mut g, action);
                        nudge(&mut g, &rt);
                        push(&mut g, false, None);
                    }
                }
            }
            vec![]
        }
    });
    rt.lock().waker = Some(waker);
    tray::run_loop();
}

/// "Settings" in the program's own tray menu: its page in the Toolbox, if one runs.
fn settings<P: Program>(name: &str) {
    if !open_in_toolbox(P::META.id) {
        let text = crate::t!("kit.tray.no_toolbox", file = data_dir().join("settings.json").display());
        std::thread::spawn({
            let name = name.to_string();
            move || message_box(&name, &text, false)
        });
    }
}

/// Tells the agent that the workers run; it then connects for status and tray (no polling, section 6). Without
/// a Toolbox nobody listens, and that is fine.
fn announce(id: &'static str) {
    std::thread::spawn(move || {
        if let Ok(c) = ipc::connect(ipc::TOOLBOX) {
            let _ = c.send(&AgentCommand::Started { id: id.to_string() });
        }
    });
}

fn finish<P: Program>(g: &mut Inner<P>) -> ! {
    // Whatever happens below, the process ends: a Toolbox that does not read must not keep it alive.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(2));
        std::process::exit(0);
    });
    g.program.shutdown();
    tray::forget(OWN_ICON);
    if g.save_due.take().is_some() {
        if let Err(e) = g.settings.save() {
            log(&format!("settings could not be saved: {e}"));
        }
    }
    for c in &g.clients {
        let _ = c.conn.send(&State::Bye);
    }
    std::process::exit(0);
}

/// Starts the program's background role and never returns. `worker` runs on a thread of its own and gets the
/// runtime; it does the program's actual work while `rt.running()`.
///
/// A double-click starts with the workers on. If the program runs already, the second start only asks the
/// first one to switch its workers on and show itself, and ends (the pipe is the single-instance lock, 4.3).
pub fn run<P: Program>(program: P, worker: impl FnOnce(Runtime<P>) + Send + 'static) {
    let id = P::META.id;
    init(&P::META);
    let workers = !std::env::args().any(|a| a == WORKERS_OFF);
    let server = match ipc::Server::bind(id) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if workers {
                if let Ok(c) = ipc::connect(id) {
                    let _ = c.send(&Command::Hello(ToolboxHello::default()));
                    let _ = c.send(&Command::Start);
                    let _ = c.send(&Command::Show);
                }
            }
            return;
        }
        Err(e) => {
            log(&format!("the pipe could not be created: {e}"));
            return;
        }
    };

    let settings = Settings::<P::Settings>::load(data_dir().join("settings.json"));
    let look: Look = settings.extra(LOOK_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    i18n::set_language(&look.language);
    let mut program = program;
    program.applied(&settings.value);
    let inner = Inner { program, settings, look, running: workers, clients: vec![], save_due: None, last_status: None, last_page: None, last_tray: None, own_icon: false, nudges: 0, waker: None };
    let rt = Runtime { inner: Arc::new((Mutex::new(inner), Condvar::new())) };
    // What Windows remembers by path follows a moved folder (section 2), and what an update put aside is removed.
    std::thread::spawn(move || {
        if let Ok(exe) = std::env::current_exe() {
            crate::autostart::repair(id, &exe);
        }
        crate::update::clean_up(&program_dir());
    });
    {
        let rt = rt.clone();
        std::thread::spawn(move || own_icon(rt));
    }
    if workers {
        announce(id);
    }
    {
        let rt = rt.clone();
        std::thread::spawn(move || worker(rt));
    }
    // Housekeeping: delayed saving, and ending when idle with the workers off.
    {
        let rt = rt.clone();
        std::thread::spawn(move || {
            let mut idle_since = Instant::now();
            let mut alone_since = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(200));
                let mut g = rt.lock();
                // Without a Toolbox the program must still be reachable (how else to quit it?): while its workers
                // run and nobody carries its tray menu, it shows an icon of its own. Not at once – a Toolbox that is
                // there connects within a moment of the start.
                if !g.running || g.clients.iter().any(|c| c.want.contains(&Want::Tray)) {
                    alone_since = Instant::now();
                }
                let own_icon = alone_since.elapsed() > ALONE;
                if own_icon != g.own_icon {
                    g.own_icon = own_icon;
                    if let Some(waker) = g.waker {
                        waker.wake();
                    }
                }
                if g.save_due.is_some_and(|t| Instant::now() >= t) {
                    g.save_due = None;
                    if let Err(e) = g.settings.save() {
                        log(&format!("settings could not be saved: {e}"));
                    }
                }
                // Only a shown settings page keeps a stopped program alive – the agent is connected to every
                // running program all the time and must not keep a stopped one around (found in M0).
                if g.running || g.clients.iter().any(|c| c.want.contains(&Want::Page)) {
                    idle_since = Instant::now();
                } else if idle_since.elapsed() > IDLE_EXIT {
                    finish(&mut g);
                }
            }
        });
    }
    loop {
        match server.accept() {
            Ok(conn) => {
                let rt = rt.clone();
                std::thread::spawn(move || serve(rt, conn));
            }
            Err(e) => {
                log(&format!("accepting a connection failed: {e}"));
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn serve<P: Program>(rt: Runtime<P>, conn: Arc<Conn>) {
    // The first line must be the caller's hello; anything else closes the connection (15.1).
    let Some(first) = conn.read_line() else { return };
    let Ok(Command::Hello(hello)) = serde_json::from_str::<Command>(&first) else { return };
    let (tx, rx) = sync_channel::<String>(256);
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
    let me = Client { conn: conn.clone(), tx: tx.clone(), want: vec![] };
    me.send(&State::Hello { protocol: PROTOCOL.min(hello.protocol), id: P::META.id.into(), version: P::META.version.into(), pid: std::process::id() });
    {
        let mut g = rt.lock();
        g.clients.push(me);
        greet(&mut g, &conn, hello);
    }
    let toast = |text: String| {
        if let Ok(line) = serde_json::to_string(&State::Toast { text }) {
            let _ = tx.try_send(line);
        }
    };
    while let Some(line) = conn.read_line() {
        let Ok(cmd) = serde_json::from_str::<Command>(&line) else { continue };
        let mut g = rt.lock();
        match cmd {
            Command::Set { key, value } => match page::apply_set(&g.settings.value, &key, value) {
                Ok(new) => {
                    g.settings.value = new;
                    let g2 = &mut *g;
                    g2.program.applied(&g2.settings.value);
                    g.save_due = Some(Instant::now() + SAVE_DELAY);
                    nudge(&mut g, &rt);
                    push(&mut g, false, None);
                }
                Err(why) => {
                    // Refused: the reason as a toast, and the unchanged page, so a control that already moved
                    // snaps back (15.1).
                    toast(why);
                    push(&mut g, true, None);
                }
            },
            Command::Action { name } => {
                if let Some(text) = act(&mut g, &name) {
                    toast(text);
                }
                nudge(&mut g, &rt);
                push(&mut g, false, None);
            }
            Command::Start => {
                if !g.running {
                    g.running = true;
                    rt.inner.1.notify_all();
                    announce(P::META.id);
                }
                push(&mut g, false, None);
            }
            Command::Stop => {
                g.running = false;
                rt.inner.1.notify_all();
                push(&mut g, false, None);
            }
            Command::Show => g.program.show(),
            Command::Quit => finish(&mut g),
            Command::Hello(h) => greet(&mut g, &conn, h),
            Command::Unknown => {}
        }
    }
    rt.lock().clients.retain(|c| !Arc::ptr_eq(&c.conn, &conn));
}

/// A `hello` – the first one of a connection or a later one: remember language and look, note what the
/// connection wants, and send it the states it asked for.
fn greet<P: Program>(g: &mut Inner<P>, conn: &Arc<Conn>, hello: ToolboxHello) {
    let look = Look { language: if i18n::is_language(&hello.language) { hello.language } else { g.look.language.clone() }, theme: hello.theme, zoom: hello.zoom };
    if look != g.look {
        i18n::set_language(&look.language);
        g.settings.set_extra(LOOK_KEY, serde_json::to_value(&look).unwrap_or_default());
        g.look = look;
        g.save_due = Some(Instant::now() + SAVE_DELAY);
    }
    if let Some(c) = g.clients.iter_mut().find(|c| Arc::ptr_eq(&c.conn, conn)) {
        c.want = hello.want;
    }
    // Everybody gets what changed (a new language changes every text); this connection gets everything it wants.
    push(g, false, Some(conn));
}

#[cfg(test)]
mod place_tests {
    use super::*;

    #[test]
    fn a_zip_preview_and_a_folder_without_write_access_are_caught() {
        let ok = std::env::temp_dir().join(format!("kit-place-{}", std::process::id()));
        assert_eq!(wrong_place(&ok, &ok.join("data")), None);
        assert!(ok.join("data").is_dir() && !ok.join("data").join(".write-test").exists(), "the probe leaves nothing behind");
        for zip in [r"C:\Users\a\AppData\Local\Temp\Temp1_Toolbox-2.0.0.zip", r"C:\Users\a\AppData\Local\Temp\7zO8A1B2C3D", r"C:\Users\a\AppData\Local\Temp\Rar$EXa1234.5678\Toolbox"] {
            assert_eq!(wrong_place(Path::new(zip), &ok.join("data")), Some("kit.start.in_zip"), "{zip}");
        }
        // Not a zip: a folder of one's own that only happens to have "temp" or "zip" in its name.
        assert_eq!(wrong_place(Path::new(r"E:\Games\Temperature\Zip Tools"), &ok.join("data")), None);
        // A data folder that cannot be made (a file is in the way) – as in Program Files without admin rights.
        std::fs::write(ok.join("blocked"), b"").unwrap();
        assert_eq!(wrong_place(&ok, &ok.join("blocked").join("data")), Some("kit.start.read_only"));
        std::fs::remove_dir_all(&ok).ok();
    }
}
