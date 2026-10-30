//! The buttons on the top bar and beside the tabs: which, and where. Four
//! places take them (left and right of the address, left and right of the
//! tabs); Settings → Window → Panel items… shows them all, to drag there,
//! along, or away.

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::app::{App, Cmd, Focus, Menu, MenuItem, Overlay, SubMenu};
use crate::config::{Config, ViewMode};
use crate::i18n::tr;
use crate::icons::Icons;
use crate::input::{Action, Input};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    Sidebar,
    Back,
    Forward,
    Up,
    Search,
    Dual,
    Copy,
    Cut,
    Paste,
    View,
    Sort,
    NewFolder,
    Create,
    NewTab,
    Terminal,
    Hidden,
    SelectAll,
    Trash,
    Rename,
    Undo,
    Redo,
    Properties,
    Usage,
    Settings,
}

/// All of them, in the palette's order.
pub const ALL: [Tool; 24] = [
    Tool::Sidebar,
    Tool::Back,
    Tool::Forward,
    Tool::Up,
    Tool::Search,
    Tool::Dual,
    Tool::Copy,
    Tool::Cut,
    Tool::Paste,
    Tool::Trash,
    Tool::Rename,
    Tool::Undo,
    Tool::Redo,
    Tool::SelectAll,
    Tool::View,
    Tool::Sort,
    Tool::Hidden,
    Tool::NewFolder,
    Tool::Create,
    Tool::NewTab,
    Tool::Terminal,
    Tool::Properties,
    Tool::Usage,
    Tool::Settings,
];

impl Tool {
    /// Theme icon names, the first one there wins.
    pub fn icons(self, cfg: &Config) -> &'static [&'static str] {
        match self {
            Tool::Sidebar if cfg.sidebar_collapsed => &["sidebar-expand-left", "view-left-new", "sidebar-show", "view-sidebar"],
            Tool::Sidebar => &["sidebar-collapse-left", "view-left-close", "sidebar-show", "view-sidebar"],
            Tool::Back => &["go-previous", "arrow-left"],
            Tool::Forward => &["go-next", "arrow-right"],
            Tool::Up => &["go-up", "arrow-up"],
            Tool::Search => &["edit-find", "system-search"],
            Tool::Dual => &["view-split-left-right", "view-dual"],
            Tool::Copy => &["edit-copy"],
            Tool::Cut => &["edit-cut"],
            Tool::Paste => &["edit-paste"],
            Tool::View if cfg.view == ViewMode::Grid => &["view-list-details", "view-list-text", "view-list"],
            Tool::View => &["view-list-icons", "view-grid", "view-app-grid"],
            Tool::Sort => &["view-sort-ascending", "view-sort", "sort-ascending"],
            Tool::NewFolder => &["folder-new"],
            Tool::Create => &["document-new", "list-add"],
            Tool::NewTab => &["tab-new", "list-add"],
            Tool::Terminal => &["utilities-terminal", "terminal"],
            Tool::Hidden => &["view-hidden", "visibility", "view-visible"],
            Tool::SelectAll => &["edit-select-all"],
            Tool::Trash => &["user-trash", "edit-delete"],
            Tool::Rename => &["edit-rename", "document-edit"],
            Tool::Undo => &["edit-undo"],
            Tool::Redo => &["edit-redo"],
            Tool::Properties => &["document-properties", "dialog-information"],
            Tool::Usage => &["baobab", "drive-harddisk", "disk-quota"],
            Tool::Settings => &["configure", "preferences-system", "settings"],
        }
    }

    /// Drawn when the theme has none.
    pub fn glyph(self) -> &'static str {
        match self {
            Tool::Sidebar => "☰",
            Tool::Back => "◀",
            Tool::Forward => "▶",
            Tool::Up => "▲",
            Tool::Search => "⌕",
            Tool::Dual => "◫",
            Tool::Copy => "⧉",
            Tool::Cut => "✂",
            Tool::Paste => "⎘",
            Tool::View => "▦",
            Tool::Sort => "⇅",
            Tool::NewFolder => "+",
            Tool::Create => "✚",
            Tool::NewTab => "⊞",
            Tool::Terminal => ">_",
            Tool::Hidden => "◌",
            Tool::SelectAll => "☑",
            Tool::Trash => "⌫",
            Tool::Rename => "✎",
            Tool::Undo => "↶",
            Tool::Redo => "↷",
            Tool::Properties => "ⓘ",
            Tool::Usage => "◔",
            Tool::Settings => "⚙",
        }
    }

    pub fn label(self) -> &'static str {
        tr(match self {
            Tool::Sidebar => "Sidebar",
            Tool::Back => "Back",
            Tool::Forward => "Forward",
            Tool::Up => "Up",
            Tool::Search => "Search",
            Tool::Dual => "Dual pane",
            Tool::Copy => "Copy",
            Tool::Cut => "Cut",
            Tool::Paste => "Paste",
            Tool::View => "List or grid",
            Tool::Sort => "Sorting",
            Tool::NewFolder => "New folder",
            Tool::Create => "Create…",
            Tool::NewTab => "New tab",
            Tool::Terminal => "Open terminal here",
            Tool::Hidden => "Show hidden files",
            Tool::SelectAll => "Select all",
            Tool::Trash => "Move to trash",
            Tool::Rename => "Rename",
            Tool::Undo => "Undo",
            Tool::Redo => "Redo",
            Tool::Properties => "Properties",
            Tool::Usage => "Disk usage",
            Tool::Settings => "Settings",
        })
    }

    /// Lit up: what it switches is on.
    pub fn on(self, cfg: &Config) -> bool {
        match self {
            Tool::Dual => cfg.dual,
            Tool::Hidden => cfg.show_hidden,
            _ => false,
        }
    }
}

