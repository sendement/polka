//! Offscreen renders of the main screens (wgpu). Not run by default:
//!   POLKA_SHOT_DIR=/tmp/shots POLKA_TEST_DIR=/some/dir cargo test -- --ignored
//! POLKA_SHOT_DIR, when set, receives PNGs to eyeball.

use std::path::PathBuf;

use egui_kittest::Harness;

use crate::app::App;
use crate::input::{Action, Input};

fn harness(dir: PathBuf) -> Harness<'static, App> {
    // Never touch the real config or gamepad from tests.
    let cfg = std::env::temp_dir().join(format!("polka-test-cfg-{}", std::process::id()));
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", &cfg);
        std::env::set_var("POLKA_NO_GAMEPAD", "1");
        std::env::set_var("XDG_CACHE_HOME", cfg.join("cache"));
    }
    Harness::builder()
        .with_size([1100.0, 680.0])
        .wgpu()
        .build_eframe(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            crate::fonts::install(&cc.egui_ctx);
            App::new(cc.egui_ctx.clone(), Some(dir))
        })
}

fn settle(h: &mut Harness<'static, App>) {
    // Image loaders and archive listings finish on other threads.
    for _ in 0..12 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

fn press(h: &mut Harness<'static, App>, actions: &[Action]) {
    for a in actions {
        h.state_mut().handle(Input::Press(*a));
        h.state_mut().handle(Input::Release(*a));
        h.step();
    }
    settle(h);
}

fn shot(h: &mut Harness<'static, App>, name: &str) {
    let img = h.render().expect("render");
    if let Some(dir) = std::env::var_os("POLKA_SHOT_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
#[ignore]
fn screens() {
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    settle(&mut h);
    press(&mut h, &[Action::Down, Action::Down]);
    shot(&mut h, "01-list");

    press(&mut h, &[Action::Actions]);
    shot(&mut h, "02-actions");
    press(&mut h, &[Action::Back]);

    // Select two entries by holding Select while moving.
    h.state_mut().handle(Input::Press(Action::Select));
    h.state_mut().handle(Input::Press(Action::Down));
    h.state_mut().handle(Input::Release(Action::Select));
    settle(&mut h);
    assert_eq!(h.state().pane().selected.len(), 2);
    press(&mut h, &[Action::Back]);
    assert!(h.state().pane().selected.is_empty());

    h.state_mut().run(crate::app::Cmd::SetView(crate::config::ViewMode::Grid));
    settle(&mut h);
    shot(&mut h, "03-grid");
    h.state_mut().run(crate::app::Cmd::SetView(crate::config::ViewMode::List));

    press(&mut h, &[Action::Menu]);
    shot(&mut h, "04-menu");
    press(&mut h, &[Action::Back]);

    h.state_mut().run(crate::app::Cmd::Rename);
    settle(&mut h);
    shot(&mut h, "05-osk");
    press(&mut h, &[Action::Back]);

    // Dual pane; open the archive in the right pane.
    press(&mut h, &[Action::View]);
    let idx = h.state().pane().entries.iter().position(|e| e.name.ends_with(".tar.gz"));
    if let Some(i) = idx {
        h.state_mut().pane_mut().set_cursor(i);
        press(&mut h, &[Action::Accept]);
        assert!(h.state().pane().loc.is_archive());
        press(&mut h, &[Action::Accept]); // into the folder inside
    }
    shot(&mut h, "06-dual-archive");
    press(&mut h, &[Action::Left]);
    assert!(h.state().focus == crate::app::Focus::Sidebar);
    shot(&mut h, "07-sidebar");

    h.state_mut().run(crate::app::Cmd::Help);
    settle(&mut h);
    shot(&mut h, "08-help");

    press(&mut h, &[Action::Back]);
    press(&mut h, &[Action::View]); // back to single pane

    features(&mut h);
}

fn osk_type(h: &mut Harness<'static, App>, text: &str) {
    if let Some(crate::app::Overlay::Osk(k)) = h.state_mut().overlay.as_mut() {
        k.text.clear();
        k.caret = 0;
        k.insert(text);
    }
    settle(h);
}

fn wait_jobs(h: &mut Harness<'static, App>) {
    for _ in 0..200 {
        settle(h);
        if h.state().jobs.is_empty() {
            return;
        }
    }
    panic!("jobs did not finish");
}

fn features(h: &mut Harness<'static, App>) {
    use crate::app::Cmd;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    h.state_mut().overlay = None;
    h.state_mut().focus = crate::app::Focus::Panes;
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir.clone()), None, &cfg);
    // A photo for thumbnails.
    image::RgbaImage::from_fn(300, 200, |x, y| image::Rgba([(x % 255) as u8, (y % 255) as u8, 160, 255]))
        .save(dir.join("photo.png"))
        .unwrap();
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::Grid));
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().reload(&cfg);
    settle(h);
    settle(h);
    shot(h, "09-thumbs-grid");
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));

    // Live filter.
    h.state_mut().run(Cmd::Filter);
    osk_type(h, "o");
    shot(h, "10-filter-live");
    h.state_mut().osk_submit();
    settle(h);
    assert!(h.state().pane().entries.iter().all(|e| e.name.to_lowercase().contains('o')));
    press(h, &[Action::Back]);
    assert!(h.state().pane().filter.is_none());

    // Conflict prompt: copy note.txt into folder twice.
    let i = h.state().pane().entries.iter().position(|e| e.name == "note.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::Copy);
    let f = h.state().pane().entries.iter().position(|e| e.name == "folder").unwrap();
    h.state_mut().pane_mut().set_cursor(f);
    press(h, &[Action::Accept]);
    h.state_mut().run(Cmd::Paste);
    wait_jobs(h);
    h.state_mut().run(Cmd::Paste);
    settle(h);
    shot(h, "11-conflict");
    assert!(matches!(h.state().overlay, Some(crate::app::Overlay::Menu(_))));
    press(h, &[Action::Accept]); // keep both
    wait_jobs(h);
    let names: Vec<_> = h.state().pane().entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"note (2).txt".to_string()), "{names:?}");
    press(h, &[Action::Back]);

    // Paste into an archive, then rename inside it.
    let a = h.state().pane().entries.iter().position(|e| e.name == "code.tar.gz").unwrap();
    h.state_mut().pane_mut().set_cursor(a);
    press(h, &[Action::Accept]);
    h.state_mut().run(Cmd::Paste);
    wait_jobs(h);
    let names: Vec<_> = h.state().pane().entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"note.txt".to_string()), "{names:?}");
    let n = names.iter().position(|e| e == "note.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(n);
    h.state_mut().run(Cmd::Rename);
    osk_type(h, "renamed.txt");
    h.state_mut().osk_submit();
    wait_jobs(h);
    shot(h, "12-archive-edited");
    let names: Vec<_> = h.state().pane().entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"renamed.txt".to_string()), "{names:?}");
    assert_eq!(h.state().pane().current().map(|e| e.name.as_str()), Some("renamed.txt"));
    press(h, &[Action::Back]);

    // Recursive search.
    h.state_mut().run(Cmd::Search);
    osk_type(h, "note");
    h.state_mut().osk_submit();
    settle(h);
    settle(h);
    shot(h, "13-search");
    let names: Vec<_> = h.state().pane().entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"folder/note.txt".to_string()), "{names:?}");

    // Icon theme picker: browse, then apply one.
    h.state_mut().run(Cmd::IconThemes);
    let target = match &h.state().overlay {
        Some(crate::app::Overlay::Themes(t)) => t.items.iter().position(|(id, _)| id.as_deref() == Some("breeze")),
        _ => panic!("no theme picker"),
    };
    if let Some(Some(i)) = target.map(Some) {
        if let Some(crate::app::Overlay::Themes(t)) = h.state_mut().overlay.as_mut() {
            t.cursor = i;
        }
    }
    settle(h);
    settle(h);
    shot(h, "14-themes");
    // The picker must not grow while the cursor moves (it used to, every frame).
    let height = |h: &Harness<'static, App>| {
        h.ctx.memory(|m| m.area_rect(egui::Id::new("overlay"))).map(|r| r.height()).unwrap()
    };
    let before = height(h);
    for _ in 0..6 {
        press(h, &[Action::Down]);
    }
    press(h, &[Action::Up, Action::Up]);
    assert!((height(h) - before).abs() < 1.0, "picker grew: {before} -> {}", height(h));
    if let (Some(crate::app::Overlay::Themes(t)), Some(i)) = (h.state_mut().overlay.as_mut(), target) {
        t.cursor = i;
    }
    press(h, &[Action::Accept]);
    assert!(h.state().overlay.is_none());
    if target.is_some() {
        assert_eq!(h.state().cfg.icon_theme.as_deref(), Some("breeze"));
    }
    settle(h);
    shot(h, "15-theme-applied");

    submenus(h);
}

