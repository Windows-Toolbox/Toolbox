//! What every exe of ours does first (`init`), and the background role of a kit program (`run`, DESIGN.md 5):
//! single instance through the pipe, `hello`, pushing states to every connection that wants them, applying `set`,
//! saving with a short delay, ending when the workers are off and nobody looks at the page any more.

use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ipc::{self, Conn};
use crate::page::{self, Builder, Page};
use crate::proto::{AgentCommand, Command, PROTOCOL, State, Status, ToolboxHello, Tray, Want};
use crate::settings::Settings;
use crate::{Meta, i18n};

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
    /// A button of the page or an entry of the tray menu. The returned text is shown as a toast.
    fn action(&mut self, _name: &str) -> Option<String> {
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
    std::panic::set_hook(Box::new(|info| {
        log(&format!("PANIC: {info}"));
        // A program whose thread died would look alive but do nothing. Ending makes the crash visible: the pipe
        // closes without `bye`, and the Toolbox shows "crashed" with a link to this log (section 8).
        std::process::exit(101);
    }));
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

    /// Blocks until the workers are switched on – what a worker does while the program only serves its page.
    pub fn wait_running(&self) {
        let mut g = self.lock();
        while !g.running {
            g = self.inner.1.wait(g).unwrap_or_else(|e| e.into_inner());
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
    let inner = Inner { program, settings, look, running: workers, clients: vec![], save_due: None, last_status: None, last_page: None, last_tray: None };
    let rt = Runtime { inner: Arc::new((Mutex::new(inner), Condvar::new())) };
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
            loop {
                std::thread::sleep(Duration::from_millis(200));
                let mut g = rt.lock();
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
                if let Some(text) = g.program.action(&name) {
                    toast(text);
                }
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