/// The four places for buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    TopLeft,
    TopRight,
    TabsLeft,
    TabsRight,
}

/// What each place holds, left to right. (The menu button stays at the
/// top bar's end: without it the mouse couldn't get back here.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Toolbars {
    pub top_left: Vec<Tool>,
    pub top_right: Vec<Tool>,
    pub tabs_left: Vec<Tool>,
    pub tabs_right: Vec<Tool>,
}

impl Default for Toolbars {
    fn default() -> Self {
        Toolbars {
            top_left: vec![Tool::Sidebar, Tool::Back, Tool::Forward, Tool::Up],
            top_right: vec![Tool::Search, Tool::Dual],
            tabs_left: Vec::new(),
            tabs_right: Vec::new(),
        }
    }
}

impl Toolbars {
    pub fn get(&self, z: Zone) -> &Vec<Tool> {
        match z {
            Zone::TopLeft => &self.top_left,
            Zone::TopRight => &self.top_right,
            Zone::TabsLeft => &self.tabs_left,
            Zone::TabsRight => &self.tabs_right,
        }
    }

    fn get_mut(&mut self, z: Zone) -> &mut Vec<Tool> {
        match z {
            Zone::TopLeft => &mut self.top_left,
            Zone::TopRight => &mut self.top_right,
            Zone::TabsLeft => &mut self.tabs_left,
            Zone::TabsRight => &mut self.tabs_right,
        }
    }

    pub fn contains(&self, t: Tool) -> bool {
        [Zone::TopLeft, Zone::TopRight, Zone::TabsLeft, Zone::TabsRight].iter().any(|z| self.get(*z).contains(&t))
    }

    /// Where it is now, if anywhere.
    fn find(&self, t: Tool) -> Option<(Zone, usize)> {
        [Zone::TopLeft, Zone::TopRight, Zone::TabsLeft, Zone::TabsRight].into_iter().find_map(|z| self.get(z).iter().position(|x| *x == t).map(|i| (z, i)))
    }

    /// Put `t` at `i` in `z` (from wherever it was), or take it away (None).
    pub fn place(&mut self, t: Tool, to: Option<(Zone, usize)>) {
        let from = self.find(t);
        if let Some((z, i)) = from {
            self.get_mut(z).remove(i);
        }
        if let Some((z, mut i)) = to {
            // Along its own row: its old place is gone.
            if let Some((fz, fi)) = from {
                if fz == z && fi < i {
                    i -= 1;
                }
            }
            let v = self.get_mut(z);
            v.insert(i.min(v.len()), t);
        }
    }
}