fn submenus(h: &mut Harness<'static, App>) {
    use crate::app::Overlay;
    let menu_depth = |h: &Harness<'static, App>| match &h.state().overlay {
        Some(Overlay::Menu(m)) => Some(m.depth()),
        _ => None,
    };
    press(h, &[Action::Menu]);
    let view = match &h.state().overlay {
        Some(Overlay::Menu(m)) => m.items.iter().position(|i| matches!(i.cmd, crate::app::Cmd::Submenu(crate::app::SubMenu::View))).unwrap(),
        _ => panic!("no menu"),
    };
    if let Some(Overlay::Menu(m)) = h.state_mut().overlay.as_mut() {
        m.cursor = view;
    }
    press(h, &[Action::Right]);
    assert_eq!(menu_depth(h), Some(1));
    // List is first, Grid second: toggling keeps the submenu open.
    let was = h.state().cfg.view;
    press(h, &[Action::Down, Action::Accept]);
    assert_eq!(h.state().cfg.view, crate::config::ViewMode::Grid, "was {was:?}");
    assert_eq!(menu_depth(h), Some(1));
    shot(h, "16-submenu");
    press(h, &[Action::Left]);
    assert_eq!(menu_depth(h), Some(0));
    press(h, &[Action::Back]);
    assert_eq!(menu_depth(h), None);

    settings(h);
}

