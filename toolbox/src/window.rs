//! The window role (DESIGN.md 8): tabs per category, the programs of the tab on the left, the chosen program's
//! page on the right – inside the Toolbox's frame (status, start/stop, folder). A process of its own that exists
//! only while the window is open.
//!
//! What it shows comes from the agent (the library, pushed on every change) and, for a kit program, from the
//! program itself (its page). What the user does goes back the same two ways. No code of any program runs here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, ViewportCommand};
use kit::app::{Look, WORKERS_OFF};
use kit::autostart::State as Autostart;
use kit::ipc::{self, Conn};
use kit::manifest::Kind;
use kit::page::Page;
use kit::proto::{Command, PROTOCOL, State, ToolboxHello, Want};
use kit::ui::{self, PageView, Toast};
use kit::{app, i18n, process, t};

use crate::backup::Plan;
use crate::catalog::Offer;
use crate::library::{self, Added, Conflict, Entry, Library, Problem, Target, Updatable, Updating};
use crate::system::Trace;
use crate::talk::{Ask, Tell};

const TITLE: &str = "Windows Toolbox";
/// The tab of programs that name no category.
const NO_CATEGORY: &str = "General";

/// The connection to the kit program whose page is shown.
#[derive(Default)]
struct PageLink {
    /// The program's id; empty while no kit program's page is shown.
    id: String,
    /// Counts the connections made: a reader thread whose number is no longer the current one keeps quiet.
    serial: u64,
    conn: Option<Arc<Conn>>,
    page: Option<Page>,
    connecting: bool,
    /// When the connection ended or could not be made.
    lost: Option<Instant>,
    /// Attempts in a row that brought no page.
    tries: u32,
}

/// What the reader threads hand to the window.
#[derive(Default)]
struct Shared {
    library: Option<Arc<Library>>,
    /// The agent asked the window to come to the front – on this program's page if given.
    show: Option<Option<String>>,
    toasts: Vec<(String, bool)>,
    added: Option<Added>,
    /// The list of "Get programs" arrived.
    catalog: Option<Result<Vec<Offer>, String>>,
    /// What an import would do.
    plan: Option<Result<Plan, String>>,
    /// What Clean up / Uninstall all found.
    traces: Option<(bool, Vec<Trace>)>,
    /// Settings were imported: language, theme and zoom come from the library again.
    imported: bool,
    /// The agent is gone or said `bye`.
    closed: bool,
    page: PageLink,
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

fn page_hello(look: &Look) -> ToolboxHello {
    ToolboxHello { protocol: PROTOCOL, language: look.language.clone(), theme: look.theme.clone(), zoom: look.zoom, want: vec![Want::Page] }
}

/// Connects to the program for its page – started with its workers off if it does not run (DESIGN.md 4.4) – and
/// follows the page on a thread of its own.
fn open_page(shared: &Arc<Mutex<Shared>>, ctx: &egui::Context, entry: &Entry, look: &Look) {
    let Some(exe) = entry.exe() else { return };
    let (id, hello) = (entry.id().to_string(), Command::Hello(page_hello(look)));
    let serial = {
        let mut s = lock(shared);
        s.page.serial += 1;
        s.page.connecting = true;
        s.page.serial
    };
    let (shared, ctx) = (shared.clone(), ctx.clone());
    std::thread::spawn(move || {
        let there = ipc::exists(&id) || process::spawn(&exe, &[WORKERS_OFF]).is_ok();
        let conn = if there { ipc::connect_wait(&id, Duration::from_secs(5)).ok().filter(|c| c.send(&hello).is_ok()) } else { None };
        if let Some(conn) = conn {
            {
                let mut s = lock(&shared);
                if s.page.serial != serial {
                    return;
                }
                s.page.conn = Some(conn.clone());
                s.page.connecting = false;
            }
            while let Some(state) = conn.read::<State>() {
                let mut s = lock(&shared);
                if s.page.serial != serial {
                    return;
                }
                match state {
                    State::Page(page) => {
                        s.page.page = Some(page);
                        s.page.tries = 0;
                    }
                    State::Toast { text } => s.toasts.push((text, false)),
                    _ => continue,
                }
                drop(s);
                ctx.request_repaint();
            }
        }
        let mut s = lock(&shared);
        if s.page.serial == serial {
            s.page.conn = None;
            s.page.page = None;
            s.page.connecting = false;
            s.page.lost = Some(Instant::now());
            s.page.tries += 1;
        }
        drop(s);
        ctx.request_repaint();
    });
}

struct App {
    shared: Arc<Mutex<Shared>>,
    agent: Arc<Conn>,
    /// Language, theme and zoom. While the window is open it is the one place they are changed, so it never
    /// takes them back from the agent – only the first library sets them.
    look: Option<Look>,
    /// The zoom changed; the agent hears about it once it stands still (a file is written for it).
    zoom_changed: Option<Instant>,
    /// The zoom before rounding, while Ctrl + wheel turns it.
    zoom_exact: f32,
    /// The program whose page is shown.
    selected: Option<String>,
    /// A program to select as soon as the library knows it (`show <id>`, a start with an id).
    wanted: Option<String>,
    /// The category whose programs are listed.
    tab: String,
    /// The Toolbox's own settings are shown instead of a program's page.
    gear: bool,
    view: PageView,
    toast: Toast,
    /// "Add program…" found programs that are on the list already, from another folder.
    conflicts: Vec<Conflict>,
    /// The folder dialog is open.
    picking: Arc<AtomicBool>,
    /// "Get programs" is open in the gear …
    catalog_open: bool,
    /// … with this list – `None` while it loads.
    catalog: Option<Result<Vec<Offer>, String>>,
    /// An import waiting to be confirmed (or why the chosen file cannot be imported).
    plan: Option<Result<Plan, String>>,
    /// Clean up (`false`) or Uninstall all (`true`) waiting to be confirmed, with what it would remove.
    traces: Option<(bool, Vec<Trace>)>,
    first_frame: bool,
}

fn category(entry: &Entry) -> &str {
    let key = entry.manifest.category.key();
    if key.is_empty() { NO_CATEGORY } else { key }
}

/// The tab's label: our own text for the categories we know, otherwise what the programs say.
fn category_label(key: &str, library: &Library, language: &str) -> String {
    let id = format!("category.{key}");
    if i18n::has(&id) {
        return i18n::text(&id);
    }
    library.programs.iter().find(|e| category(e) == key).map(|e| e.manifest.category.get(language)).filter(|l| !l.is_empty()).unwrap_or(key).to_string()
}

fn muted_label(ui: &mut egui::Ui, text: impl Into<String>) {
    let color = ui::muted(ui);
    ui.label(RichText::new(text).color(color));
}

/// "Update" for one program (DESIGN.md 9) – while it runs, how far it is; for a copy built on this PC the hint to
/// run `build.ps1` instead; nothing for one that cannot be updated.
fn update_button(ui: &mut egui::Ui, target: &Target, updating: Option<&Updating>, busy: bool) -> bool {
    match (target.updatable(), updating) {
        (_, Some(step)) => {
            ui.spinner();
            muted_label(ui, match step {
                Updating::Checking => t!("update.checking"),
                Updating::Downloading(version) => t!("update.downloading", version = version),
                Updating::Installing(version) => t!("update.installing", version = version),
            });
            false
        }
        (Updatable::Yes, None) => ui.add_enabled(!busy, egui::Button::new(t!("update.button"))).on_hover_text(t!("update.button.hint")).clicked(),
        (Updatable::Built, None) => {
            muted_label(ui, t!("update.built"));
            false
        }
        (Updatable::No, None) => false,
    }
}

/// A button that opens a file, a folder or an address – the bottom bar shows which while the mouse is over it.
fn open_button(ui: &mut egui::Ui, label: String, target: &str) {
    let button = ui.button(label);
    ui::link_target(&button, target);
    if button.clicked() {
        process::open(target);
    }
}

impl App {
    fn ask(&self, ask: Ask) {
        let _ = self.agent.send(&ask);
    }