/// A button being dragged: from the palette, or from a place.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub tool: Tool,
    pub from_bar: bool,
}

/// Where buttons can go this frame: the place, and its buttons' rects.
#[derive(Clone, Debug)]
pub struct Target {
    pub zone: Zone,
    pub area: Rect,
    pub items: Vec<Rect>,
}

/// The place and position a drop at `p` lands on.
pub fn drop_at(targets: &[Target], p: Pos2) -> Option<(Zone, usize)> {
    let t = targets.iter().find(|t| t.area.expand2(vec2(8.0, 6.0)).contains(p))?;
    let i = t.items.iter().filter(|r| r.center().x < p.x).count();
    Some((t.zone, i))
}

/// Buttons of one place, from `x` on, `size` square, in `row` (its height).
/// While the panel items are edited it shows its room (empty: a dashed
/// square to drop on) and its buttons are dragged, not pressed.
/// Returns its target, a button pressed, and one picked up.
#[allow(clippy::too_many_arguments)]
pub fn draw_place(ui: &mut Ui, icons: &mut Icons, cfg: &Config, zone: Zone, x: f32, row: Rect, size: f32, editing: bool) -> (Target, Option<Tool>, Option<Tool>) {
    let tools = cfg.toolbars.get(zone).clone();
    let step = size + 4.0;
    let y = row.center().y - size / 2.0;
    let mut items = Vec::new();
    let (mut pressed, mut picked) = (None, None);
    for (i, t) in tools.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x + i as f32 * step, y), Vec2::splat(size));
        items.push(r);
        let resp = crate::view::tool_button(ui, r, icons, t.icons(cfg), t.glyph(), t.label(), t.on(cfg));
        if editing {
            let d = ui.interact(r, ui.id().with(("tool-drag", zone as u8, i)), Sense::click_and_drag());
            if d.drag_started_by(egui::PointerButton::Primary) {
                picked = Some(*t);
            }
        } else if resp.clicked() {
            pressed = Some(*t);
        }
    }
    let w = (tools.len() as f32 * step - 4.0).max(if editing { size } else { 0.0 });
    let area = Rect::from_min_size(pos2(x, y), vec2(w.max(0.0), size));
    if editing {
        let v = ui.visuals();
        let stroke = Stroke::new(1.0, v.selection.bg_fill.gamma_multiply(0.7));
        if tools.is_empty() {
            dashed_rect(ui, area, stroke);
        } else {
            ui.painter().rect_stroke(area.expand(2.0), 8.0, stroke, StrokeKind::Outside);
        }
    }
    (Target { zone, area, items }, pressed, picked)
}

/// The room a place takes in its row.
pub fn width(cfg: &Config, zone: Zone, size: f32, editing: bool) -> f32 {
    let n = cfg.toolbars.get(zone).len();
    if n == 0 {
        if editing { size } else { 0.0 }
    } else {
        n as f32 * (size + 4.0) - 4.0
    }
}

fn dashed_rect(ui: &Ui, r: Rect, stroke: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        ui.painter().extend(egui::Shape::dashed_line(&[w[0], w[1]], stroke, 4.0, 3.0));
    }
}

