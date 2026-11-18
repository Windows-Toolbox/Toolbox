//! The `list` block (DESIGN.md 15.2): many entries of one kind. Two switches decide how it looks – **Groups** (one
//! folding heading per group, or one flat list) and **Detail** (an entry's settings beside the list, or opening
//! under its row) – so there are four ways, all from the same entries. Filter, chips, sort and the switches are the
//! renderer's own business – nothing of it goes to the program, so typing costs no round trip. What the user chose
//! is kept in egui's memory under the list's `key`, so it survives leaving the page (for as long as the window runs).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Align, Color32, Id, Key, Layout, Modifiers, RichText, Sense, UiBuilder};

use super::{PageView, accent, health_color, muted};
use crate::page::{Item, List};
use crate::proto::{Command, Health};
use crate::t;

/// The sort every list has: by group (in the list's order of groups), then A–Z. Without groups: A–Z.
const ACTIVE_FIRST: &str = "";
const A_TO_Z: &str = "az";
/// How long a new entry, or one that changed its group (a game started), is marked.
const FLASH: Duration = Duration::from_millis(1500);
/// Below this width Detail puts the entry under the list instead of beside it.
const SIDE_BY_SIDE: f32 = 560.0;

#[derive(Clone, Default)]
struct State {
    /// The switch Groups: one folding heading per group.
    grouped: bool,
    /// The switch Detail: the chosen entry beside the list instead of opening under its row.
    side: bool,
    filter: String,
    /// The chip: a group's id, or none for "All".
    chip: Option<String>,
    sort: String,
    /// The entry open under its row …
    open: Option<String>,
    /// … and the one chosen with Detail.
    chosen: Option<String>,
    adding: bool,
    /// The order shown last – kept while the mouse is over the list (rows must not jump under it).
    shown: Vec<String>,
    /// What decided that order; a change by the user applies at once.
    asked: (String, Option<String>, String),
    /// Where the rows were last frame.
    rect: Option<egui::Rect>,
    /// Each entry's group as seen last frame – to notice a change.
    groups: HashMap<String, String>,
    flash: HashMap<String, Instant>,
    seen: bool,
}

/// Every word of the filter is in the title, the subtitle or a tag (upper and lower case alike).
pub(crate) fn matches(item: &Item, filter: &str) -> bool {
    let hay = format!("{} {} {}", item.title, item.subtitle, item.tags.join(" ")).to_lowercase();
    filter.split_whitespace().all(|word| hay.contains(&word.to_lowercase()))
}

/// The entries to show, in their order: those matching the filter and the chip, sorted.
pub(crate) fn arrange(list: &List, filter: &str, chip: Option<&str>, sort: &str) -> Vec<usize> {
    let rank = |item: &Item| list.groups.iter().position(|g| g.id == item.group).unwrap_or(usize::MAX);
    let title = |item: &Item| item.title.to_lowercase();
    let mut shown: Vec<usize> = (0..list.items.len()).filter(|&i| matches(&list.items[i], filter) && chip.is_none_or(|c| list.items[i].group == c)).collect();
    shown.sort_by(|&a, &b| {
        let (a, b) = (&list.items[a], &list.items[b]);
        let first = match sort {
            ACTIVE_FIRST => rank(a).cmp(&rank(b)),
            A_TO_Z => std::cmp::Ordering::Equal,
            id => {
                let place = |i: &Item| i.order.get(id).copied().unwrap_or(f64::INFINITY);
                place(a).total_cmp(&place(b))
            }
        };
        first.then_with(|| title(a).cmp(&title(b)))
    });
    shown
}

/// The order to show while the rows must not move: the old order for the entries still there, new ones after them.
pub(crate) fn steady(shown: &[String], wanted: &[String]) -> Vec<String> {
    let mut order: Vec<String> = shown.iter().filter(|id| wanted.contains(id)).cloned().collect();
    order.extend(wanted.iter().filter(|id| !shown.contains(id)).cloned());
    order
}

