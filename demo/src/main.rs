//! Kit Demo – the smallest kit program: one switch, one slider, one live status line, one button. It shows a
//! whole program on one screen, and the kit's tests talk to it (`tests\chain.rs`).
//!
//! What is *not* here is the point: no pipe code, no loading or saving, no single-instance check, no manifest –
//! all of that is the kit's job (DESIGN.md 5).
#![windows_subsystem = "windows"]

use kit::app::{self, Program};
use kit::page::Builder;
use kit::proto::{Health, MenuEntry, Status, Tray};
use kit::t;
use serde::{Deserialize, Serialize};

/// The settings. A new setting = one field here + one line in `page`.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    /// seconds between two readings
    rate: f64,
    /// a switch: off unless switched on (opt-in)
    warn_hot: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self { rate: 1.0, warn_hot: false }
    }
}

struct Demo {
    count: u64,
    running: bool,
    settings: Settings,
}

impl Demo {
    /// The "measured" value: wanders between 45 and 64.
    fn value(&self) -> u64 {
        45 + (self.count * 7) % 20
    }

    fn health(&self) -> Health {
        if self.settings.warn_hot && self.value() >= 60 { Health::Warn } else { Health::Ok }
    }
}

impl Program for Demo {
    type Settings = Settings;
    const META: kit::Meta = kit::meta!();

    fn page(&self, p: &mut Builder, _s: &Settings) {
        p.section(t!("page.readings"), |p| {
            if self.running {
                p.status(t!("page.reading", value = self.value(), count = self.count), self.health());
            } else {
                p.text(t!("page.stopped"));
            }
            p.slider(t!("page.rate"), "rate", 0.5, 5.0, 0.5, "s");
            p.switch(t!("page.warn_hot"), "warn_hot");
            p.buttons(|b| {
                b.button(t!("page.reset"), "reset");
            });
        });
    }

    fn status(&self, running: bool) -> Status {
        Status { running, state: self.health(), text: if running { t!("status.value", value = self.value()) } else { String::new() } }
    }

    fn tray(&self) -> Tray {
        Tray { line: t!("status.value", value = self.value()), menu: vec![MenuEntry { label: t!("page.reset"), action: "reset".into() }], live: vec![] }
    }

    fn applied(&mut self, s: &Settings) {
        self.settings = s.clone();
    }

    fn action(&mut self, name: &str, _s: &mut Settings) -> Option<String> {
        match name {
            "reset" => {
                self.count = 0;
                Some(t!("toast.reset"))
            }
            _ => None,
        }
    }
}

fn main() {
    kit::texts!();
    let demo = Demo { count: 0, running: false, settings: Settings::default() };
    app::run(demo, |rt| {
        loop {
            // While the program only serves its page, the worker sleeps here.
            rt.wait_running();
            rt.with(|d, _| d.running = true);
            rt.changed();
            while rt.running() {
                let rate = rt.with(|d, _| d.settings.rate);
                // Not `thread::sleep`: a changed rate, a pressed button or Stop end the pause at once.
                let started = std::time::Instant::now();
                rt.sleep(std::time::Duration::from_secs_f64(rate.clamp(0.1, 10.0)));
                if started.elapsed().as_secs_f64() < rate.clamp(0.1, 10.0) {
                    continue;
                }
                rt.with(|d, _| d.count += 1);
                rt.changed();
            }
            rt.with(|d, _| d.running = false);
            rt.changed();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Demo {
        Demo { count: 3, running: true, settings: Settings::default() }
    }

    /// Every key the page names is a field of the settings (DESIGN.md 13) – one line per program.
    #[test]
    fn page_keys_are_settings() {
        kit::page::check_keys::<Settings>(|p| demo().page(p, &Settings::default())).unwrap();
    }

    /// Every text id used in the code exists, in every language.
    #[test]
    fn texts_are_complete() {
        kit::i18n::check_ids(env!("CARGO_MANIFEST_DIR")).unwrap();
    }

    /// The page as data, without a window: which blocks, which values.
    #[test]
    fn the_page() {
        kit::texts!();
        kit::i18n::set_language("en");
        let settings = serde_json::json!({"rate": 2.5, "warn_hot": true});
        let mut b = Builder::new(&settings);
        demo().page(&mut b, &Settings::default());
        let page = serde_json::to_value(b.finish()).unwrap();
        let blocks = &page["blocks"][0]["blocks"];
        assert_eq!(page["blocks"][0]["title"], "Readings");
        assert_eq!(blocks[0]["type"], "status");
        assert_eq!((&blocks[1]["type"], &blocks[1]["value"]), (&"slider".into(), &2.5.into()));
        assert_eq!(blocks[2]["value"], true);
        assert_eq!(blocks[3]["buttons"][0]["action"], "reset");
    }
}
