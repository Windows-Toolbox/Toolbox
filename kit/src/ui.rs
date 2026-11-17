//! The look every window of ours shares, and the page renderer (DESIGN.md 7, 4.4) – used by the Toolbox and by a
//! program's own settings window: one description, one renderer, two places.

use std::time::{Duration, Instant};

use egui::{Color32, FontId, RichText, TextStyle, ThemePreference};

use crate::page::{Block, Page};
use crate::proto::{Command, Health};
use crate::t;

/// The blue of our headings on a dark ground – a lighter tone of egui's own selection blue, which marks what is
/// chosen (tabs, the program on the left) …
pub const ACCENT: Color32 = Color32::from_rgb(0x6c, 0xb4, 0xf5);
/// … and on a light one.
pub const ACCENT_ON_LIGHT: Color32 = Color32::from_rgb(0x0b, 0x5f, 0xa5);
/// Good / warn / bad on a dark ground …
pub const GOOD: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x97);
pub const WARN: Color32 = Color32::from_rgb(0xf5, 0xc4, 0x51);
pub const BAD: Color32 = Color32::from_rgb(0xff, 0x64, 0x64);
/// … and on a light one, where the bright tones would be hard to read.
pub const GOOD_ON_LIGHT: Color32 = Color32::from_rgb(0x1f, 0x9e, 0x6a);
pub const WARN_ON_LIGHT: Color32 = Color32::from_rgb(0xb7, 0x79, 0x1f);
pub const BAD_ON_LIGHT: Color32 = Color32::from_rgb(0xd6, 0x45, 0x45);

/// The themes a user can choose; `theme` in `hello` is one of them.
pub const THEMES: &[&str] = &["dark", "light", "system"];

/// Applies the look: the Last Bastion editor's style – egui's own visuals (blue for what is chosen), text sizes
/// 12 / 15 / 15 / 19 / 14 – in the chosen theme and zoom. Cheap enough to call whenever one of them changes.
pub fn theme(ctx: &egui::Context, theme: &str, zoom: f32) {
    ctx.set_theme(match theme {
        "light" => ThemePreference::Light,
        "system" => ThemePreference::System,
        _ => ThemePreference::Dark,
    });
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(15.0)),
            (TextStyle::Button, FontId::proportional(15.0)),
            (TextStyle::Heading, FontId::proportional(19.0)),
            (TextStyle::Monospace, FontId::monospace(14.0)),
        ]
        .into();
    });
    let zoom = zoom.clamp(0.5, 3.0);
    if (ctx.zoom_factor() - zoom).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
}

pub fn good(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { GOOD } else { GOOD_ON_LIGHT }
}
pub fn warn(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { WARN } else { WARN_ON_LIGHT }
}
pub fn bad(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { BAD } else { BAD_ON_LIGHT }
}
/// For what explains rather than states.
pub fn muted(ui: &egui::Ui) -> Color32 {
    ui.visuals().weak_text_color()
}
/// The blue of headings.
pub fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode { ACCENT } else { ACCENT_ON_LIGHT }
}

/// A page's title: big and blue.
pub fn heading(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    let color = accent(ui);
    ui.label(RichText::new(text).heading().color(color))
}

/// A small grey line that explains what stands above or below it.
pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.label(RichText::new(text).small().weak())
}

/// A section as in the Last Bastion editor: a blue heading that folds shut with a click (egui remembers it by
/// `salt` – usually the title; anything that tells two sections of one page apart), its content indented under it.
pub fn section<R>(ui: &mut egui::Ui, title: &str, salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let heading = RichText::new(title).color(accent(ui));
    egui::CollapsingHeader::new(heading).id_salt(salt).default_open(true).show(ui, add).body_returned
}

/// A button of the bar at the top of a window – the editor's module bar: bigger than the others, blue while chosen.
pub fn bar_button(ui: &mut egui::Ui, text: impl Into<String>, chosen: bool) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(16.0)).selected(chosen).min_size(egui::vec2(0.0, 32.0)))
}

pub fn health_color(ui: &egui::Ui, h: Health) -> Color32 {
    match h {
        Health::Ok => good(ui),
        Health::Warn => warn(ui),
        Health::Bad => bad(ui),
        Health::Unknown => muted(ui),
    }
}

/// How every window of ours starts out: its title; no icon from eframe (it would put egui's own over ours a frame
/// later – `use_program_icon` sets the one compiled into the exe); and, for tests and screenshots, a place far
/// off the screen, without a taskbar entry and never activated, so nothing pops up (DESIGN.md 13).
pub fn viewport(title: &str) -> egui::ViewportBuilder {
    let viewport = egui::ViewportBuilder::default().with_title(title).with_icon(std::sync::Arc::new(egui::IconData::default()));
    if crate::ipc::sandbox().is_empty() && std::env::var_os("TOOLBOX_OFFSCREEN").is_none() {
        viewport
    } else {
        viewport.with_position([-20000.0, -20000.0]).with_taskbar(false).with_active(false)
    }
}