/// While one is dragged: it under the mouse, and where it'd land.
pub fn draw_drag(ctx: &egui::Context, icons: &mut Icons, cfg: &Config, d: &Drag, targets: &[Target]) {
    let Some(p) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tool-dragged")));
    let v = ctx.global_style().visuals.clone();
    if let Some((z, i)) = drop_at(targets, p) {
        if let Some(t) = targets.iter().find(|t| t.zone == z) {
            let x = match (t.items.get(i), t.items.last()) {
                (Some(r), _) => r.min.x - 2.0,
                (None, Some(r)) => r.max.x + 2.0,
                _ => t.area.center().x,
            };
            painter.line_segment([pos2(x, t.area.min.y - 2.0), pos2(x, t.area.max.y + 2.0)], Stroke::new(3.0, v.selection.bg_fill));
        }
    }
    // (In an area of its own: an image needs a ui to paint with.)
    egui::Area::new(egui::Id::new("tool-dragged-icon")).order(egui::Order::Tooltip).interactable(false).fixed_pos(p - Vec2::splat(18.0)).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
        ui.painter().rect(r, 8.0, v.window_fill.gamma_multiply(0.9), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
        let px = ((22.0 * ui.pixels_per_point()).ceil() as u16).max(22);
        match icons.ui_icon(d.tool.icons(cfg), px, v.text_color()) {
            Some(u) => egui::Image::new(u).paint_at(ui, Rect::from_center_size(r.center(), Vec2::splat(22.0))),
            None => {
                ui.painter().text(r.center(), Align2::CENTER_CENTER, d.tool.glyph(), FontId::proportional(18.0), v.text_color());
            }
        }
    });
}

pub enum PaletteEv {
    Pick(Tool),
    Reset,
    Done,
}

/// The panel items window: every button, to drag onto a place.
pub fn draw_palette(ui: &mut Ui, icons: &mut Icons, cfg: &Config) -> Option<PaletteEv> {
    let mut ev = None;
    ui.set_width(560.0);
    ui.label(egui::RichText::new(tr("Panel items")).strong().size(18.0));
    ui.label(egui::RichText::new(tr("Drag buttons onto the panels above; drag one off a panel to take it away.")).weak());
    ui.add_space(8.0);
    const COLS: usize = 6;
    let tile = vec2((560.0 - (COLS - 1) as f32 * 6.0) / COLS as f32, 76.0);
    for line in ALL.chunks(COLS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for t in line {
                let (r, resp) = ui.allocate_exact_size(tile, Sense::click_and_drag());
                let v = ui.visuals();
                let placed = cfg.toolbars.contains(*t);
                let fill = if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.widgets.inactive.weak_bg_fill };
                ui.painter().rect(r, 8.0, fill, if placed { Stroke::new(1.0, v.selection.bg_fill) } else { Stroke::NONE }, StrokeKind::Inside);
                let icon = Rect::from_center_size(pos2(r.center().x, r.min.y + 20.0), Vec2::splat(22.0));
                let px = ((22.0 * ui.pixels_per_point()).ceil() as u16).max(22);
                match icons.ui_icon(t.icons(cfg), px, v.text_color()) {
                    Some(u) => egui::Image::new(u).paint_at(ui, icon),
                    None => {
                        ui.painter().text(icon.center(), Align2::CENTER_CENTER, t.glyph(), FontId::proportional(18.0), v.text_color());
                    }
                }
                let g = ui.painter().layout(t.label().to_string(), FontId::proportional(11.0), v.text_color(), tile.x - 6.0);
                ui.painter().galley(pos2(r.center().x - g.size().x / 2.0, r.min.y + 36.0), g, v.text_color());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t.label()));
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    ev = Some(PaletteEv::Pick(*t));
                }
            }
        });
        ui.add_space(6.0);
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button(tr("Default")).clicked() {
            ev = Some(PaletteEv::Reset);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(egui::RichText::new(tr("Done")).strong())).clicked() {
                ev = Some(PaletteEv::Done);
            }
        });
    });
    ev
}