fn settings(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay, row};
    h.state_mut().run(Cmd::Settings);
    settle(h);
    // → over the swatches previews colours immediately.
    press(h, &[Action::Right, Action::Right]);
    assert_eq!(h.state().cfg.accent, crate::config::ACCENT_PRESETS[2]);
    shot(h, "17-settings");
    // Mix and save a colour from the "+" tile.
    let add = crate::app::swatches(&h.state().cfg).len();
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.swatch = add;
    }
    press(h, &[Action::Accept]);
    assert!(matches!(&h.state().overlay, Some(Overlay::Settings(st)) if st.editor.is_some()));
    press(h, &[Action::Right, Action::Right, Action::Down, Action::Right]);
    shot(h, "18-color-editor");
    press(h, &[Action::Down, Action::Down, Action::Accept]); // to Save
    assert_eq!(h.state().cfg.custom_colors.len(), 1);
    assert_eq!(h.state().cfg.accent, Some(h.state().cfg.custom_colors[0]));
    // Y deletes the saved colour under the cursor.
    press(h, &[Action::Actions]);
    assert!(h.state().cfg.custom_colors.is_empty());
    // Icon sizes.
    let before = h.state().cfg.list_icon;
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::LIST_ICON;
    }
    press(h, &[Action::Right, Action::Right, Action::Right, Action::Right]);
    assert_eq!(h.state().cfg.list_icon, before + 8.0);
    press(h, &[Action::Back]);
    assert!(h.state().overlay.is_none());
    shot(h, "19-big-list-icons");

    h.state_mut().run(Cmd::NewTab);
    h.state_mut().run(Cmd::NewTab);
    settle(h);
    shot(h, "20-tabs");
    assert_eq!(h.state().tabs.len(), 3);

    pin_icons(h);
}

