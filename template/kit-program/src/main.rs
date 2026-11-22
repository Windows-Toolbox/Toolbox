//! Example – the template of a kit program (Toolbox\DESIGN.md 5). It does one small thing in the background: it
//! counts the minutes it runs and shows them in the Toolbox and in the tray. Replace that with what your program
//! does; the frame around it stays.
//!
//! What the kit does for you: the pipe to the Toolbox, loading and saving the settings, the page in the Toolbox,
//! the tray icon when no Toolbox runs, single instance, "Start with Windows", the log. What is left for you:
//!   Settings   one field per setting (missing and broken values fall back to their defaults)
//!   page       one line per setting – the Toolbox draws it
//!   work       the worker: what the program does while it runs
#![windows_subsystem = "windows"]

use std::time::{Duration, Instant};

use kit::app::{self, Program, Runtime};
use kit::page::Builder;
use kit::proto::{Health, MenuEntry, Status, Tray};
use kit::t;
use serde::{Deserialize, Serialize};

/// The settings – `data\settings.json` next to the exe. A new setting = one field here + one line in `page`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Show the minutes in the tray menu's line.
    pub in_tray: bool,
    /// How often the count is brought up to date, in seconds.
    pub every: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { in_tray: true, every: 30.0 }
    }
}

/// What the program knows while it runs – the page and the status are drawn from it.
struct Example {
    settings: Settings,
    /// Minutes since Start; `None` while the workers are off.
    minutes: Option<u64>,
}

impl Example {
    fn line(&self) -> String {
        match self.minutes {
            Some(m) => t!("status.minutes", minutes = m),
            None => String::new(),
        }
    }
}

impl Program for Example {
    type Settings = Settings;
    const META: kit::Meta = kit::meta!();

    fn page(&self, p: &mut Builder, _s: &Settings) {
        p.section(t!("page.status"), |p| match self.minutes {
            Some(_) => p.status(self.line(), Health::Ok),
            None => p.text(t!("page.stopped")),
        });
        p.section(t!("page.settings"), |p| {
            p.switch(t!("page.in_tray"), "in_tray");
            p.slider(t!("page.every"), "every", 5.0, 300.0, 5.0, "s");
        });
        p.section(t!("page.advanced"), |p| {
            p.buttons(|b| {
                b.button(t!("page.open_log"), "open_log").button(t!("page.reset"), "reset");
            });
        });
    }

    fn status(&self, running: bool) -> Status {
        if running { Status { running, state: Health::Ok, text: self.line() } } else { Status::default() }
    }

    fn tray(&self) -> Tray {
        let check = if self.settings.in_tray { "✓ " } else { "" };
        let line = if self.settings.in_tray { self.line() } else { String::new() };
        Tray { line, menu: vec![MenuEntry { label: format!("{check}{}", t!("menu.in_tray")), action: "toggle".into() }], live: vec![] }
    }

    fn applied(&mut self, s: &Settings) {
        self.settings = s.clone();
    }

    fn action(&mut self, name: &str, s: &mut Settings) -> Option<String> {
        match name {
            "open_log" => {
                kit::process::open(&app::data_dir().join("log.txt").to_string_lossy());
            }
            // A changed setting is applied and saved like a `set` from the page.
            "toggle" => s.in_tray = !s.in_tray,
            "reset" => {
                *s = Settings::default();
                return Some(t!("toast.reset"));
            }
            _ => {}
        }
        None
    }
}

/// The worker: runs while the program is started (Start, the tray, "Start with Windows").
fn work(rt: Runtime<Example>) {
    loop {
        // While the program only serves its page in the Toolbox, the worker sleeps here.
        rt.wait_running();
        let started = Instant::now();
        while rt.running() {
            let every = rt.with(|p, s| {
                p.minutes = Some(started.elapsed().as_secs() / 60);
                s.every
            });
            rt.changed();
            // Ends early when a setting changes, a button is pressed or the program is stopped.
            rt.sleep(Duration::from_secs_f64(every.clamp(1.0, 3600.0)));
        }
        rt.with(|p, _| p.minutes = None);
        rt.changed();
    }
}

fn main() {
    kit::texts!();
    app::run(Example { settings: Settings::default(), minutes: None }, work);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(minutes: Option<u64>) -> Example {
        kit::texts!();
        kit::i18n::set_language("en");
        Example { settings: Settings::default(), minutes }
    }

    /// Every key the page names is a field of the settings (Toolbox\DESIGN.md 13).
    #[test]
    fn page_keys_are_settings() {
        let e = example(Some(3));
        kit::page::check_keys::<Settings>(|p| e.page(p, &Settings::default())).unwrap();
    }

    /// Every text id used in the code exists, in every language.
    #[test]
    fn texts_are_complete() {
        kit::i18n::check_ids(env!("CARGO_MANIFEST_DIR")).unwrap();
    }

    #[test]
    fn status_and_tray() {
        let mut e = example(Some(3));
        assert_eq!(e.status(true), Status { running: true, state: Health::Ok, text: "Running for 3 min".into() });
        assert_eq!(e.status(false), Status::default());
        assert_eq!(e.tray().line, "Running for 3 min");
        let mut s = e.settings.clone();
        e.action("toggle", &mut s);
        e.applied(&s);
        assert_eq!(e.tray().line, "", "switched off in the tray menu");
    }
}