/// Gives this thread's windows the icon compiled into the exe. Call once the window exists (first frame); the
/// window must come from `viewport`.
pub fn use_program_icon() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> windows_sys::core::BOOL {
        unsafe {
            let big = crate::tray::program_icon(GetSystemMetrics(SM_CXICON));
            let small = crate::tray::program_icon(GetSystemMetrics(SM_CXSMICON));
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        }
        1
    }
    unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(each), 0) };
}

/// What every window of ours does when it cannot be created (DESIGN.md 7): on a PC without usable OpenGL the
/// window would simply not appear. This says why – in a plain Windows message box and in `log.txt`.
/// `error` is what `eframe::run_native` returned.
pub fn no_window(title: &str, error: &dyn std::fmt::Display) {
    crate::app::log(&format!("the window could not be created: {error}"));
    crate::app::message_box(title, &t!("kit.no_opengl", error = error), true);
}

/// What the renderer remembers between frames: the slider the user is holding (4.3, the slider rule).
#[derive(Default)]
pub struct PageView {
    held: Option<Held>,
    /// The text field the user is typing in: its key and what it holds now. The program hears of it when the
    /// field is left – until then pushed pages must not overwrite what is being typed.
    typing: Option<Typing>,
}

struct Typing {
    key: String,
    text: String,
    /// Set when the field was left and the text was sent.
    left: Option<Instant>,
}

struct Held {
    key: String,
    value: f64,
    /// Set when the mouse was released: the program's page may still show an older value for a moment.
    released: Option<Instant>,
}

impl PageView {
    /// Draws `page`; returns the commands the user's input caused (`set`, `action`) – the caller sends them to
    /// the program.
    pub fn show(&mut self, ui: &mut egui::Ui, page: &Page) -> Vec<Command> {
        let mut out = vec![];
        // A released slider waits for the program's page to show its value (or half a second) before pushed values
        // count again – otherwise the handle jumps back for one frame.
        if let Some(h) = &self.held {
            if let Some(t) = h.released {
                let echoed = find_slider(&page.blocks, &h.key).is_some_and(|v| (v - h.value).abs() < 1e-9);
                if echoed || t.elapsed() > Duration::from_millis(500) {
                    self.held = None;
                } else {
                    ui.ctx().request_repaint_after(Duration::from_millis(50));
                }
            }
        }
        self.blocks(ui, &page.blocks, &mut out);
        out
    }

    fn blocks(&mut self, ui: &mut egui::Ui, blocks: &[Block], out: &mut Vec<Command>) {
        for (i, b) in blocks.iter().enumerate() {
            self.block(ui, b, i, out);
        }
    }

    /// `i`: the block's place among its siblings – tells two sections with the same title apart.
    fn block(&mut self, ui: &mut egui::Ui, b: &Block, i: usize, out: &mut Vec<Command>) {
        match b {
            Block::Section { title, blocks } => {
                section(ui, title, (title, i), |ui| self.blocks(ui, blocks, out));
            }
            Block::Text { text } => {
                ui.label(RichText::new(text).color(muted(ui)));
            }
            Block::Status { text, state } => {
                ui.label(RichText::new(format!("● {text}")).color(health_color(ui, *state)));
            }
            Block::Switch { label, key, value } => {
                let mut v = *value;
                if ui.checkbox(&mut v, label).changed() {
                    out.push(Command::Set { key: key.clone(), value: v.into() });
                }
            }
            Block::Slider { label, key, value, min, max, step, unit } => {
                let held = self.held.as_ref().filter(|h| &h.key == key).map(|h| h.value);
                let mut v = held.unwrap_or(*value);
                ui.horizontal(|ui| {
                    ui.label(label);
                    let suffix = if unit.is_empty() { String::new() } else { format!(" {unit}") };
                    let r = ui.add(egui::Slider::new(&mut v, *min..=*max).step_by(*step).suffix(suffix));
                    if r.changed() {
                        out.push(Command::Set { key: key.clone(), value: v.into() });
                    }
                    if r.dragged() || r.changed() {
                        self.held = Some(Held { key: key.clone(), value: v, released: None });
                    }
                    if !r.dragged() {
                        if let Some(h) = self.held.as_mut().filter(|h| &h.key == key && h.released.is_none()) {
                            h.released = Some(Instant::now());
                        }
                    }
                });
            }
            Block::Choice { label, key, value, options } => {
                ui.horizontal_wrapped(|ui| {
                    if !label.is_empty() {
                        ui.label(label);
                    }
                    for o in options {
                        let r = ui.selectable_label(&o.value == value, &o.label);
                        // A marked option (15.2): a thin frame in its colour.
                        if let Some(state @ (Health::Warn | Health::Bad)) = o.state {
                            let radius = ui.visuals().widgets.inactive.corner_radius;
                            ui.painter().rect_stroke(r.rect, radius, egui::Stroke::new(1.0, health_color(ui, state)), egui::StrokeKind::Inside);
                        }
                        if r.clicked() && &o.value != value {
                            out.push(Command::Set { key: key.clone(), value: o.value.clone() });
                        }
                    }
                });
            }
            Block::Entry { label, key, value, secret } => {
                // Handed over a moment ago: until the program's page shows it (or half a second), the typed text
                // stays – otherwise the old value would flash up.
                if self.typing.as_ref().is_some_and(|t| &t.key == key && t.left.is_some_and(|at| &t.text == value || at.elapsed() > Duration::from_millis(500))) {
                    self.typing = None;
                }
                ui.horizontal(|ui| {
                    ui.label(label);
                    let mut text = self.typing.as_ref().filter(|t| &t.key == key).map(|t| t.text.clone()).unwrap_or_else(|| value.clone());
                    let r = ui.add(egui::TextEdit::singleline(&mut text).password(*secret).desired_width(280.0));
                    if r.has_focus() || r.changed() {
                        self.typing = Some(Typing { key: key.clone(), text: text.clone(), left: None });
                    }
                    // Leaving the field (a click elsewhere, Tab, Enter) hands the text over – once, and only if it
                    // differs from what the program has.
                    if r.lost_focus() {
                        if &text != value {
                            out.push(Command::Set { key: key.clone(), value: text.clone().into() });
                            self.typing = Some(Typing { key: key.clone(), text, left: Some(Instant::now()) });
                            ui.ctx().request_repaint_after(Duration::from_millis(550));
                        } else {
                            self.typing = None;
                        }
                    }
                });
            }
            Block::Buttons { buttons } => {
                ui.horizontal_wrapped(|ui| {
                    for btn in buttons {
                        if ui.button(&btn.label).clicked() {
                            out.push(Command::Action { name: btn.action.clone() });
                        }
                    }
                });
            }
            Block::Unknown => {
                ui.label(RichText::new(format!("⚠ {}", t!("kit.page.newer_toolbox"))).color(warn(ui)));
            }
        }
    }
}