    fn select(&mut self, entry: &Entry) {
        self.selected = Some(entry.id().to_string());
        self.tab = category(entry).to_string();
        self.gear = false;
    }

    /// A new language, theme or zoom: applied here at once, told to the agent (which tells every running
    /// program) and to the program whose page is shown.
    fn look_changed(&mut self, ctx: &egui::Context) {
        let Some(look) = &self.look else { return };
        ui::theme(ctx, &look.theme, look.zoom);
        i18n::set_language(&look.language);
        self.ask(Ask::Look { language: look.language.clone(), theme: look.theme.clone(), zoom: look.zoom });
        if let Some(conn) = &lock(&self.shared).page.conn {
            let _ = conn.send(&Command::Hello(page_hello(look)));
        }
    }

    /// Ctrl + wheel and Ctrl + plus / minus / 0 change the zoom.
    fn zoom(&mut self, ctx: &egui::Context) {
        let Some(look) = &mut self.look else { return };
        let turned = ctx.input(|i| i.zoom_delta());
        if turned != 1.0 {
            self.zoom_exact = (self.zoom_exact * turned).clamp(0.5, 3.0);
            // In steps of 5 %, so the number is one a person would choose.
            let step = (self.zoom_exact * 20.0).round() / 20.0;
            if (step - ctx.zoom_factor()).abs() > 0.001 {
                ctx.set_zoom_factor(step);
            }
        }
        let now = ctx.zoom_factor();
        if (now - look.zoom).abs() > 0.001 {
            look.zoom = now;
            self.zoom_changed = Some(Instant::now());
            self.toast.show(t!("toast.zoom", percent = (now * 100.0).round()), false);
        }
        if turned == 1.0 {
            self.zoom_exact = now;
        }
        const STILL: Duration = Duration::from_millis(400);
        match self.zoom_changed {
            Some(at) if at.elapsed() >= STILL => {
                self.zoom_changed = None;
                self.look_changed(ctx);
            }
            Some(at) => ctx.request_repaint_after(STILL - at.elapsed()),
            None => {}
        }
    }