impl App {
    /// A button pressed.
    pub fn run_tool(&mut self, t: Tool) {
        let cfg = self.cfg.clone();
        let viewing = self.tabs[self.tab].viewer.is_some();
        match t {
            // A file shown: back and up close it (and its tab).
            Tool::Back | Tool::Up if viewing => {
                self.focus = Focus::Panes;
                self.request_close_viewer();
            }
            Tool::Back => {
                self.focus = Focus::Panes;
                self.handle(Input::Press(Action::HistoryBack));
            }
            Tool::Forward => {
                self.focus = Focus::Panes;
                self.handle(Input::Press(Action::HistoryForward));
            }
            Tool::Up => {
                self.focus = Focus::Panes;
                self.pane_mut().go_up(&cfg);
            }
            Tool::Sidebar => self.run(Cmd::ToggleSidebar),
            Tool::Search => self.run(Cmd::FindOrFilter),
            Tool::Dual => self.run(Cmd::ToggleDual),
            Tool::Copy => self.run(Cmd::Copy),
            Tool::Cut => self.run(Cmd::Cut),
            Tool::Paste => self.run(Cmd::Paste),
            Tool::View => self.run(Cmd::SetView(if cfg.view == ViewMode::Grid { ViewMode::List } else { ViewMode::Grid })),
            Tool::Sort => {
                let m = self.build_submenu(SubMenu::Sort);
                self.overlay = Some(Overlay::Menu(m));
            }
            Tool::NewFolder => self.run(Cmd::NewFolder),
            Tool::Create => {
                // A folder, or a document from a template.
                self.scan_templates();
                let writable = self.pane().loc.dest().is_some();
                let mut items = vec![MenuItem { enabled: writable, ..crate::app::item(tr("New folder"), Cmd::NewFolder) }];
                if writable {
                    items.extend(self.templates_menu(0));
                }
                self.overlay = Some(Overlay::Menu(Menu::new(tr("Create…"), items)));
            }
            Tool::NewTab => self.run(Cmd::NewTab),
            Tool::Terminal => self.run(Cmd::TermOpen(self.pane().loc.local_path().map(|p| p.to_path_buf()))),
            Tool::Hidden => self.run(Cmd::ToggleHidden),
            Tool::SelectAll => self.run(Cmd::SelectAll),
            Tool::Trash => self.run(Cmd::TrashAsk),
            Tool::Rename => self.run(Cmd::Rename),
            Tool::Undo => self.run(Cmd::Undo),
            Tool::Redo => self.run(Cmd::Redo),
            Tool::Properties => self.run(Cmd::Properties),
            Tool::Usage => self.run(Cmd::UsageOpen(None)),
            Tool::Settings => self.run(Cmd::Settings),
        }
    }

    /// A button let go: placed where it landed, or taken away.
    pub fn tool_drop(&mut self) {
        let Some(d) = self.tool_drag.take() else { return };
        let p = self.ctx.input(|i| i.pointer.latest_pos());
        let to = p.and_then(|p| drop_at(&self.tool_targets, p));
        // From the palette onto nothing: nothing happens.
        if to.is_none() && !d.from_bar {
            return;
        }
        self.cfg.toolbars.place(d.tool, to);
        self.cfg.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placing() {
        let mut b = Toolbars::default();
        // Along its own row.
        b.place(Tool::Back, Some((Zone::TopLeft, 4)));
        assert_eq!(b.top_left, [Tool::Sidebar, Tool::Forward, Tool::Up, Tool::Back]);
        // To another place, and in from the palette.
        b.place(Tool::Search, Some((Zone::TabsRight, 0)));
        b.place(Tool::Copy, Some((Zone::TabsRight, 0)));
        assert_eq!(b.tabs_right, [Tool::Copy, Tool::Search]);
        assert_eq!(b.top_right, [Tool::Dual]);
        // Away.
        b.place(Tool::Dual, None);
        assert!(b.top_right.is_empty() && !b.contains(Tool::Dual));
        let t = [Target { zone: Zone::TopLeft, area: Rect::from_min_size(pos2(0.0, 0.0), vec2(80.0, 38.0)), items: vec![Rect::from_min_size(pos2(0.0, 0.0), vec2(38.0, 38.0)), Rect::from_min_size(pos2(42.0, 0.0), vec2(38.0, 38.0))] }];
        assert_eq!(drop_at(&t, pos2(30.0, 10.0)), Some((Zone::TopLeft, 1)));
        assert_eq!(drop_at(&t, pos2(5.0, 10.0)), Some((Zone::TopLeft, 0)));
        assert_eq!(drop_at(&t, pos2(300.0, 10.0)), None);
    }
}