fn find_slider(blocks: &[Block], key: &str) -> Option<f64> {
    blocks.iter().find_map(|b| match b {
        Block::Slider { key: k, value, .. } if k == key => Some(*value),
        Block::Section { blocks, .. } => find_slider(blocks, key),
        _ => None,
    })
}

/// A short message that disappears by itself – the result of an action, why something was refused.
#[derive(Default)]
pub struct Toast {
    current: Option<(String, bool, Instant)>,
}

impl Toast {
    const SHOWN: Duration = Duration::from_secs(4);

    pub fn show(&mut self, text: impl Into<String>, error: bool) {
        self.current = Some((text.into(), error, Instant::now()));
    }

    /// Draws the message while it is fresh; asks for the repaint that removes it.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        match &self.current {
            Some((text, error, at)) if at.elapsed() < Self::SHOWN => {
                let color = if *error { bad(ui) } else { good(ui) };
                ui.label(RichText::new(text).color(color));
                ui.ctx().request_repaint_after(Self::SHOWN - at.elapsed());
            }
            _ => self.current = None,
        }
    }
}

/// Like a browser's status bar: while the mouse is over `response`, `hovered_target` returns `target` – what the
/// button opens (a folder, an address).
pub fn link_target(response: &egui::Response, target: impl Into<String>) {
    if response.hovered() {
        let ctx = &response.ctx;
        let frame = ctx.cumulative_frame_nr();
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("kit.link_target"), (frame, target.into())));
    }
}

/// The target under the mouse, if any (as of the previous frame – the bar is usually drawn before the page).
pub fn hovered_target(ctx: &egui::Context) -> Option<String> {
    let frame = ctx.cumulative_frame_nr();
    let (at, target) = ctx.data(|d| d.get_temp::<(u64, String)>(egui::Id::unique("kit.link_target")))?;
    (frame <= at + 1).then_some(target)
}

#[cfg(test)]
mod tests {
    /// DESIGN.md 7, 13: the symbols our pages use must be there – in egui's fonts or, through eframe's
    /// `system_fonts`, in the Windows fonts.
    #[test]
    fn symbols_in_font() {
        let ctx = egui::Context::default();
        ctx.add_font_provider(std::sync::Arc::new(egui_system_font_provider::SystemFontProvider::new()));
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let font = egui::FontId::proportional(15.0);
        let missing: String = ctx.fonts_mut(|f| "●○⚠⚒⚙✓▸°·…".chars().filter(|&c| !f.has_glyph(&font, c)).collect());
        assert!(missing.is_empty(), "missing in egui's fonts: {missing}");
    }

    /// The renderer needs no window: a page is drawn into an egui context, and what the user did comes back as
    /// commands.
    #[test]
    fn a_page_is_drawn_without_a_window() {
        use crate::page::{Block, Page};
        let page = Page {
            blocks: vec![Block::Section {
                title: "S".into(),
                blocks: vec![
                    Block::Switch { label: "On".into(), key: "on".into(), value: false },
                    Block::Entry { label: "Name".into(), key: "name".into(), value: "x".into(), secret: true },
                    Block::Unknown,
                    Block::Text { text: "t".into() },
                ],
            }],
        };
        let ctx = egui::Context::default();
        super::theme(&ctx, "light", 1.25);
        let mut view = super::PageView::default();
        let mut commands = vec![];
        ctx.run_ui(Default::default(), |ui| commands = view.show(ui, &page)).textures_delta.clear();
        assert!(commands.is_empty(), "nothing was clicked");
    }
}