    /// Takes what the reader threads brought. `None` until the first library is there.
    fn take_news(&mut self, ctx: &egui::Context) -> Option<(Arc<Library>, Option<(String, Page)>, bool)> {
        let mut s = lock(&self.shared);
        if s.closed {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        for (text, error) in s.toasts.drain(..) {
            self.toast.show(text, error);
        }
        let show = s.show.take();
        let added = s.added.take();
        if let Some(catalog) = s.catalog.take() {
            self.catalog = Some(catalog);
        }
        if let Some(plan) = s.plan.take() {
            self.plan = Some(plan);
        }
        if let Some(traces) = s.traces.take() {
            self.traces = Some(traces);
        }
        if std::mem::take(&mut s.imported) {
            self.look = None;
        }
        let library = s.library.clone()?;
        // The page together with the program it belongs to: for one frame after another program was chosen, the
        // old one's page is still here and must not be drawn under the new one's name.
        let page = s.page.page.clone().map(|page| (s.page.id.clone(), page));
        let page_failed = s.page.conn.is_none() && !s.page.connecting && s.page.tries >= 3;
        drop(s);

        if self.look.is_none() {
            let settings = &library.settings;
            self.look = Some(Look { language: settings.language.clone(), theme: settings.theme.clone(), zoom: settings.zoom });
            self.zoom_exact = settings.zoom;
            ui::theme(ctx, &settings.theme, settings.zoom);
            i18n::set_language(&settings.language);
        }
        if let Some(id) = show {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.wanted = id.or(self.wanted.take());
        }
        if let Some(added) = added {
            self.added(added);
        }
        if let Some(entry) = self.wanted.as_ref().and_then(|id| library.programs.iter().find(|e| e.id() == id)) {
            self.select(entry);
            self.wanted = None;
        }
        // The selection must be a program of the shown tab; the tab must exist.
        let in_tab = |e: &&Entry| category(e) == self.tab;
        if !self.selected.as_ref().is_some_and(|id| library.programs.iter().filter(in_tab).any(|e| e.id() == id)) {
            match library.programs.iter().find(in_tab).or(library.programs.first()) {
                Some(entry) => {
                    self.selected = Some(entry.id().to_string());
                    self.tab = category(entry).to_string();
                }
                None => self.selected = None,
            }
        }
        Some((library, page, page_failed))
    }

    /// What "Add program…" did, as a message – and the questions it left.
    fn added(&mut self, added: Added) {
        let language = i18n::language();
        if !added.added.is_empty() {
            let names: Vec<&str> = added.added.iter().map(|n| n.get(&language)).collect();
            self.toast.show(t!("toast.added", names = names.join(", ")), false);
        } else if let Some(why) = added.broken.first() {
            self.toast.show(t!("toast.broken", why = why), true);
        } else if !added.conflicts.is_empty() {
            self.toast.show(t!("toast.conflict"), false);
        } else if added.known > 0 {
            self.toast.show(t!("toast.known"), false);
        } else {
            self.toast.show(t!("toast.nothing_found"), true);
        }
        if !added.conflicts.is_empty() {
            self.conflicts = added.conflicts;
            self.gear = true;
        }
    }

    /// "Add program…": the folder dialog, on a thread of its own so the window keeps drawing.
    fn add_program(&mut self, ctx: &egui::Context, library: &Library) {
        if self.picking.swap(true, Ordering::SeqCst) {
            return;
        }
        // Starts where programs usually lie: the folder that holds the Toolbox's folder.
        let start = library.folder.ancestors().nth(2).filter(|p| p.is_dir()).map(|p| p.to_path_buf());
        let (agent, picking, ctx, title) = (self.agent.clone(), self.picking.clone(), ctx.clone(), t!("gear.add.title"));
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            if let Some(folder) = dialog.pick_folder() {
                let _ = agent.send(&Ask::Add { folder });
            }
            picking.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    /// "Get programs…": opens the list in the gear and loads it (again).
    fn open_catalog(&mut self) {
        self.gear = true;
        self.catalog_open = true;
        self.catalog = None;
        self.ask(Ask::Catalog);
    }

    /// The list of "Get programs": each program with its description and a button – or what is in the way.
    fn catalog_list(&mut self, ui: &mut egui::Ui, library: &Library) {
        let language = i18n::language();
        let busy = !library.updating.is_empty();
        match &self.catalog {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    muted_label(ui, t!("get.loading"));
                });
            }
            Some(Err(why)) => {
                ui.label(RichText::new(format!("⚠ {}", t!("get.list_failed", why = why))).color(ui::warn(ui)));
                if ui.button(t!("page.retry")).clicked() {
                    self.open_catalog();
                }
            }
            Some(Ok(offers)) => {
                let mut get = None;
                for offer in offers {
                    ui.add_space(4.0);
                    egui::Sides::new().show(
                        ui,
                        |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(offer.name.get(&language)).strong());
                                if !offer.description.is_empty() {
                                    ui::hint(ui, offer.description.get(&language));
                                }
                            });
                        },
                        |ui| {
                            if let Some(step) = library.updating.get(&offer.id) {
                                muted_label(ui, match step {
                                    Updating::Checking => t!("update.checking"),
                                    Updating::Downloading(v) => t!("update.downloading", version = v),
                                    Updating::Installing(v) => t!("update.installing", version = v),
                                });
                                ui.spinner();
                            } else if library.programs.iter().any(|e| e.id() == offer.id) {
                                muted_label(ui, format!("✔ {}", t!("get.listed")));
                            } else if ui.add_enabled(!busy, egui::Button::new(t!("get.button"))).on_hover_text(t!("get.button.hint")).clicked() {
                                get = Some(offer.id.clone());
                            }
                        },
                    );
                }
                if let Some(id) = get {
                    self.ask(Ask::Get { id });
                }
            }
        }
    }

    /// Keeps the page connection in line with what is shown: only the visible page is connected to its program.
    fn keep_page(&mut self, ctx: &egui::Context, want: Option<&Entry>) {
        let Some(look) = self.look.clone() else { return };
        let want_id = want.map_or("", |e| e.id());
        let mut s = lock(&self.shared);
        if s.page.id != want_id {
            // Leaving a page: its program – if it only ran for the page – ends by itself.
            if let Some(conn) = s.page.conn.take() {
                conn.close();
            }
            s.page = PageLink { id: want_id.to_string(), serial: s.page.serial + 1, ..Default::default() };
            drop(s);
            self.view = PageView::default();
            if let Some(entry) = want {
                open_page(&self.shared, ctx, entry, &look);
            }
        } else if let (Some(entry), None, false, true) = (want, &s.page.conn, s.page.connecting, s.page.tries < 3) {
            // The program ended (a restart, a crash): look for it again after a moment.
            const PAUSE: Duration = Duration::from_secs(1);
            let since = s.page.lost.map_or(PAUSE, |t| t.elapsed());
            drop(s);
            if since >= PAUSE {
                open_page(&self.shared, ctx, entry, &look);
            } else {
                ctx.request_repaint_after(PAUSE - since);
            }
        }
    }

    // ---------- a program's page ----------

    fn program_page(&mut self, ui: &mut egui::Ui, entry: &Entry, library: &Library, page: Option<&Page>, page_failed: bool) {
        let language = i18n::language();
        let m = &entry.manifest;
        let running = entry.run.running == Some(true);

        egui::Sides::new().show(
            ui,
            |ui| ui::heading(ui, m.name.get(&language)),
            |ui| {
                if !m.version.is_empty() {
                    muted_label(ui, format!("v{}", m.version));
                }
            },
        );
        if !m.description.is_empty() {
            ui::hint(ui, m.description.get(&language));
        }
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            if entry.run.crashed {
                ui.label(RichText::new(format!("⚠ {}", t!("status.crashed"))).color(ui::bad(ui)));
                let log = entry.log();
                if log.exists() {
                    open_button(ui, t!("page.open_log"), &log.to_string_lossy());
                }
            } else if running {
                ui.label(RichText::new(format!("● {}", t!("status.running"))).color(ui::health_color(ui, entry.run.state)));
                if !entry.run.text.is_empty() {
                    ui.label(&entry.run.text);
                }
            } else if entry.run.running == Some(false) {
                muted_label(ui, format!("○ {}", t!("status.stopped")));
            }
        });

        // What Start would run must be there.
        let target = entry.start_target();
        let missing = target.as_ref().filter(|t| !t.exists());
        if let Some(file) = missing {
            ui.label(RichText::new(format!("⚠ {}", t!("page.missing", file = file.display()))).color(ui::warn(ui)));
        }

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if let (Some(target), None, false) = (&target, missing, running) {
                let label = m.button.as_ref().map(|b| b.get(&language).to_string()).filter(|b| !b.is_empty()).unwrap_or_else(|| t!("page.start"));
                let button = ui.button(label);
                ui::link_target(&button, m.launch.clone().unwrap_or_else(|| target.to_string_lossy().into_owned()));
                if button.clicked() {
                    self.ask(Ask::Start { id: entry.id().into() });
                }
            }
            if running && entry.kind != Kind::Exe {
                if ui.button(t!("page.stop")).clicked() {
                    self.ask(Ask::Stop { id: entry.id().into() });
                }
                if ui.button(t!("page.restart")).clicked() {
                    self.ask(Ask::Restart { id: entry.id().into() });
                }
            }
            open_button(ui, t!("page.open_folder"), &entry.folder.to_string_lossy());
            if let Some(readme) = entry.readme() {
                open_button(ui, t!("page.readme"), &readme.to_string_lossy());
            }
            if ui.button(t!("page.remove")).on_hover_text(t!("page.remove.hint")).clicked() {
                self.ask(Ask::Remove { stored: entry.stored.clone() });
            }
            if update_button(ui, &entry.target(), library.updating.get(entry.id()), !library.updating.is_empty()) {
                self.ask(Ask::Update { id: entry.id().into() });
            }
        });
        if entry.has_autostart() {
            ui.add_space(2.0);
            let mut on = entry.autostart_on();
            if ui.checkbox(&mut on, t!("page.autostart")).changed() {
                self.ask(Ask::Autostart { id: entry.id().into(), on });
            }
            let noted = entry.autostart.as_ref().filter(|a| a.state != Autostart::Off);
            match noted.and_then(|a| a.target.as_ref().map(|t| (a.state, t))) {
                Some((Autostart::DisabledByWindows, _)) if entry.autostart_is_mine() => {
                    ui.label(RichText::new(format!("⚠ {}", t!("page.autostart.disabled"))).color(ui::warn(ui)));
                }
                Some((_, other)) if !entry.autostart_is_mine() => {
                    ui::hint(ui, t!("page.autostart.elsewhere", file = other.display()));
                }
                _ => {}
            }
            let mut again = entry.restart_after_crash(&library.settings);
            if ui.checkbox(&mut again, t!("page.restart_crash")).on_hover_text(t!("page.restart_crash.hint")).changed() {
                self.ask(Ask::RestartAfterCrash { id: entry.id().into(), on: again });
            }
            if entry.run.gave_up {
                ui.label(RichText::new(format!("⚠ {}", t!("status.gave_up"))).color(ui::warn(ui)));
            }
        }
        ui.add_space(6.0);
        ui.separator();

        match entry.kind {
            Kind::Kit => match page {
                Some(page) => {
                    let commands = self.view.show(ui, page);
                    if let Some(conn) = &lock(&self.shared).page.conn {
                        for command in commands {
                            let _ = conn.send(&command);
                        }
                    }
                }
                None if page_failed => {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("⚠ {}", t!("page.no_page"))).color(ui::warn(ui)));
                    if ui.button(t!("page.retry")).clicked() {
                        let mut s = lock(&self.shared);
                        s.page.tries = 0;
                        s.page.lost = None;
                    }
                }
                None => {
                    ui.add_space(8.0);
                    ui.spinner();
                }
            },
            Kind::Script => {
                // The script's options, drawn like a program's page; a `set` from it is an option to save.
                for command in self.view.show(ui, &library::script_page(entry, &language)) {
                    if let Command::Set { key, value } = command {
                        self.ask(Ask::SetOption { id: entry.id().into(), name: key, value });
                    }
                }
            }
            Kind::Exe => {}
        }
    }

    // ---------- the Toolbox's own settings ----------

    fn gear_page(&mut self, ui: &mut egui::Ui, library: &Library) {
        let ctx = ui.ctx().clone();
        let language = i18n::language();
        let Some(mut look) = self.look.clone() else { return };

        ui::heading(ui, t!("gear.title"));
        ui.add_space(6.0);

        ui::section(ui, &t!("gear.look"), "look", |ui| {
            ui.horizontal(|ui| {
                ui.label(t!("gear.theme"));
                for (theme, label) in [("dark", t!("gear.theme.dark")), ("light", t!("gear.theme.light")), ("system", t!("gear.theme.system"))] {
                    if ui.selectable_label(look.theme == theme, label).clicked() {
                        look.theme = theme.to_string();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(t!("gear.language"));
                for (code, name) in i18n::LANGUAGES {
                    if ui.selectable_label(look.language == *code, *name).clicked() {
                        look.language = code.to_string();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(t!("gear.zoom", percent = (look.zoom * 100.0).round()));
                if ui.button("−").clicked() {
                    ctx.set_zoom_factor((look.zoom - 0.1).max(0.5));
                }
                if ui.button("+").clicked() {
                    ctx.set_zoom_factor((look.zoom + 0.1).min(3.0));
                }
                if ui.button(t!("gear.zoom.reset")).clicked() {
                    ctx.set_zoom_factor(1.0);
                }
            });
            ui::hint(ui, t!("gear.zoom.hint"));
        });
        if Some(&look) != self.look.as_ref() {
            self.look = Some(look);
            self.look_changed(&ctx);
        }

        ui::section(ui, &t!("gear.programs"), "programs", |ui| {
            ui::hint(ui, t!("gear.programs.text"));
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(t!("get.open")).on_hover_text(t!("get.open.hint")).clicked() {
                    self.open_catalog();
                }
                if ui.add_enabled(!self.picking.load(Ordering::SeqCst), egui::Button::new(t!("gear.add"))).clicked() {
                    self.add_program(&ctx, library);
                }
            });
            if self.catalog_open {
                ui.add_space(4.0);
                self.catalog_list(ui, library);
                ui.add_space(4.0);
                ui.separator();
            }
            let mut answered = None;
            for (i, conflict) in self.conflicts.iter().enumerate() {
                ui.add_space(6.0);
                let text = t!("gear.conflict", name = conflict.name.get(&language), listed = conflict.listed.display(), new = conflict.new.display());
                ui.label(RichText::new(format!("⚠ {text}")).color(ui::warn(ui)));
                ui.horizontal(|ui| {
                    if ui.button(t!("gear.conflict.keep")).clicked() {
                        answered = Some((i, false));
                    }
                    if ui.button(t!("gear.conflict.replace")).clicked() {
                        answered = Some((i, true));
                    }
                });
            }
            if let Some((i, replace)) = answered {
                let conflict = self.conflicts.remove(i);
                if replace {
                    self.ask(Ask::Replace { folder: conflict.new });
                }
            }
            let updatable = library.programs.iter().any(|e| e.target().updatable() == Updatable::Yes);
            if updatable {
                ui.add_space(2.0);
                let busy = !library.updating.is_empty();
                if ui.add_enabled(!busy, egui::Button::new(t!("update.all"))).on_hover_text(t!("update.all.hint")).clicked() {
                    self.ask(Ask::UpdateAll);
                }
            }
            ui.add_space(6.0);
            for entry in &library.programs {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").on_hover_text(t!("page.remove.hint")).clicked() {
                        self.ask(Ask::Remove { stored: entry.stored.clone() });
                    }
                    ui.label(entry.manifest.name.get(&language));
                    muted_label(ui, entry.folder.to_string_lossy());
                });
            }
            for lost in &library.missing {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").on_hover_text(t!("page.remove.hint")).clicked() {
                        self.ask(Ask::Remove { stored: lost.stored.clone() });
                    }
                    let why = match &lost.problem {
                        Problem::NotFound => t!("gear.problem.not_found"),
                        Problem::Broken(why) => t!("gear.problem.broken", why = why),
                        Problem::SameId(name) => t!("gear.problem.same_id", name = name),
                    };
                    ui.label(RichText::new(format!("⚠ {} – {why}", lost.folder.display())).color(ui::warn(ui)));
                });
            }
        });

        ui::section(ui, &t!("gear.tray"), "tray", |ui| {
            ui::hint(ui, t!("gear.tray.text"));
            ui.add_space(2.0);
            open_button(ui, t!("gear.tray.open"), "ms-settings:taskbar");
        });

        ui::section(ui, &t!("gear.toolbox"), "toolbox", |ui| {
            ui::hint(ui, t!("gear.version", version = crate::META.version));
            ui.add_space(2.0);
            self.own_switches(ui, library);
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let own = kit::manifest::Manifest::parse(crate::META.manifest).ok().map(|m| Target::of(&m, &library.folder));
                if let Some(own) = own {
                    if update_button(ui, &own, library.updating.get(library::OWN_ID), !library.updating.is_empty()) {
                        self.ask(Ask::Update { id: library::OWN_ID.into() });
                    }
                }
                open_button(ui, t!("page.open_folder"), &library.folder.to_string_lossy());
                let log = library.data.join("log.txt");
                if log.exists() {
                    open_button(ui, t!("page.open_log"), &log.to_string_lossy());
                }
                if ui.button(t!("toolbox.restart")).clicked() {
                    self.ask(Ask::RestartToolbox);
                }
                if ui.button(t!("gear.quit")).clicked() {
                    self.ask(Ask::Quit { all: false });
                }
                if ui.button(t!("gear.quit.all")).clicked() {
                    self.ask(Ask::Quit { all: true });
                }
            });
        });
        ui::section(ui, &t!("gear.backup"), "backup", |ui| {
            ui::hint(ui, t!("gear.backup.text"));
            ui.add_space(2.0);
            let free = !self.picking.load(Ordering::SeqCst);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(free, egui::Button::new(t!("gear.export"))).clicked() {
                    self.pick_file(&ctx, library, true);
                }
                if ui.add_enabled(free, egui::Button::new(t!("gear.import"))).clicked() {
                    self.pick_file(&ctx, library, false);
                }
            });
            self.import_plan(ui);
        });

        ui::section(ui, &t!("gear.remove"), "remove", |ui| {
            ui::hint(ui, t!("gear.remove.text"));
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(t!("gear.clean_up")).clicked() {
                    self.traces = None;
                    self.ask(Ask::Traces { everything: false });
                }
                if ui.button(t!("gear.uninstall")).clicked() {
                    self.traces = None;
                    self.ask(Ask::Traces { everything: true });
                }
            });
            self.trace_list(ui, library);
        });
        ui.add_space(10.0);
    }

    /// The Toolbox's own "Start with Windows" and "Show in the Start menu" – each also says when Windows switched it
    /// off or when it belongs to another copy.
    fn own_switches(&mut self, ui: &mut egui::Ui, library: &Library) {
        let own = &library.own;
        let mut on = own.autostart_on();
        if ui.checkbox(&mut on, t!("gear.autostart")).on_hover_text(t!("gear.autostart.hint")).changed() {
            self.ask(Ask::OwnAutostart { on });
        }
        match (own.autostart.state, own.autostart.target.as_ref()) {
            (Autostart::DisabledByWindows, Some(_)) if own.autostart_is_mine() => {
                ui.label(RichText::new(format!("⚠ {}", t!("page.autostart.disabled"))).color(ui::warn(ui)));
            }
            (Autostart::On | Autostart::DisabledByWindows, Some(other)) if !own.autostart_is_mine() => {
                ui::hint(ui, t!("page.autostart.elsewhere", file = other.display()));
            }
            _ => {}
        }
        let mut shown = own.shortcut_on();
        if ui.checkbox(&mut shown, t!("gear.start_menu")).changed() {
            self.ask(Ask::StartMenu { on: shown });
        }
        if let Some(other) = own.shortcut.as_ref().filter(|_| !own.shortcut_is_mine()) {
            ui::hint(ui, t!("gear.start_menu.elsewhere", file = other.display()));
        }
    }

    /// Export (`save`) or import: the file dialog, on a thread of its own so the window keeps drawing.
    fn pick_file(&mut self, ctx: &egui::Context, library: &Library, save: bool) {
        if self.picking.swap(true, Ordering::SeqCst) {
            return;
        }
        self.plan = None;
        let start = library.folder.ancestors().nth(2).filter(|p| p.is_dir()).map(|p| p.to_path_buf());
        let title = if save { t!("gear.export.title") } else { t!("gear.import.title") };
        let (agent, picking, ctx, name) = (self.agent.clone(), self.picking.clone(), ctx.clone(), t!("gear.export.file"));
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title).add_filter("Zip", &["zip"]);
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            let ask = if save { dialog.set_file_name(name).save_file().map(|file| Ask::Export { file }) } else { dialog.pick_file().map(|file| Ask::ImportCheck { file }) };
            if let Some(ask) = ask {
                let _ = agent.send(&ask);
            }
            picking.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    /// An import waiting to be confirmed: what it takes over, what it leaves out.
    fn import_plan(&mut self, ui: &mut egui::Ui) {
        let language = i18n::language();
        let names = |list: &[kit::manifest::Text]| list.iter().map(|n| n.get(&language).to_string()).collect::<Vec<_>>().join(", ");
        let mut answer = None;
        match &self.plan {
            None => return,
            Some(Err(why)) => {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("⚠ {}", t!("toast.import_failed", why = why))).color(ui::warn(ui)));
                if ui.button(t!("gear.cancel")).clicked() {
                    answer = Some(None);
                }
            }
            Some(Ok(plan)) => {
                ui.add_space(4.0);
                ui.label(t!("gear.import.plan", file = plan.file.display()));
                if plan.toolbox {
                    ui.label(format!("• {}", t!("gear.import.toolbox")));
                }
                if !plan.programs.is_empty() {
                    ui.label(format!("• {}", t!("gear.import.programs", names = names(&plan.programs))));
                }
                if !plan.skipped.is_empty() {
                    muted_label(ui, t!("gear.import.skipped", names = names(&plan.skipped)));
                }
                let something = plan.toolbox || !plan.programs.is_empty();
                ui::hint(ui, if something { t!("gear.import.replace") } else { t!("gear.import.nothing") });
                ui.horizontal(|ui| {
                    if something && ui.button(t!("gear.import.do")).clicked() {
                        answer = Some(Some(plan.file.clone()));
                    }
                    if ui.button(t!("gear.cancel")).clicked() {
                        answer = Some(None);
                    }
                });
            }
        }
        if let Some(file) = answer {
            self.plan = None;
            if let Some(file) = file {
                self.ask(Ask::Import { file });
            }
        }
    }

    /// What Clean up or Uninstall all found – listed before anything is removed (DESIGN.md 9).
    fn trace_list(&mut self, ui: &mut egui::Ui, library: &Library) {
        let Some((everything, traces)) = &self.traces else { return };
        let language = i18n::language();
        let target = |t: &Trace| match t.target() {
            Some(p) if p.exists() => p.display().to_string(),
            Some(p) => format!("{} {}", p.display(), t!("trace.gone")),
            None => t!("trace.gone"),
        };
        ui.add_space(4.0);
        if !everything && traces.is_empty() {
            ui.label(t!("gear.clean_up.none"));
        } else {
            ui.label(if *everything { t!("gear.uninstall.text") } else { t!("gear.clean_up.found") });
            for trace in traces {
                let line = match trace {
                    Trace::Autostart { id, .. } => {
                        let name = library.programs.iter().find(|e| e.id() == id).map(|e| e.manifest.name.get(&language).to_string());
                        let name = name.unwrap_or_else(|| if id == library::OWN_ID { "Windows Toolbox".into() } else { id.clone() });
                        t!("trace.autostart", name = name, target = target(trace))
                    }
                    Trace::Shortcut { path, .. } => t!("trace.shortcut", path = path.display(), target = target(trace)),
                };
                ui.label(format!("• {line}"));
            }
            if *everything {
                ui::hint(ui, t!("gear.uninstall.kept"));
            }
        }
        let (mut close, mut go) = (false, false);
        ui.horizontal(|ui| {
            let label = if *everything { t!("gear.uninstall.do") } else { t!("gear.clean_up.do") };
            if (*everything || !traces.is_empty()) && ui.button(label).clicked() {
                go = true;
            }
            if ui.button(t!("gear.cancel")).clicked() {
                close = true;
            }
        });
        if go {
            let ask = if *everything { Ask::Uninstall } else { Ask::CleanUp { traces: traces.clone() } };
            self.ask(ask);
        }
        if go || close {
            self.traces = None;
        }
    }

    /// Nothing on the list yet.
    fn welcome(&mut self, ui: &mut egui::Ui, library: &Library) {
        let ctx = ui.ctx().clone();
        ui.add_space(30.0);
        ui::heading(ui, t!("welcome.title"));
        ui.add_space(4.0);
        muted_label(ui, t!("welcome.text"));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(t!("get.open")).on_hover_text(t!("get.open.hint")).clicked() {
                self.open_catalog();
            }
            if ui.add_enabled(!self.picking.load(Ordering::SeqCst), egui::Button::new(t!("gear.add"))).clicked() {
                self.add_program(&ctx, library);
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

impl App {
    /// One frame: everything the window shows. Needs no window – a test draws it into memory.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.first_frame {
            self.first_frame = false;
            ui::use_program_icon();
        }
        let Some((library, page, page_failed)) = self.take_news(&ctx) else {
            egui::CentralPanel::default().show(ui, |ui| ui.spinner());
            return;
        };
        self.zoom(&ctx);
        let language = i18n::language();

        // The categories in the order their first program stands on the list.
        let mut tabs: Vec<&str> = vec![];
        for entry in &library.programs {
            if !tabs.contains(&category(entry)) {
                tabs.push(category(entry));
            }
        }

        // Like the editor's module bar: the categories on the left, the Toolbox's own settings on the right.
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(3.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            let (gear, tab) = (self.gear, self.tab.clone());
            let (tab_clicked, gear_clicked) = egui::Sides::new().show(
                ui,
                |ui| {
                    let mut clicked = None;
                    for key in &tabs {
                        if ui::bar_button(ui, category_label(key, &library, &language), !gear && tab == *key).clicked() {
                            clicked = Some(key.to_string());
                        }
                    }
                    clicked
                },
                |ui| ui::bar_button(ui, t!("tray.settings"), gear).on_hover_text(t!("gear.title")).clicked(),
            );
            if let Some(key) = tab_clicked {
                self.tab = key;
                self.gear = false;
            }
            if gear_clicked {
                self.gear = !self.gear;
            }
            ui.add_space(3.0);
        });

        // Like the editor's status line. Left, like a browser's status bar: what the button under the mouse opens –
        // otherwise the last message. Right: how many programs run, the size, the version.
        egui::Panel::bottom("bar").show(ui, |ui| {
            egui::Sides::new().shrink_left().truncate().show(
                ui,
                |ui| match ui::hovered_target(&ctx) {
                    Some(target) => muted_label(ui, target),
                    None => self.toast.ui(ui),
                },
                |ui| {
                    // From the right edge leftwards.
                    muted_label(ui, format!("v{}", crate::META.version));
                    ui.separator();
                    ui.label(t!("gear.zoom", percent = (ctx.zoom_factor() * 100.0).round()));
                    if !library.programs.is_empty() {
                        ui.separator();
                        let running = library.programs.iter().filter(|e| e.run.running == Some(true)).count();
                        ui.label(t!("bar.running", running = running, all = library.programs.len()));
                    }
                },
            );
        });

        if !library.programs.is_empty() {
            egui::Panel::left("programs").default_size(210.0).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.add_space(4.0);
                    let mut clicked = None;
                    for entry in library.programs.iter().filter(|e| category(e) == self.tab) {
                        let (dot, color) = match (entry.run.crashed, entry.run.running) {
                            (true, _) => ("⚠", ui::bad(ui)),
                            (_, Some(true)) => ("●", ui::health_color(ui, entry.run.state)),
                            _ => ("○", ui::muted(ui)),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(dot).color(color));
                            let chosen = !self.gear && self.selected.as_deref() == Some(entry.id());
                            if ui.selectable_label(chosen, entry.manifest.name.get(&language)).clicked() {
                                clicked = Some(entry);
                            }
                        });
                    }
                    if let Some(entry) = clicked {
                        self.select(entry);
                    }
                });
            });
        }

        let shown = if self.gear { None } else { self.selected.as_ref().and_then(|id| library.programs.iter().find(|e| e.id() == id)) };
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match shown {
                _ if self.gear => self.gear_page(ui, &library),
                Some(entry) => self.program_page(ui, entry, &library, page.as_ref().filter(|(id, _)| id == entry.id()).map(|(_, page)| page), page_failed),
                None => self.welcome(ui, &library),
            });
        });

        self.keep_page(&ctx, shown.filter(|e| e.kind == Kind::Kit));
    }
}