impl PageView {
    pub(super) fn list(&mut self, ui: &mut egui::Ui, list: &List, out: &mut Vec<Command>) {
        let id = Id::unique(("kit.list", &list.key));
        let mut st: State = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        // Without groups there is nothing to group by.
        let grouped = st.grouped && !list.groups.is_empty();
        if grouped || st.chip.as_ref().is_some_and(|c| !list.groups.iter().any(|g| &g.id == c)) {
            st.chip = None;
        }
        if st.sort != ACTIVE_FIRST && st.sort != A_TO_Z && !list.sorts.iter().any(|s| s.id == st.sort) {
            st.sort = ACTIVE_FIRST.into();
        }

        // Marks: new entries (not on the first look) and entries that changed their group.
        let now = Instant::now();
        for item in &list.items {
            let before = st.groups.insert(item.id.clone(), item.group.clone());
            if st.seen && before.as_ref() != Some(&item.group) {
                st.flash.insert(item.id.clone(), now);
            }
        }
        st.groups.retain(|k, _| list.items.iter().any(|i| &i.id == k));
        st.flash.retain(|_, at| at.elapsed() < FLASH);
        st.seen = true;

        self.list_bar(ui, list, &mut st, grouped, id);
        if st.adding
            && let Some(add) = &list.add
        {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.blocks(ui, &add.blocks, out);
            });
        }

        if list.items.is_empty() {
            ui.label(RichText::new(&list.empty).color(muted(ui)));
            ui.data_mut(|d| d.insert_temp(id, st));
            return;
        }

        // The order: what the filter, chip and sort give – unless only the program changed something while the
        // mouse is over the rows.
        let wanted: Vec<String> = arrange(list, &st.filter, st.chip.as_deref(), &st.sort).into_iter().map(|i| list.items[i].id.clone()).collect();
        let asked = (st.filter.clone(), st.chip.clone(), st.sort.clone());
        let pointer_over = st.rect.zip(ui.ctx().pointer_hover_pos()).is_some_and(|(r, p)| r.contains(p));
        st.shown = if asked == st.asked && pointer_over { steady(&st.shown, &wanted) } else { wanted };
        st.asked = asked;
        let rows: Vec<&Item> = st.shown.iter().filter_map(|id| list.items.iter().find(|i| &i.id == id)).collect();

        let top = ui.cursor().min;
        if rows.is_empty() {
            ui.label(RichText::new(t!("kit.list.none", filter = st.filter.trim())).color(muted(ui)));
        } else {
            let side = st.side;
            let entries = |this: &mut Self, ui: &mut egui::Ui, st: &mut State, out: &mut Vec<Command>| {
                if grouped { this.groups(ui, list, &rows, st, side, out) } else { this.rows(ui, &rows, st, side, out) }
            };
            if side { self.detail(ui, &rows, &mut st, out, entries) } else { entries(self, ui, &mut st, out) }
        }
        st.rect = Some(egui::Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y)));
        ui.add_space(8.0);
        if !st.flash.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(40));
        }
        ui.data_mut(|d| d.insert_temp(id, st));
    }

    /// The switches Groups and Detail, filter field, sort, the add button; the chips under them.
    fn list_bar(&mut self, ui: &mut egui::Ui, list: &List, st: &mut State, grouped: bool, id: Id) {
        let filter_id = id.with("filter");
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
            ui.memory_mut(|m| m.request_focus(filter_id));
        }
        ui.horizontal_wrapped(|ui| {
            if !list.groups.is_empty() && ui.selectable_label(grouped, t!("kit.list.groups")).on_hover_text(t!("kit.list.groups.tip")).clicked() {
                st.grouped = !grouped;
            }
            if ui.selectable_label(st.side, t!("kit.list.detail")).on_hover_text(t!("kit.list.detail.tip")).clicked() {
                st.side = !st.side;
            }
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(&mut st.filter).id(filter_id).hint_text(t!("kit.list.filter")).desired_width(220.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                st.filter.clear();
            }
            let mut sorts: Vec<(String, String)> = vec![];
            if !list.groups.is_empty() && !grouped {
                sorts.push((ACTIVE_FIRST.into(), t!("kit.list.sort.active")));
            }
            sorts.extend(list.sorts.iter().map(|s| (s.id.clone(), s.label.clone())));
            sorts.push((A_TO_Z.into(), t!("kit.list.sort.az")));
            // In groups "active first" is A–Z anyway; shown as such.
            let shown_sort = if st.sort == ACTIVE_FIRST && (grouped || list.groups.is_empty()) { A_TO_Z.to_string() } else { st.sort.clone() };
            let current = sorts.iter().find(|(k, _)| *k == shown_sort).map(|(_, l)| l.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt(id.with("sort")).selected_text(current).show_ui(ui, |ui| {
                for (k, l) in &sorts {
                    ui.selectable_value(&mut st.sort, k.clone(), l);
                }
            });
            if let Some(add) = &list.add
                && ui.selectable_label(st.adding, &add.label).clicked()
            {
                st.adding = !st.adding;
            }
        });
        if !grouped && !list.groups.is_empty() {
            let counted: Vec<&Item> = list.items.iter().filter(|i| matches(i, &st.filter)).collect();
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(st.chip.is_none(), format!("{} {}", t!("kit.list.all"), counted.len())).clicked() {
                    st.chip = None;
                }
                for g in &list.groups {
                    let n = counted.iter().filter(|i| i.group == g.id).count();
                    let chosen = st.chip.as_deref() == Some(g.id.as_str());
                    if (n > 0 || chosen) && ui.selectable_label(chosen, format!("{} {n}", g.label)).clicked() {
                        st.chip = if chosen { None } else { Some(g.id.clone()) };
                    }
                }
            });
        }
        ui.add_space(2.0);
    }

    /// The rows. Without Detail an entry opens under its row (only one at a time); with Detail a click chooses it
    /// and the rows are compact.
    fn rows(&mut self, ui: &mut egui::Ui, rows: &[&Item], st: &mut State, side: bool, out: &mut Vec<Command>) {
        for (n, item) in rows.iter().enumerate() {
            if side {
                let chosen = st.chosen.as_deref() == Some(item.id.as_str());
                if self.row(ui, item, st, chosen, true, out).clicked() {
                    st.chosen = Some(item.id.clone());
                }
                continue;
            }
            if n > 0 {
                ui.separator();
            }
            let open = st.open.as_deref() == Some(item.id.as_str());
            if self.row(ui, item, st, open, false, out).clicked() {
                st.open = if open { None } else { Some(item.id.clone()) };
            }
            if open {
                ui.indent(("kit.list.open", &item.id), |ui| {
                    ui.add_space(4.0);
                    self.blocks(ui, &item.blocks, out);
                    ui.add_space(4.0);
                });
            }
        }
    }

    /// `entries` (the rows, flat or in groups) on the left, the chosen entry beside them – or under them in a narrow
    /// window.
    fn detail(&mut self, ui: &mut egui::Ui, rows: &[&Item], st: &mut State, out: &mut Vec<Command>, entries: impl Fn(&mut Self, &mut egui::Ui, &mut State, &mut Vec<Command>)) {
        if !rows.iter().any(|i| Some(&i.id) == st.chosen.as_ref()) {
            st.chosen = rows.first().map(|i| i.id.clone());
        }
        let chosen = rows.iter().find(|i| Some(&i.id) == st.chosen.as_ref()).copied();
        let right = |this: &mut Self, ui: &mut egui::Ui, out: &mut Vec<Command>| match chosen {
            Some(item) => {
                ui.label(RichText::new(&item.title).strong().size(16.0));
                this.blocks(ui, &item.blocks, out);
            }
            None => {
                ui.label(RichText::new(t!("kit.list.choose")).color(muted(ui)));
            }
        };
        if ui.available_width() < SIDE_BY_SIDE {
            egui::ScrollArea::vertical().id_salt("kit.list.detail.rows").max_height(260.0).show(ui, |ui| entries(self, ui, st, out));
            ui.separator();
            right(self, ui, out);
            return;
        }
        let width = (ui.available_width() * 0.36).clamp(220.0, 320.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                egui::ScrollArea::vertical().id_salt("kit.list.detail.rows").max_height(520.0).show(ui, |ui| entries(self, ui, st, out));
            });
            ui.separator();
            // What is left of the width, so that long texts wrap instead of running out of the window.
            let rest = ui.available_width();
            ui.vertical(|ui| {
                ui.set_max_width(rest);
                right(self, ui, out);
            });
        });
    }

    /// One folding heading per group with its count – groups without an entry are left out. While filtering, every
    /// group with a match is open.
    fn groups(&mut self, ui: &mut egui::Ui, list: &List, rows: &[&Item], st: &mut State, side: bool, out: &mut Vec<Command>) {
        let filtering = !st.filter.trim().is_empty();
        for g in &list.groups {
            let mine: Vec<&Item> = rows.iter().filter(|i| i.group == g.id).copied().collect();
            if mine.is_empty() {
                continue;
            }
            let title = RichText::new(format!("{}  {}", g.label, mine.len())).color(accent(ui));
            let header = egui::CollapsingHeader::new(title).id_salt(("kit.list.group", &list.key, &g.id)).default_open(g.open);
            let header = if filtering { header.open(Some(true)) } else { header };
            header.show(ui, |ui| self.rows(ui, &mine, st, side, out));
        }
        // Entries of no known group come last, under no heading.
        let rest: Vec<&Item> = rows.iter().filter(|i| !list.groups.iter().any(|g| g.id == i.group)).copied().collect();
        if !rest.is_empty() {
            self.rows(ui, &rest, st, side, out);
        }
    }

    /// One row: dot, title and subtitle (compact: the state's text instead), tags, the row's button, the arrow.
    /// The whole row answers a click; the button inside it gets its own.
    fn row(&mut self, ui: &mut egui::Ui, item: &Item, st: &State, marked: bool, compact: bool, out: &mut Vec<Command>) -> egui::Response {
        let background = ui.painter().add(egui::Shape::Noop);
        let inner = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let color = if item.state == Health::Unknown { muted(ui) } else { health_color(ui, item.state) };
                let dot = if item.group.is_empty() || item.state != Health::Unknown { "●" } else { "○" };
                let r = ui.label(RichText::new(dot).color(color));
                if !item.text.is_empty() {
                    r.on_hover_text(&item.text);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !compact {
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                        let _ = rect;
                        egui::collapsing_header::paint_default_icon(ui, if marked { 1.0 } else { 0.0 }, &r);
                        if let Some(b) = &item.button
                            && ui.small_button(&b.label).clicked()
                        {
                            out.push(Command::Action { name: b.action.clone() });
                        }
                        for tag in item.tags.iter().rev() {
                            egui::Frame::new()
                                .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
                                .corner_radius(3)
                                .inner_margin(egui::Margin::symmetric(5, 0))
                                .show(ui, |ui| ui.label(RichText::new(tag).small().color(muted(ui))));
                        }
                    }
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        ui.add(egui::Label::new(RichText::new(&item.title).strong()).truncate().selectable(false));
                        let below = if compact { &item.text } else { &item.subtitle };
                        if !below.is_empty() {
                            ui.add(egui::Label::new(RichText::new(below).small().color(muted(ui))).truncate().selectable(false));
                        }
                    });
                });
            });
        });
        let r = inner.response;
        let flash = st.flash.get(&item.id).map(|at| 1.0 - at.elapsed().as_secs_f32() / FLASH.as_secs_f32()).unwrap_or(0.0).max(0.0);
        let v = ui.visuals();
        let fill = if marked {
            v.selection.bg_fill.gamma_multiply(0.35)
        } else if r.hovered() {
            v.widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        let fill = if flash > 0.0 { accent(ui).gamma_multiply(0.35 * flash) } else { fill };
        ui.painter().set(background, egui::Shape::rect_filled(r.rect.expand2(egui::vec2(4.0, 2.0)), 3.0, fill));
        if r.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Group;

    fn item(id: &str, title: &str, group: &str, last: Option<f64>) -> Item {
        let order = last.map(|l| [("last".to_string(), l)].into()).unwrap_or_default();
        Item { id: id.into(), title: title.into(), subtitle: format!("{id}.exe"), group: group.into(), tags: vec!["16:9 left".into()], order, ..Item::default() }
    }

    fn list() -> List {
        List {
            key: "t".into(),
            groups: vec![Group { id: "active".into(), label: "Active".into(), open: true }, Group { id: "other".into(), label: "Others".into(), open: false }],
            items: vec![item("c", "Celeste", "other", Some(2.0)), item("r", "rayman", "active", None), item("a", "Anno", "other", Some(1.0)), item("b", "Balatro", "active", Some(3.0))],
            ..List::default()
        }
    }

    #[test]
    fn filter_and_sort() {
        let l = list();
        let titles = |v: Vec<usize>| v.into_iter().map(|i| l.items[i].title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(arrange(&l, "", None, ACTIVE_FIRST)), ["Balatro", "rayman", "Anno", "Celeste"], "groups in order, then A–Z ignoring case");
        assert_eq!(titles(arrange(&l, "", None, A_TO_Z)), ["Anno", "Balatro", "Celeste", "rayman"]);
        assert_eq!(titles(arrange(&l, "", None, "last")), ["Anno", "Celeste", "Balatro", "rayman"], "smaller first, missing last");
        assert_eq!(titles(arrange(&l, "", Some("active"), A_TO_Z)), ["Balatro", "rayman"]);
        assert_eq!(titles(arrange(&l, "  CEL ", None, A_TO_Z)), ["Celeste"], "case and spaces do not matter");
        assert_eq!(titles(arrange(&l, "r.exe", None, A_TO_Z)), ["rayman"], "the subtitle counts");
        assert_eq!(titles(arrange(&l, "left nno", None, A_TO_Z)), ["Anno"], "every word, tags too");
        assert!(arrange(&l, "zzz", None, A_TO_Z).is_empty());
    }

    #[test]
    fn rows_stay_while_the_mouse_is_over_them() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(steady(&s(&["a", "b", "c"]), &s(&["c", "a", "d", "b"])), s(&["a", "b", "c", "d"]));
        assert_eq!(steady(&s(&["a", "b", "c"]), &s(&["c", "a"])), s(&["a", "c"]), "gone ones go at once");
    }

    /// All four ways (Groups and Detail on or off) draw, with and without entries, in a window that is wide and one
    /// that is narrow.
    #[test]
    fn every_way_is_drawn() {
        let ctx = egui::Context::default();
        let mut l = list();
        l.items[1].blocks = vec![crate::page::Block::Text { text: "inside".into() }];
        l.add = Some(crate::page::Add { label: "+".into(), blocks: vec![] });
        for (grouped, side) in [(false, false), (true, false), (false, true), (true, true)] {
            for width in [900.0, 400.0] {
                for items in [l.items.clone(), vec![]] {
                    let mut st = State { grouped, side, open: Some("r".into()), chosen: Some("r".into()), adding: true, ..State::default() };
                    let page = crate::page::Page { blocks: vec![crate::page::Block::List(List { items, ..l.clone() })] };
                    let mut pv = PageView::default();
                    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0))), ..Default::default() };
                    let mut commands = vec![];
                    ctx.run_ui(input, |ui| {
                        ui.data_mut(|d| d.insert_temp(Id::unique(("kit.list", "t")), std::mem::take(&mut st)));
                        commands = pv.show(ui, &page);
                    })
                    .textures_delta
                    .clear();
                    assert!(commands.is_empty(), "{grouped} {side} {width}: nothing was clicked");
                }
            }
        }
    }
}