fn pin_icons(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("folder");
    h.state_mut().run(Cmd::Pin(dir.clone()));
    h.state_mut().run(Cmd::SetIconTheme(Some("Papirus-Dark".into())));
    h.state_mut().run(Cmd::Settings);
    settle(h);
    shot(h, "21-settings-padding");
    press(h, &[Action::Back]);
    h.state_mut().run(Cmd::ChooseIcon(dir.clone()));
    let groups = match &h.state().overlay {
        Some(Overlay::IconPicker(p)) => p.groups.len(),
        _ => panic!("no picker"),
    };
    assert!(groups > 1, "Papirus should have colour groups");
    press(h, &[Action::NextTab, Action::NextTab]);
    for _ in 0..7 {
        press(h, &[Action::Down]);
    }
    settle(h);
    shot(h, "22-icon-picker");
    let chosen = match &h.state().overlay {
        Some(Overlay::IconPicker(p)) => {
            // The cursor row must be scrolled into view.
            assert!(p.scroll_y > 0.0, "grid did not scroll to the cursor");
            p.names()[p.cursor].clone()
        }
        _ => unreachable!(),
    };
    press(h, &[Action::Accept]);
    assert_eq!(h.state().cfg.pinned_icons.get(&dir.to_string_lossy().into_owned()), Some(&chosen));
    settle(h);
    shot(h, "23-pinned-icon");

    // Own picture: the photo from earlier becomes the pinned folder's icon.
    // From the icon picker: △ → browse the panes, ✕ on a picture picks it.
    let back = h.state().pane().loc.clone();
    h.state_mut().run(Cmd::ChooseIcon(dir.clone()));
    press(h, &[Action::Actions]);
    assert!(h.state().icon_pick.is_some() && h.state().overlay.is_none());
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir.parent().unwrap().to_path_buf()), None, &cfg);
    settle(h);
    shot(h, "24a-pick-picture");
    let i = h.state().pane().entries.iter().position(|e| e.name == "photo.png").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    assert!(h.state().icon_pick.is_none());
    assert_eq!(h.state().pane().loc, back, "pane goes back after picking");
    let stored = h.state().cfg.pinned_icons.get(&dir.to_string_lossy().into_owned()).cloned().unwrap();
    assert!(stored.starts_with('/') && std::path::Path::new(&stored).exists(), "{stored}");
    settle(h);
    shot(h, "24-pinned-photo-icon");

    range_selection(h);
    closing_tabs(h);
    passwords(h);
}

fn closing_tabs(h: &mut Harness<'static, App>) {
    use crate::app::Cmd;
    while h.state().tabs.len() < 3 {
        h.state_mut().run(Cmd::NewTab);
    }
    h.state_mut().tab = 1;
    h.state_mut().run(Cmd::CloseTabAt(1)); // current: its right neighbour takes over
    assert_eq!((h.state().tabs.len(), h.state().tab), (2, 1));
    h.state_mut().run(Cmd::CloseTabAt(0)); // left of current: current shifts
    assert_eq!((h.state().tabs.len(), h.state().tab), (1, 0));
    h.state_mut().run(Cmd::CloseTabAt(0)); // never the last one
    assert_eq!(h.state().tabs.len(), 1);
    settle(h);
}

fn type_and_submit(h: &mut Harness<'static, App>, text: &str) {
    osk_type(h, text);
    h.state_mut().osk_submit();
    settle(h);
}