/// Runs the window until it is closed. `wanted`: the program whose page it opens on.
pub fn run(wanted: Option<String>) {
    // The window lives on what the agent tells it; the agent started it.
    let Ok(agent) = ipc::connect(ipc::TOOLBOX) else {
        return app::log("the window was started without an agent");
    };
    let viewport = ui::viewport(TITLE).with_inner_size([980.0, 640.0]).with_min_inner_size([640.0, 400.0]);

    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    let result = eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let shared = Arc::new(Mutex::new(Shared::default()));
            let (first, has_first) = std::sync::mpsc::channel::<()>();
            let _ = agent.send(&Ask::Window);
            {
                let (agent, shared, ctx) = (agent.clone(), shared.clone(), ctx.clone());
                std::thread::spawn(move || {
                    while let Some(tell) = agent.read::<Tell>() {
                        let mut s = lock(&shared);
                        match tell {
                            Tell::Library(library) => {
                                s.library = Some(Arc::new(library));
                                let _ = first.send(());
                            }
                            Tell::Toast { text, error } => s.toasts.push((text, error)),
                            Tell::Added(added) => s.added = Some(added),
                            Tell::Catalog { offers } => s.catalog = Some(offers),
                            Tell::Plan(plan) => s.plan = Some(plan),
                            Tell::Traces { everything, traces } => s.traces = Some((everything, traces)),
                            Tell::Imported => s.imported = true,
                            Tell::Show { id } => s.show = Some(id),
                            Tell::Bye => break,
                            Tell::Unknown => continue,
                        }
                        drop(s);
                        ctx.request_repaint();
                    }
                    lock(&shared).closed = true;
                    ctx.request_repaint();
                });
            }
            // The library arrives within milliseconds; waiting for it lets the very first frame have the right
            // theme, instead of a dark flash in a light window.
            let _ = has_first.recv_timeout(Duration::from_millis(500));
            ui::theme(&ctx, "dark", 1.0);
            let app = App {
                shared,
                agent,
                look: None,
                zoom_changed: None,
                zoom_exact: 1.0,
                selected: None,
                wanted,
                tab: String::new(),
                gear: false,
                view: PageView::default(),
                toast: Toast::default(),
                conflicts: vec![],
                picking: Arc::new(AtomicBool::new(false)),
                catalog_open: false,
                catalog: None,
                plan: None,
                traces: None,
                first_frame: true,
            };
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = result {
        ui::no_window(TITLE, &e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Missing, Run, Settings};
    use kit::manifest::Manifest;
    use kit::page::Block;

    fn entry(folder: &str, json: &str, run: Run) -> Entry {
        let manifest = Manifest::parse(json).unwrap();
        let kind = manifest.kind();
        let options = manifest.options.iter().map(|(name, option)| (name.clone(), option.default.clone())).collect();
        Entry { stored: folder.into(), folder: folder.into(), home: folder.into(), manifest, kind, options, run, autostart: None }
    }

    /// DESIGN.md 13: the drawing, without a window. Every kind of program, running, stopped and crashed, the gear
    /// with everything it can show, the empty library, dark and light – each is drawn into memory. What must hold:
    /// nothing panics, and the window chooses a sensible page by itself.
    #[test]
    fn every_page_is_drawn_without_a_window() {
        let id = format!("toolbox-window-test-{}", std::process::id());
        let server = ipc::Server::bind(&id).unwrap();
        let agent = ipc::connect(&id).unwrap();
        let _agent_side = server.accept().unwrap();
        i18n::load(include_str!("../assets/i18n.json"));

        let running = Run { running: Some(true), text: "52 °C".into(), ..Default::default() };
        let crashed = Run { running: Some(false), crashed: true, ..Default::default() };
        let programs = vec![
            entry("C:\\Nowhere\\Monitor", r#"{"id":"monitor","name":{"de":"Monitor","en":"Monitor"},"version":"2.0","category":{"de":"Allgemein","en":"General"},"description":"Temperatures","exe":"M.exe","protocol":1,"background":true}"#, running.clone()),
            entry("C:\\Nowhere\\Crashed", r#"{"id":"crashed","name":"Crashed","exe":"C.exe","protocol":1}"#, crashed),
            entry("C:\\Nowhere\\Rayman", r#"{"id":"rayman","name":"Rayman","category":"Games","run":"r.ps1","hint":"Set the game to windowed mode first.","button":{"de":"Spielen","en":"Play"},"options":{"Pos":{"values":["Left","Center"],"default":"Center"}}}"#, Run { running: Some(false), ..Default::default() }),
            entry("C:\\Nowhere\\Game", r#"{"id":"game","name":"Game","category":"Games","exe":"G.exe"}"#, running),
            // "Start with Windows": switched off in Task Manager, and an entry that starts another copy.
            Entry { autostart: Some(kit::autostart::Entry { state: Autostart::DisabledByWindows, target: Some("C:\\Nowhere\\Plus\\P.exe".into()) }), ..entry("C:\\Nowhere\\Plus", r#"{"id":"plus","name":"Plus","exe":"P.exe","protocol":1,"background":true}"#, Run::default()) },
            Entry { autostart: Some(kit::autostart::Entry { state: Autostart::On, target: Some(std::env::current_exe().unwrap()) }), ..entry("C:\\Nowhere\\Other", r#"{"id":"other","name":"Other","exe":"O.exe","protocol":1,"background":true}"#, Run::default()) },
        ];
        let missing = vec![
            Missing { stored: "..\\Gone".into(), folder: "C:\\Gone".into(), problem: Problem::NotFound },
            Missing { stored: "..\\Broken".into(), folder: "C:\\Broken".into(), problem: Problem::Broken("expected value at line 1".into()) },
            Missing { stored: "..\\Twice".into(), folder: "C:\\Twice".into(), problem: Problem::SameId("Monitor".into()) },
        ];
        let library = |programs: &[Entry], theme: &str| Arc::new(Library { settings: Settings { language: "en".into(), theme: theme.into(), zoom: 1.0, ..Default::default() }, programs: programs.to_vec(), missing: missing.clone(), folder: "C:\\Nowhere\\Toolbox\\dist".into(), data: "C:\\Nowhere\\Toolbox\\dist\\data".into(), updating: Default::default(), own: Default::default() });

        for theme in ["dark", "light"] {
            let shared = Arc::new(Mutex::new(Shared::default()));
            let mut app = App { shared: shared.clone(), agent: agent.clone(), look: None, zoom_changed: None, zoom_exact: 1.0, selected: None, wanted: None, tab: String::new(), gear: false, view: PageView::default(), toast: Toast::default(), conflicts: vec![], picking: Arc::new(AtomicBool::new(false)), catalog_open: false, catalog: None, plan: None, traces: None, first_frame: true };
            let ctx = egui::Context::default();
            let draw = |app: &mut App| ctx.run_ui(Default::default(), |ui| app.frame(ui)).textures_delta.clear();

            // Before the agent has said anything, and with nothing on the list.
            draw(&mut app);
            lock(&shared).library = Some(library(&[], theme));
            draw(&mut app);
            assert_eq!((app.selected.as_deref(), app.look.as_ref().map(|l| l.theme.as_str())), (None, Some(theme)));

            // With programs: the first one is chosen, its category is the tab.
            lock(&shared).library = Some(library(&programs, theme));
            draw(&mut app);
            assert_eq!((app.selected.as_deref(), app.tab.as_str()), (Some("monitor"), "General"));
            // A kit program whose page has arrived – with a block this Toolbox does not know.
            {
                let mut s = lock(&shared);
                s.page.page = Some(Page { blocks: vec![Block::Section { open: true, title: "Section".into(), blocks: vec![Block::Switch { label: "Switch".into(), key: "on".into(), value: true }, Block::Unknown] }] });
                s.toasts.push(("Saved".into(), false));
            }
            draw(&mut app);
            // Each program in turn; `show <id>` from the agent chooses the page and its tab.
            for e in &programs {
                lock(&shared).show = Some(Some(e.id().to_string()));
                draw(&mut app);
                assert_eq!((app.selected.as_deref(), app.tab.as_str()), (Some(e.id()), category(e)));
                draw(&mut app);
            }
            // An update under way – on a page and for the Toolbox in the gear – and one program that can be updated.
            {
                let mut updating = library(&programs, theme);
                let u = Arc::make_mut(&mut updating);
                u.updating.insert("monitor".into(), Updating::Downloading("2.1".into()));
                u.updating.insert(library::OWN_ID.into(), Updating::Checking);
                u.programs[1].manifest.repository = Some("Windows-Toolbox/Crashed".into());
                u.programs[1].manifest.update_key = Some(kit::update::KEY.into());
                lock(&shared).library = Some(updating);
                for id in ["monitor", "crashed"] {
                    lock(&shared).show = Some(Some(id.into()));
                    draw(&mut app);
                }
                app.gear = true;
                draw(&mut app);
                // "Get programs": loading, failed, and loaded – one program on the list already, one being got.
                app.catalog_open = true;
                draw(&mut app);
                lock(&shared).catalog = Some(Err("GitHub cannot be reached".into()));
                draw(&mut app);
                let offer = |id: &str| Offer { id: id.into(), name: id.into(), description: "What it does".into(), repository: "o/r".into(), update_key: kit::update::KEY.into() };
                lock(&shared).catalog = Some(Ok(vec![offer("monitor"), offer("new_one"), offer("other_one")]));
                Arc::make_mut(lock(&shared).library.as_mut().unwrap()).updating.insert("new_one".into(), Updating::Installing("1.0".into()));
                draw(&mut app);
                assert!(matches!(&app.catalog, Some(Ok(list)) if list.len() == 3));
                app.gear = false;
                app.catalog_open = false;
            }
            // The gear with everything M6 shows: the Toolbox's own switches (on, switched off by Windows, another
            // copy's), "Restart after a crash" that gave up, an import to confirm, Clean up and Uninstall all.
            {
                let mut own = library(&programs, theme);
                let u = Arc::make_mut(&mut own);
                u.own = crate::system::Own { exe: "C:\\Nowhere\\Toolbox.exe".into(), autostart: kit::autostart::Entry { state: Autostart::DisabledByWindows, target: Some("C:\\Nowhere\\Toolbox.exe".into()) }, shortcut: Some(std::env::current_exe().unwrap()) };
                u.settings.restart_after_crash = vec!["monitor".into()];
                u.programs[0].run.gave_up = true;
                lock(&shared).library = Some(own);
                lock(&shared).show = Some(Some("monitor".into()));
                draw(&mut app);
                app.gear = true;
                let plan = Plan { file: "C:\\Backup.zip".into(), toolbox: true, programs: vec!["Monitor".into()], skipped: vec!["Gone".into()] };
                lock(&shared).plan = Some(Ok(plan));
                lock(&shared).traces = Some((false, vec![Trace::Autostart { id: "gone".into(), target: Some("C:\\Gone\\G.exe".into()) }, Trace::Shortcut { path: "C:\\S.lnk".into(), target: None }]));
                draw(&mut app);
                assert!(app.plan.is_some() && app.traces.is_some());
                lock(&shared).plan = Some(Err("this zip holds no exported settings".into()));
                lock(&shared).traces = Some((true, vec![]));
                draw(&mut app);
                lock(&shared).imported = true;
                draw(&mut app);
                assert_eq!(app.look.as_ref().map(|l| l.theme.as_str()), Some(theme), "after an import the look comes from the library");
                app.gear = false;
            }
            // A tab that lost its last program: the window moves on by itself.
            lock(&shared).library = Some(library(&programs[..4], theme));
            draw(&mut app);
            assert_eq!(app.tab, "General");

            // The gear, with a question from "Add program…" and the folders that show no program.
            lock(&shared).added = Some(Added { conflicts: vec![Conflict { name: "Game".into(), listed: "C:\\Nowhere\\Game".into(), new: "D:\\Copy\\Game".into() }], ..Default::default() });
            draw(&mut app);
            assert!(app.gear && app.conflicts.len() == 1, "a conflict opens the gear, where it is asked about");
            draw(&mut app);

            // The agent ends: the window is asked to close.
            lock(&shared).closed = true;
            draw(&mut app);
        }
    }
}