fn passwords(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Osk, OskPurpose, Overlay};
    if std::process::Command::new("7z").arg("i").output().is_err() {
        return;
    }
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let work = dir.join("pw");
    std::fs::create_dir_all(work.join("secret")).unwrap();
    std::fs::write(work.join("secret/a.txt"), "hidden text").unwrap();
    for (name, args) in [("locked.zip", vec!["-tzip", "-pqwe", "-mem=AES256"]), ("names.7z", vec!["-pqwe", "-mhe=on"])] {
        let ok = std::process::Command::new("7z").arg("a").args(&args).arg(dir.join(name)).arg("secret").current_dir(&work).stdout(std::process::Stdio::null()).status().unwrap().success();
        assert!(ok);
    }
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir.clone()), None, &cfg);
    h.state_mut().pane_mut().reload(&cfg); // the archives were made after the last listing
    let i = h.state().pane().entries.iter().position(|e| e.name == "locked.zip").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    let is_pw_prompt = |h: &Harness<'static, App>| matches!(&h.state().overlay, Some(Overlay::Osk(Osk { purpose: OskPurpose::Password { .. }, secret: true, .. })));

    // Extract: asks, a wrong password asks again, the right one extracts.
    h.state_mut().run(Cmd::ExtractHere);
    wait_jobs(h);
    assert!(is_pw_prompt(h), "no password prompt");
    osk_type(h, "qwx");
    shot(h, "26-password");
    h.state_mut().osk_submit();
    wait_jobs(h);
    assert!(is_pw_prompt(h), "wrong password not asked again");
    let title = match &h.state().overlay {
        Some(Overlay::Osk(k)) => k.title.clone(),
        _ => unreachable!(),
    };
    assert!(title.contains(crate::i18n::tr("Wrong password")), "{title}");
    type_and_submit(h, "qwe");
    wait_jobs(h);
    assert!(h.state().overlay.is_none());
    assert_eq!(std::fs::read_to_string(dir.join("secret/a.txt")).unwrap(), "hidden text");

    // 7z with encrypted names: browsing asks first, then lists.
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Archive { archive: dir.join("names.7z"), inner: String::new() }, None, &cfg);
    for _ in 0..50 {
        settle(h);
        if is_pw_prompt(h) {
            break;
        }
    }
    assert!(is_pw_prompt(h), "listing did not ask for a password");
    type_and_submit(h, "qwe");
    for _ in 0..50 {
        settle(h);
        if h.state().pane().loading.is_none() {
            break;
        }
    }
    let names: Vec<_> = h.state().pane().entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, ["secret"]);
    shot(h, "27-7z-unlocked");
}

fn range_selection(h: &mut Harness<'static, App>) {
    use crate::app::Cmd;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir), None, &cfg);
    h.state_mut().focus = crate::app::Focus::Panes;
    let p = h.state_mut().pane_mut();
    p.selected.clear();
    p.set_cursor(2);
    let names = |h: &Harness<'static, App>| -> Vec<usize> {
        let p = h.state().pane();
        let mut v: Vec<usize> = p.entries.iter().enumerate().filter(|(_, e)| p.selected.contains(&e.name)).map(|(i, _)| i).collect();
        v.sort();
        v
    };
    h.state_mut().handle(Input::Press(Action::Select));
    press(h, &[Action::Down, Action::Down]);
    assert_eq!(names(h), [2, 3, 4]);
    // Going back gives items their old state, past the anchor selects upwards.
    press(h, &[Action::Up]);
    assert_eq!(names(h), [2, 3]);
    press(h, &[Action::Up, Action::Up]);
    assert_eq!(names(h), [1, 2]);
    // Arrows stay in this pane while selecting.
    press(h, &[Action::Left]);
    assert!(h.state().focus == crate::app::Focus::Panes);
    h.state_mut().handle(Input::Release(Action::Select));
    // Deselecting with a range: start on a selected item.
    h.state_mut().pane_mut().set_cursor(2);
    h.state_mut().handle(Input::Press(Action::Select));
    press(h, &[Action::Up]);
    h.state_mut().handle(Input::Release(Action::Select));
    assert!(names(h).is_empty());
    shot(h, "25-after-range");
}
