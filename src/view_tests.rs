//! Offscreen renders of the main screens (wgpu). Not run by default:
//!   POLKA_SHOT_DIR=/tmp/shots POLKA_TEST_DIR=/some/dir cargo test -- --ignored
//! POLKA_SHOT_DIR, when set, receives PNGs to eyeball.

use std::path::PathBuf;

use egui_kittest::Harness;

use crate::app::App;
use crate::input::{Action, Input};

/// Tests run in parallel in one process; the one that changes process-wide
/// things (the language, the drive listing) runs on its own.
static GLOBALS: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn harness(dir: PathBuf) -> Harness<'static, App> {
    harness_with(dir, [1100.0, 680.0], "")
}

/// `tag` gives a config of its own (one that leaves the others' alone).
fn harness_with(dir: PathBuf, size: [f32; 2], tag: &str) -> Harness<'static, App> {
    // Never touch the real config or gamepad from tests.
    let cfg = std::env::temp_dir().join(format!("polka-test-cfg-{}{tag}", std::process::id()));
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", &cfg);
        std::env::set_var("POLKA_NO_GAMEPAD", "1");
        std::env::set_var("XDG_CACHE_HOME", cfg.join("cache"));
        // The trash (and everything else in ~/.local/share) of their own.
        std::env::set_var("XDG_DATA_HOME", cfg.join("data"));
        let _ = std::fs::create_dir_all(cfg.join("data/Trash/files"));
        let _ = std::fs::create_dir_all(cfg.join("data/Trash/info"));
    }
    Harness::builder()
        .with_size(size)
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
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
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
        let kind = h.state().overlay.as_ref().map(std::mem::discriminant);
        h.ctx.memory(|m| m.area_rect(egui::Id::new(("overlay", kind, None::<(String, usize)>)))).map(|r| r.height()).unwrap()
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
    compress_dialog(h);
    volumes(h);
    viewer(h);
    editing(h);
    replacing(h);
    folding_and_locks(h);
    templates(h);
    cursors(h);
    music(h);
    video(h);
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

fn compress_dialog(h: &mut Harness<'static, App>) {
    use crate::app::{crow, Cmd, Overlay};
    use crate::ops::{ArchiveFormat, Level, Method};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir.join("pw")), None, &cfg);
    h.state_mut().pane_mut().reload(&cfg);
    let i = h.state().pane().entries.iter().position(|e| e.name == "secret").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::CompressAsk);
    settle(h);
    shot(h, "28-compress-default");
    let dlg = |h: &Harness<'static, App>| match &h.state().overlay {
        Some(Overlay::Compress(d)) => (d.row, d.opts.clone()),
        _ => panic!("archive dialog closed"),
    };
    assert_eq!(dlg(h).0, crow::CREATE);
    // Up skips "encrypt names" (no password yet) and lands on the password.
    press(h, &[Action::Up]);
    assert_eq!(dlg(h).0, crow::PASSWORD);
    press(h, &[Action::Up, Action::Right, Action::Up, Action::Right, Action::Up, Action::Right]);
    let (row, o) = dlg(h);
    assert_eq!((row, o.split, o.level, o.method), (crow::METHOD, Some(10 * 1024 * 1024), Level::Max, Method::Bzip2));
    let seven = crate::ops::have_7z();
    press(h, &[Action::Up, Action::Right]);
    assert_eq!(dlg(h).1.format, if seven { ArchiveFormat::SevenZ } else { ArchiveFormat::TarZst });
    if !seven {
        return;
    }
    // Password: entered twice; a mismatch keeps none.
    press(h, &[Action::Down, Action::Down, Action::Down, Action::Down, Action::Accept]);
    type_and_submit(h, "one");
    type_and_submit(h, "two");
    assert_eq!(dlg(h).1.password, None);
    press(h, &[Action::Accept]);
    type_and_submit(h, "s3cret pw");
    shot(h, "29-compress-confirm");
    type_and_submit(h, "s3cret pw");
    assert_eq!(dlg(h).1.password.as_deref(), Some("s3cret pw"));
    press(h, &[Action::Down, Action::Accept]); // encrypt names
    assert!(dlg(h).1.encrypt_names);
    shot(h, "30-compress-7z");
    press(h, &[Action::Menu]);
    wait_jobs(h);
    let out = dir.join("pw/secret.7z");
    assert!(out.exists(), "no archive");
    crate::vfs::set_password(&out, "s3cret pw");
    let idx = crate::vfs::archive_index(&out).unwrap();
    assert!(idx.dirs["secret"].iter().any(|e| e.name == "a.txt"));
    // The settings are remembered, the password is not.
    h.state_mut().run(Cmd::CompressAsk);
    let (_, o) = dlg(h);
    assert_eq!((o.format, o.method, o.level, o.password), (ArchiveFormat::SevenZ, Method::Bzip2, Level::Max, None));
    press(h, &[Action::Back]);
    assert!(h.state().overlay.is_none());
}

fn volumes(h: &mut Harness<'static, App>) {
    use crate::app::Cmd;
    use crate::ops::{ArchiveFormat, CompressOpts};
    use crate::vfs::Location;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let work = dir.join("vols");
    std::fs::create_dir_all(work.join("pack/sub")).unwrap();
    let mut x = 0x1234_5678_9abc_def1u64;
    let noise: Vec<u8> = (0..200_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(work.join("pack/sub/noise.bin"), &noise).unwrap();
    std::fs::write(work.join("pack/readme.txt"), "volumes").unwrap();
    let job = crate::ops::spawn(
        crate::ops::Op::Compress { sources: vec![work.join("pack")], out: work.join("pack.zip"), opts: CompressOpts { split: Some(64 * 1024), ..CompressOpts::new(ArchiveFormat::Zip) } },
        || {},
    );
    while job.finished_at.lock().unwrap().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::fs::write(work.join("notes.txt"), "x").unwrap();
    if let Some(rar) = std::env::var_os("POLKA_RAR_VOLUMES") {
        for e in std::fs::read_dir(rar).unwrap().flatten() {
            std::fs::copy(e.path(), work.join(e.file_name())).unwrap();
        }
    }
    // Volumes one by one first; folding is checked further down.
    h.state_mut().cfg.collapse_volumes = false;
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    let cfg2 = h.state().cfg.clone();
    h.state_mut().pane_mut().navigate(Location::Local(work.clone()), None, &cfg2);
    settle(h);
    let pos = |h: &Harness<'static, App>, n: &str| h.state().pane().entries.iter().position(|e| e.name == n).unwrap();
    let i = pos(h, "pack.zip.002");
    h.state_mut().pane_mut().set_cursor(i);
    settle(h);
    let m = h.state().pane().volumes.get("pack.zip.002").cloned().expect("volume group");
    assert_eq!((m.index, m.count, m.archive.as_str()), (1, 4, "pack.zip"));
    shot(h, "31-volumes-list");
    if h.state().pane().volumes.contains_key("multi.part2.rar") {
        // The mouse over another set takes the highlight over from the cursor.
        let row = h.state().pane().entries.iter().position(|e| e.name == "multi.part2.rar").unwrap();
        assert_eq!(row, 2, "layout changed; adjust the hover point");
        h.hover_at(egui::pos2(320.0, 218.0));
        settle(h);
        let key = h.state().pane().volumes["multi.part2.rar"].key.clone();
        assert_eq!(h.state().pane().hover_volume.as_ref(), Some(&key));
        shot(h, "31b-volumes-hover");
        h.hover_at(egui::pos2(1000.0, 600.0));
        settle(h);
    }
    if h.state().pane().volumes.contains_key("multi.part1.rar") {
        let i = pos(h, "multi.part3.rar");
        h.state_mut().pane_mut().set_cursor(i);
        h.state_mut().run(Cmd::SetView(crate::config::ViewMode::Grid));
        settle(h);
        shot(h, "32-volumes-grid");
        // Into the RAR set from its third volume.
        press(h, &[Action::Accept]);
        settle(h);
        match &h.state().pane().loc {
            Location::Archive { archive, .. } => assert!(archive.ends_with("multi.part1.rar")),
            l => panic!("{l:?}"),
        }
        assert!(!h.state().pane().entries.is_empty());
        press(h, &[Action::Back]);
        h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    }
    let _ = cfg;
    // Into the split zip from its second volume.
    let i = pos(h, "pack.zip.002");
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    settle(h);
    let names: Vec<_> = h.state().pane().entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, ["pack"]);
    press(h, &[Action::Accept]);
    shot(h, "33-inside-split");
    // Delete inside: the archive is cut again into fewer volumes.
    h.state_mut().run(Cmd::DeleteInArchive { archive: work.join("pack.zip.001"), inners: vec!["pack/sub".into()] });
    wait_jobs(h);
    assert!(!work.join("pack.zip.003").exists(), "volumes were not re-cut");
    press(h, &[Action::Back, Action::Back]);
    settle(h);
    assert!(h.state().pane().entries.iter().all(|e| e.name != "pack.zip.002"));
    // Extract from a volume.
    let i = pos(h, "pack.zip.001");
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::ExtractHere);
    wait_jobs(h);
    // "pack" exists already (the source), so it lands as "pack (2)".
    assert_eq!(std::fs::read_to_string(work.join("pack (2)/readme.txt")).unwrap(), "volumes");
}

fn viewer(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay};
    use crate::vfs::Location;
    use crate::viewer::Content;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let work = dir.join("view");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/src/viewer/text.rs"), work.join("text.rs")).unwrap();
    std::fs::write(
        work.join("README.md"),
        "# Polka\n\nФайловый менеджер для **геймпада**, с [ссылкой](https://example.org) и [переходом](#установка).\n\n> [!NOTE]\n> Работает с *DualSense* и Xbox.\n\n![пример](a-red.png)\n\n## Возможности\n\n- [x] архивы\n- [ ] видео\n  1. вложенный\n  2. список\n\n| Кнопка | Действие |\n|:--|--:|\n| A | Открыть |\n| B | Назад |\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n---\n\n## Установка\n\n- пункт\n- `код`\n",
    )
    .unwrap();
    for (name, w, hgt, c) in [("a-red.png", 640u32, 400u32, [200u8, 60, 60]), ("b-wide.png", 9000, 300, [60, 160, 90]), ("c-blue.png", 300, 500, [50, 90, 210])] {
        image::RgbaImage::from_fn(w, hgt, |x, y| {
            let k = ((x / 40 + y / 40) % 2) as u8 * 40;
            image::Rgba([c[0].saturating_sub(k), c[1].saturating_sub(k), c[2], 255])
        })
        .save(work.join(name))
        .unwrap();
    }
    std::fs::write(
        work.join("d-logo.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="120"><rect width="200" height="120" rx="20" fill="#3584e4"/><circle cx="60" cy="60" r="35" fill="#f6d32d"/></svg>"##,
    )
    .unwrap();
    {
        use image::codecs::gif::GifEncoder;
        let f = std::fs::File::create(work.join("e-anim.gif")).unwrap();
        let mut enc = GifEncoder::new(f);
        for c in [[220u8, 50, 50, 255], [50, 200, 80, 255]] {
            let frame = image::Frame::from_parts(image::RgbaImage::from_pixel(120, 80, image::Rgba(c)), 0, 0, image::Delay::from_numer_denom_ms(200, 1));
            enc.encode_frame(frame).unwrap();
        }
    }
    assert!(crate::viewer::kind(&dir.join("ail-cli.desktop"), "ail-cli.desktop").is_none() || !dir.join("ail-cli.desktop").exists());
    assert!(crate::viewer::kind(&dir.join("code.tar.gz"), "code.tar.gz").is_none());
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().focus = crate::app::Focus::Panes;
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    h.state_mut().pane_mut().navigate(Location::Local(work.clone()), None, &cfg);
    settle(h);
    let open = |h: &mut Harness<'static, App>, name: &str| {
        let i = h.state().pane().entries.iter().position(|e| e.name == name).unwrap();
        h.state_mut().pane_mut().set_cursor(i);
        press(h, &[Action::Accept]);
        for _ in 0..30 {
            settle(h);
            let ready = match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
                Some(Content::Text(t)) => t.doc.lang.is_none() || t.doc.highlighted,
                Some(Content::Image(im)) => im.pic.pixels.is_some() || im.pic.error.is_some(),
                _ => false,
            };
            if ready {
                break;
            }
        }
        settle(h);
    };
    let text_scroll = |h: &Harness<'static, App>| match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Text(t)) => t.scroll,
        _ => panic!("no text viewer"),
    };

    // Code with highlighting.
    open(h, "text.rs");
    assert!(h.state().cur_tab().viewer.is_some(), "viewer did not open");
    shot(h, "34-text-viewer");
    press(h, &[Action::Down, Action::Down, Action::Down, Action::RightTrigger]);
    assert!(text_scroll(h).y > 0.0, "did not scroll");
    // Settings beside the file: theme, font size, wrapping.
    press(h, &[Action::Menu]);
    assert!(matches!(h.state().overlay, Some(Overlay::ViewerSettings(_))));
    press(h, &[Action::Right]);
    assert_eq!(h.state().cfg.viewer.theme, "One Dark");
    press(h, &[Action::Down, Action::Right]);
    let font = h.state().cfg.viewer.font.clone();
    press(h, &[Action::Down, Action::Right, Action::Right]);
    assert_eq!(h.state().cfg.viewer.font_size, 17.0);
    shot(h, "35-viewer-settings");
    press(h, &[Action::Down, Action::Accept]);
    assert!(!h.state().cfg.viewer.wrap);
    press(h, &[Action::Back]);
    press(h, &[Action::Right, Action::Right]);
    assert!(text_scroll(h).x > 0.0, "no sideways scroll without wrapping");
    shot(h, "36-text-nowrap");
    eprintln!("viewer font picked: {font:?}");
    // Back to the list, on the same file.
    press(h, &[Action::Back]);
    assert!(h.state().cur_tab().viewer.is_none());
    assert_eq!(h.state().pane().current().unwrap().name, "text.rs");
    h.state_mut().cfg.viewer.wrap = true;

    open(h, "README.md");
    shot(h, "37-markdown");
    press(h, &[Action::Back]);

    // Pictures: ←/→ walks the folder, L2/R2 zoom, ✕ 1:1.
    open(h, "a-red.png");
    shot(h, "38-image");
    press(h, &[Action::Right]);
    for _ in 0..30 {
        settle(h);
        if let Some(Content::Image(im)) = h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
            if im.pic.pixels.is_some() {
                break;
            }
        }
    }
    let (full, scale, actual) = match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Image(im)) => (im.pic.full, im.scale(), im.actual),
        _ => panic!("no image viewer"),
    };
    assert_eq!(full, Some([9000, 300]), "wide picture");
    assert!(actual > 1.0, "huge picture should be scaled down into the texture: {actual}");
    assert_eq!(h.state().pane().current().unwrap().name, "b-wide.png", "pane cursor follows");
    shot(h, "39-image-wide");
    press(h, &[Action::RightTrigger, Action::RightTrigger]);
    let zoomed = match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Image(im)) => im.scale(),
        _ => unreachable!(),
    };
    assert!(zoomed > scale * 1.5);
    press(h, &[Action::Accept]);
    let pct = match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Image(im)) => im.scale() / im.actual,
        _ => unreachable!(),
    };
    assert!((pct - 1.0).abs() < 1e-3, "✕ should show 1:1, got {pct}");
    shot(h, "40-image-actual");
    press(h, &[Action::Actions]);
    assert!(matches!(h.state().overlay, Some(Overlay::Menu(_))));
    shot(h, "41-viewer-actions");
    press(h, &[Action::Back, Action::Back]);
    // SVG and GIF go through egui's loaders.
    for name in ["d-logo.svg", "e-anim.gif"] {
        open(h, name);
        for _ in 0..10 {
            settle(h);
        }
        match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
            Some(Content::Image(im)) => assert!(im.pic.error.is_none() && im.pic.pixels.is_some(), "{name}"),
            _ => panic!("{name} not shown"),
        }
        shot(h, &format!("40b-{name}"));
        press(h, &[Action::Back]);
    }
    assert!(h.state().cur_tab().viewer.is_none());

    // A text file inside an archive.
    h.state_mut().pane_mut().navigate(Location::Archive { archive: dir.join("code.tar.gz"), inner: "src".into() }, None, &cfg);
    for _ in 0..20 {
        settle(h);
        if h.state().pane().loading.is_none() {
            break;
        }
    }
    open(h, "main.rs");
    for _ in 0..30 {
        if h.state().cur_tab().viewer.is_some() {
            break;
        }
        settle(h);
    }
    assert_eq!(h.state().cur_tab().viewer.as_ref().map(|v| v.name.as_str()), Some("main.rs"));
    // Dual mode: the viewer takes both panes.
    h.state_mut().run(Cmd::ToggleDual);
    settle(h);
    shot(h, "42-viewer-dual-archive");
    press(h, &[Action::Back]);
    h.state_mut().run(Cmd::ToggleDual);
    settle(h);
}

fn key(h: &mut Harness<'static, App>, key: egui::Key, modifiers: egui::Modifiers) {
    h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.event(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
}

fn type_text(h: &mut Harness<'static, App>, s: &str) {
    h.event(egui::Event::Text(s.into()));
    h.step();
}

fn editing(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay};
    use crate::vfs::Location;
    use crate::viewer::Content;
    use egui::{Key, Modifiers};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let work = dir.join("view");
    std::fs::write(work.join("notes.txt"), "first line\nfoo bar foo\nfood fork\n").unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(work.clone()), None, &cfg);
    settle(h);
    let open = |h: &mut Harness<'static, App>, name: &str| {
        let i = h.state().pane().entries.iter().position(|e| e.name == name).unwrap();
        h.state_mut().pane_mut().set_cursor(i);
        press(h, &[Action::Accept]);
        settle(h);
    };
    let text = |h: &mut Harness<'static, App>| -> (String, bool) {
        match h.state_mut().text_mut() {
            Some(t) => (t.doc.text.clone(), t.ed.dirty),
            None => panic!("no text viewer"),
        }
    };
    let ctrl = Modifiers::COMMAND;

    // Keyboard: type at the end of the first line, then save.
    open(h, "notes.txt");
    key(h, Key::End, Modifiers::NONE);
    type_text(h, "!");
    // A space typed on the keyboard stays (the key must not act as □).
    h.event(egui::Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.event(egui::Event::Text(" ".into()));
    h.step();
    key(h, Key::Space, Modifiers::NONE);
    settle(h);
    assert_eq!(text(h).0, "first line! \nfoo bar foo\nfood fork\n", "space");
    key(h, Key::Backspace, Modifiers::NONE);
    assert_eq!(text(h), ("first line!\nfoo bar foo\nfood fork\n".into(), true));
    settle(h);
    shot(h, "43-edit-keyboard");
    // Search: words, then a regex.
    key(h, Key::F, ctrl);
    type_text(h, "foo");
    settle(h);
    let found = |h: &mut Harness<'static, App>| h.state_mut().text_mut().and_then(|t| t.ed.find.as_ref().map(|f| f.matches.len())).unwrap_or(0);
    assert_eq!(found(h), 3);
    key(h, Key::W, Modifiers::ALT);
    assert_eq!(found(h), 2);
    key(h, Key::Enter, Modifiers::NONE);
    shot(h, "44-find");
    key(h, Key::W, Modifiers::ALT);
    key(h, Key::R, Modifiers::ALT);
    for _ in 0..3 {
        key(h, Key::Backspace, Modifiers::NONE);
    }
    type_text(h, "fo+[dr]");
    assert_eq!(found(h), 2, "regex fo+[dr] in 'food fork'");
    key(h, Key::Escape, Modifiers::NONE);
    key(h, Key::Escape, Modifiers::NONE);
    assert!(h.state_mut().text_mut().unwrap().ed.find.is_none());
    key(h, Key::S, ctrl);
    settle(h);
    assert_eq!(std::fs::read_to_string(work.join("notes.txt")).unwrap(), "first line!\nfoo bar foo\nfood fork\n");
    assert!(!text(h).1);

    // Gamepad: ✕ edits, ✕ types on the OSK, □ erases, □ held selects.
    {
        // As if picking up the pad after the keyboard was put away.
        let t = h.state_mut().text_mut().unwrap();
        t.ed.caret = 0;
        t.ed.editing = false;
    }
    press(h, &[Action::Accept]);
    assert!(h.state().text_editing());
    press(h, &[Action::Down, Action::Accept]);
    assert!(matches!(h.state().overlay, Some(Overlay::Osk(_))));
    type_and_submit(h, "ab ");
    assert!(text(h).0.contains("\nab foo bar"), "{:?}", text(h).0);
    press(h, &[Action::Select]);
    h.state_mut().handle(Input::Release(Action::Select));
    assert!(text(h).0.contains("\nabfoo bar"), "{:?}", text(h).0);
    h.state_mut().handle(Input::Press(Action::Select));
    press(h, &[Action::Right, Action::Right, Action::Right]);
    h.state_mut().handle(Input::Release(Action::Select));
    assert_eq!(h.state_mut().text_mut().unwrap().selected_text().as_deref(), Some("foo"));
    press(h, &[Action::Actions]);
    shot(h, "45-edit-menu");
    press(h, &[Action::Back]);
    // Leaving with changes asks.
    press(h, &[Action::Back, Action::Back]);
    assert!(matches!(h.state().overlay, Some(Overlay::Menu(_))), "no unsaved-changes prompt");
    shot(h, "46-unsaved");
    press(h, &[Action::Down, Action::Accept]); // don't save
    assert!(h.state().cur_tab().viewer.is_none());
    assert_eq!(std::fs::read_to_string(work.join("notes.txt")).unwrap(), "first line!\nfoo bar foo\nfood fork\n");

    // Markdown: GitHub view by default, ⧉ flips to the source.
    open(h, "README.md");
    for _ in 0..5 {
        settle(h);
    }
    shot(h, "47-md-rendered");
    press(h, &[Action::Down, Action::Down, Action::Down]);
    settle(h);
    shot(h, "48-md-rendered-scrolled");
    press(h, &[Action::RightTrigger]);
    settle(h);
    shot(h, "48a-md-lists-table-code");
    h.state_mut().viewer_link("#установка".into());
    settle(h);
    shot(h, "48b-md-anchor");
    let (scroll, anchor) = {
        let t = h.state_mut().text_mut().unwrap();
        (t.md_scroll, t.anchors.get("установка").copied())
    };
    assert!(anchor.is_some_and(|a| a > 100.0 && (scroll - a).abs() < 60.0 || scroll > 100.0), "anchor {anchor:?} scroll {scroll}");
    press(h, &[Action::View]);
    assert!(!h.state().cfg.viewer.md_rendered);
    shot(h, "49-md-source");
    press(h, &[Action::View, Action::Back]);
    assert!(h.state().cfg.viewer.md_rendered);

    // A file inside an archive is saved back into it.
    h.state_mut().pane_mut().navigate(Location::Archive { archive: dir.join("code.tar.gz"), inner: "src".into() }, None, &cfg);
    for _ in 0..20 {
        settle(h);
        if h.state().pane().loading.is_none() {
            break;
        }
    }
    open(h, "fonts.rs");
    for _ in 0..30 {
        if h.state().cur_tab().viewer.is_some() {
            break;
        }
        settle(h);
    }
    assert!(matches!(h.state().cur_tab().viewer.as_ref().map(|v| &v.content), Some(Content::Text(_))));
    key(h, Key::Home, ctrl);
    type_text(h, "// edited in Polka\n");
    assert!(h.state_mut().text_mut().unwrap().ed.dirty);
    key(h, Key::S, ctrl);
    wait_jobs(h);
    let out = dir.join("x-fonts");
    std::fs::create_dir_all(&out).unwrap();
    crate::vfs::extract(&dir.join("code.tar.gz"), &["src/fonts.rs".into()], &out, |_| true).unwrap();
    assert!(std::fs::read_to_string(out.join("fonts.rs")).unwrap().starts_with("// edited in Polka\n"));
    assert!(!h.state_mut().text_mut().unwrap().ed.dirty);
    // ○ leaves editing first, then closes.
    press(h, &[Action::Back]);
    assert!(h.state().cur_tab().viewer.is_some());
    press(h, &[Action::Back]);
    assert!(h.state().cur_tab().viewer.is_none());
    let _ = Cmd::Nothing;
}

/// Pick a menu item by its label (opening submenus) with the gamepad.
fn menu_pick(h: &mut Harness<'static, App>, label: &str) {
    use crate::app::Overlay;
    let Some(Overlay::Menu(root)) = h.state_mut().overlay.as_mut() else { panic!("no menu") };
    let depth = root.depth();
    let m = root.at_depth_mut(depth);
    let i = m.items.iter().position(|it| it.label == label).unwrap_or_else(|| panic!("no {label:?} in {:?}", m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()));
    m.cursor = i;
    press(h, &[Action::Accept]);
}

fn folding_and_locks(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay, row};
    use crate::vfs::Location;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let work = dir.join("vols");
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().focus = crate::app::Focus::Panes;
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    // Settings: the new rows; folding switched on from there.
    h.state_mut().run(Cmd::Settings);
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::THEME;
    }
    press(h, &[Action::Down]);
    press(h, &[Action::Right, Action::Accept]); // pictures: off
    assert!(!h.state().cfg.open_images);
    press(h, &[Action::Accept]); // and on again
    press(h, &[Action::Right, Action::Accept]); // music: off
    assert!(!h.state().cfg.open_music);
    press(h, &[Action::Accept]); // and on again
    assert!(h.state().cfg.open_music);
    press(h, &[Action::Right, Action::Accept]); // video: off
    assert!(!h.state().cfg.open_videos);
    press(h, &[Action::Accept]); // and on again
    assert!(h.state().cfg.open_videos);
    press(h, &[Action::Down]);
    if h.state().cfg.collapse_volumes {
        press(h, &[Action::Accept]);
    }
    press(h, &[Action::Accept]);
    assert!(h.state().cfg.collapse_volumes);
    // Pictures from the internet: on by default, ✕ flips it.
    press(h, &[Action::Down]);
    assert!(h.state().cfg.viewer.web_images);
    press(h, &[Action::Accept]);
    assert!(!h.state().cfg.viewer.web_images);
    press(h, &[Action::Accept]);
    assert!(h.state().cfg.viewer.web_images);
    shot(h, "50-settings-open-in");
    press(h, &[Action::Back]);
    // Pictures off: they go to the desktop app, and the menu offers Polka.
    h.state_mut().cfg.open_images = false;
    let png = dir.join("view/a-red.png");
    assert!(crate::viewer::kind_in(&png, "a-red.png", crate::viewer::OpenIn::from(&h.state().cfg)).is_none());
    h.state_mut().cfg.open_images = true;
    assert!(crate::viewer::kind_in(&png, "a-red.png", crate::viewer::OpenIn::from(&h.state().cfg)).is_some());

    // Folding: the RAR set and the split zip are one item each.
    h.state_mut().pane_mut().navigate(Location::Local(work.clone()), None, &cfg);
    let c2 = h.state().cfg.clone();
    h.state_mut().pane_mut().refilter_with(None, &c2);
    let i = h.state().pane().entries.iter().position(|e| e.name == "notes.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    settle(h);
    let names = |h: &Harness<'static, App>| h.state().pane().entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    let n = names(h);
    assert!(n.contains(&"pack.zip.001".to_string()) && !n.iter().any(|x| x.starts_with("multi.part") && x != "multi.part1.rar"), "{n:?}");
    let folded = h.state().pane().collapsed.get("multi.part1.rar").map(|c| (c.0.archive.clone(), c.2.len()));
    if let Some(f) = &folded {
        assert_eq!(f.0, "multi.rar");
    }
    shot(h, "51-folded");
    // The cursor on a folded set opens it; moving away folds it again.
    let i = h.state().pane().entries.iter().position(|e| e.name == "pack.zip.001").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    settle(h);
    assert!(names(h).iter().any(|x| x == "pack.zip.001"));
    let open = h.state().pane().entries.iter().filter(|e| e.name.starts_with("pack.zip.")).count();
    assert!(open >= 1);
    shot(h, "52-unfolded");
    // Whole set as the target when folded: copy names.
    let i = h.state().pane().entries.iter().position(|e| e.name == "notes.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    settle(h);
    if folded.is_some() {
        let head = h.state().pane().entries.iter().position(|e| e.name == "multi.part1.rar").unwrap();
        let name = h.state().pane().entries[head].name.clone();
        h.state_mut().pane_mut().selected.insert(name);
        let t = h.state().pane().targets();
        assert_eq!(t.len(), 4, "folded set selects all volumes: {:?}", t.iter().map(|e| &e.name).collect::<Vec<_>>());
        h.state_mut().pane_mut().selected.clear();
    }

    // Lock badges on password archives (made by `passwords`).
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::Grid));
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    for _ in 0..20 {
        settle(h);
        let ok = ["locked.zip", "names.7z"].iter().all(|n| {
            let p = dir.join(n);
            let t = std::fs::metadata(&p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
            t.and_then(|t| crate::locks::locked(&p, t)) == Some(true)
        });
        if ok {
            break;
        }
    }
    settle(h);
    shot(h, "53-lock-badges");
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
}

fn templates(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay};
    use crate::vfs::Location;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let tmpl = dir.join("tmpl");
    std::fs::create_dir_all(tmpl.join("Код")).unwrap();
    std::fs::write(tmpl.join("Заметка.md"), "# Заметка\n").unwrap();
    std::fs::write(tmpl.join("Код/script.sh"), "#!/bin/sh\n").unwrap();
    // dirs reads the templates folder from user-dirs.dirs.
    let conf = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(conf.join("user-dirs.dirs"), format!("XDG_TEMPLATES_DIR=\"{}\"\n", tmpl.display())).unwrap();
    let work = dir.join("made");
    std::fs::create_dir_all(&work).unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(work.clone()), None, &cfg);
    settle(h);
    press(h, &[Action::Menu]);
    menu_pick(h, crate::i18n::tr("New document"));
    shot(h, "54-templates-menu");
    menu_pick(h, "Заметка");
    let start = match &h.state().overlay {
        Some(Overlay::Osk(k)) => (k.string(), k.caret),
        _ => panic!("no name prompt"),
    };
    assert_eq!(start, ("Заметка.md".to_string(), 7), "caret before the extension");
    h.state_mut().osk_submit();
    settle(h);
    assert_eq!(std::fs::read_to_string(work.join("Заметка.md")).unwrap(), "# Заметка\n");
    assert_eq!(h.state().pane().current().unwrap().name, "Заметка.md");
    // A subfolder is a submenu; the empty file from △.
    press(h, &[Action::Menu]);
    menu_pick(h, crate::i18n::tr("New document"));
    menu_pick(h, "Код");
    menu_pick(h, "script");
    h.state_mut().osk_submit();
    settle(h);
    assert!(work.join("script.sh").exists());
    press(h, &[Action::Actions]);
    menu_pick(h, crate::i18n::tr("New document"));
    menu_pick(h, crate::i18n::tr("Empty file"));
    h.state_mut().osk_submit();
    settle(h);
    assert_eq!(std::fs::metadata(work.join(crate::i18n::tr("New file"))).unwrap().len(), 0);
    // Taken names get a number.
    h.state_mut().run(Cmd::NewFile(Some(tmpl.join("Заметка.md"))));
    assert!(matches!(&h.state().overlay, Some(Overlay::Osk(k)) if k.string() == "Заметка (2).md"));
    h.state_mut().overlay = None;
    // Inside an archive.
    h.state_mut().pane_mut().navigate(Location::Archive { archive: dir.join("code.tar.gz"), inner: "src".into() }, None, &cfg);
    for _ in 0..20 {
        settle(h);
        if h.state().pane().loading.is_none() {
            break;
        }
    }
    h.state_mut().run(Cmd::NewFile(Some(tmpl.join("Заметка.md"))));
    h.state_mut().osk_submit();
    wait_jobs(h);
    let out = dir.join("x-tmpl");
    std::fs::create_dir_all(&out).unwrap();
    crate::vfs::extract(&dir.join("code.tar.gz"), &["src/Заметка.md".into()], &out, |_| true).unwrap();
    assert_eq!(std::fs::read_to_string(out.join("Заметка.md")).unwrap(), "# Заметка\n");
}

fn cursors(h: &mut Harness<'static, App>) {
    use crate::app::Cmd;
    use crate::vfs::Location;
    use crate::viewer::Content;
    let src = std::path::Path::new("/usr/share/icons/Adwaita/cursors");
    if !src.is_dir() {
        return;
    }
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let cur = dir.join("theme/cursors");
    std::fs::create_dir_all(&cur).unwrap();
    for n in ["left_ptr", "hand2", "watch", "text", "crosshair", "help"] {
        let _ = std::fs::copy(src.join(n), cur.join(n));
    }
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::Grid));
    h.state_mut().pane_mut().navigate(Location::Local(cur.clone()), None, &cfg);
    for _ in 0..10 {
        settle(h);
    }
    shot(h, "55-cursor-thumbs");
    let i = h.state().pane().entries.iter().position(|e| e.name == "watch").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    settle(h);
    match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Image(im)) => {
            assert!(im.pic.frames.len() > 1, "watch is animated");
            assert!(!im.pic.cursor_sizes.is_empty() && im.pic.hotspot.is_some());
            assert_eq!(im.siblings.len(), 6, "←/→ walk the cursors");
        }
        _ => panic!("cursor not shown"),
    }
    shot(h, "56-cursor-view");
    press(h, &[Action::Right]);
    settle(h);
    shot(h, "57-cursor-next");
    press(h, &[Action::Back]);
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
}

fn music(h: &mut Harness<'static, App>) {
    use crate::app::Overlay;
    use crate::vfs::Location;
    use crate::viewer::Content;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("music");
    std::fs::create_dir_all(&dir).unwrap();
    let ff = |args: &[&str]| std::process::Command::new("ffmpeg").args(["-v", "quiet", "-y"]).args(args).current_dir(&dir).status().is_ok_and(|s| s.success());
    if !ff(&["-f", "lavfi", "-i", "testsrc=size=320x320:duration=1", "-frames:v", "1", "art.png"]) {
        return; // no ffmpeg
    }
    assert!(ff(&[
        "-f", "lavfi", "-i", "sine=frequency=330:duration=20", "-i", "art.png", "-map", "0:a", "-map", "1:v", "-c:a", "libmp3lame", "-b:a", "128k", "-c:v", "png",
        "-id3v2_version", "3", "-metadata", "title=Утро", "-metadata", "artist=Polka Band", "-metadata", "album=Тестовый альбом", "-metadata", "date=2024",
        "-metadata", "track=1", "-metadata:s:v", "comment=Cover (front)", "01 Утро.mp3",
    ]));
    assert!(ff(&["-f", "lavfi", "-i", "sine=frequency=440:duration=20", "-metadata", "title=Вечер", "-metadata", "artist=Polka Band", "02 Вечер.flac"]));
    assert!(ff(&["-f", "lavfi", "-i", "sine=frequency=550:duration=20", "-c:a", "libopus", "03 Ночь.opus"]));
    std::fs::remove_file(dir.join("art.png")).unwrap();

    let audio = |h: &Harness<'static, App>| match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Audio(a)) => Some((a.index, a.cursor, a.tracks.len(), a.meta.title.clone(), a.cover.is_some(), a.playing(), a.pos().as_secs_f32())),
        _ => None,
    };
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let i = h.state().pane().entries.iter().position(|e| e.name.starts_with("01")).unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    for _ in 0..10 {
        settle(h);
        if audio(h).is_some_and(|a| a.4) {
            break;
        }
    }
    let launched = crate::launch::LAUNCHED.with(|l| l.borrow().clone());
    assert!(launched.iter().all(|c| !c.contains("music")), "handed to another app: {launched:?}");
    let a = audio(h).expect("music opens in the player");
    assert_eq!((a.0, a.2, a.3.as_deref(), a.4, a.5), (0, 3, Some("Утро"), true, true), "tags, cover, playing");
    shot(h, "58-music");
    // → seeks 5 s each; ✕ pauses and plays.
    press(h, &[Action::Right, Action::Right]);
    assert!(audio(h).unwrap().6 >= 10.0, "seeked: {}", audio(h).unwrap().6);
    press(h, &[Action::Accept]);
    assert!(!audio(h).unwrap().5);
    press(h, &[Action::Accept]);
    assert!(audio(h).unwrap().5);
    // ↓ ✕ plays another track of the playlist; the header follows.
    press(h, &[Action::Down, Action::Accept]);
    for _ in 0..5 {
        settle(h);
    }
    let a = audio(h).unwrap();
    assert_eq!((a.0, a.3.as_deref(), a.4), (1, Some("Вечер"), false));
    assert!(h.state().cur_tab().viewer.as_ref().unwrap().name.starts_with("02"));
    shot(h, "59-music-no-cover");
    // R2: next (Opus goes through ffmpeg).
    press(h, &[Action::RightTrigger]);
    for _ in 0..5 {
        settle(h);
    }
    let a = audio(h).unwrap();
    assert_eq!(a.0, 2);
    assert!(a.5 && a.6 > 0.0, "opus plays: {a:?}");
    shot(h, "60-music-opus");
    // □ tap: repeat; □ + ↑: volume (and no repeat change).
    let rep = h.state().cfg.viewer.repeat;
    press(h, &[Action::Select]);
    assert_eq!(h.state().cfg.viewer.repeat, (rep + 1) % 3);
    let vol = h.state().cfg.viewer.volume;
    h.state_mut().handle(Input::Press(Action::Select));
    h.state_mut().handle(Input::Press(Action::Up));
    h.state_mut().handle(Input::Release(Action::Select));
    settle(h);
    assert!((h.state().cfg.viewer.volume - (vol + 0.05).min(1.0)).abs() < 1e-3);
    assert_eq!(h.state().cfg.viewer.repeat, (rep + 1) % 3);
    shot(h, "61-music-volume");
    // Space (keyboard) pauses.
    key(h, egui::Key::Space, egui::Modifiers::NONE);
    settle(h);
    assert!(!audio(h).unwrap().5, "space pauses");
    press(h, &[Action::Actions]);
    assert!(matches!(h.state().overlay, Some(Overlay::Menu(_))));
    shot(h, "62-music-menu");
    h.state_mut().overlay = None;
    // ○ closes on the last track played; covers show as thumbnails.
    press(h, &[Action::Back]);
    assert!(h.state().cur_tab().viewer.is_none());
    assert!(h.state().pane().current().unwrap().name.starts_with("03"));
    h.state_mut().run(crate::app::Cmd::SetView(crate::config::ViewMode::Grid));
    for _ in 0..10 {
        settle(h);
    }
    shot(h, "63-music-thumbs");
    h.state_mut().run(crate::app::Cmd::SetView(crate::config::ViewMode::List));
}

/// Just the player (quick to rerun), also in a narrow window.
#[test]
#[ignore]
fn music_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    music(&mut h);
    h.set_size(egui::vec2(760.0, 700.0));
    h.state_mut().handle(Input::Press(Action::Accept));
    h.state_mut().handle(Input::Release(Action::Accept));
    for _ in 0..5 {
        settle(&mut h);
    }
    shot(&mut h, "64-music-narrow");
}

fn video(h: &mut Harness<'static, App>) {
    use crate::app::Overlay;
    use crate::vfs::Location;
    use crate::viewer::Content;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("video");
    std::fs::create_dir_all(&dir).unwrap();
    let ff = |args: &[&str]| std::process::Command::new("ffmpeg").args(["-v", "quiet", "-y"]).args(args).current_dir(&dir).status().is_ok_and(|s| s.success());
    std::fs::write(dir.join("s.srt"), "1\n00:00:00,000 --> 00:00:30,000\nПривет из субтитров\n").unwrap();
    if !ff(&[
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25:duration=30", "-f", "lavfi", "-i", "sine=f=440:d=30", "-f", "lavfi", "-i", "sine=f=660:d=30",
        "-i", "s.srt", "-map", "0", "-map", "1", "-map", "2", "-map", "3", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac",
        "-c:s", "srt", "-metadata:s:a:0", "language=eng", "-metadata:s:a:1", "language=rus", "-metadata:s:s:0", "language=rus", "Фильм.mkv",
    ]) {
        return; // no ffmpeg
    }
    std::fs::remove_file(dir.join("s.srt")).unwrap();
    assert!(ff(&["-f", "lavfi", "-i", "smptebars=size=480x360:rate=25:duration=5", "-c:v", "libx264", "-pix_fmt", "yuv420p", "Клип.mp4"]));

    let vid = |h: &Harness<'static, App>| match h.state().cur_tab().viewer.as_ref().map(|v| &v.content) {
        Some(Content::Video(m)) => Some((m.index, m.texture.is_some(), m.playing(), m.pos(), m.full, m.subs.clone(), m.audio_track)),
        _ => None,
    };
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let i = h.state().pane().entries.iter().position(|e| e.name == "Фильм.mkv").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    for _ in 0..10 {
        settle(h);
        if vid(h).is_some_and(|v| v.1) {
            break;
        }
    }
    let launched = crate::launch::LAUNCHED.with(|l| l.borrow().clone());
    assert!(launched.iter().all(|c| !c.contains("video")), "handed to another app: {launched:?}");
    let v = vid(h).expect("video opens in the player");
    assert!(v.1 && v.2, "frames and playing: {v:?}");
    assert!(matches!(v.5, crate::viewer::video::Subs::Inside(_)), "its subtitles are on");
    shot(h, "65-video");
    // → seeks 10 s; ✕ pauses.
    let before = vid(h).unwrap().3;
    press(h, &[Action::Right]);
    assert!(vid(h).unwrap().3 >= before + 9.0, "seeked: {:?}", vid(h));
    press(h, &[Action::Accept]);
    for _ in 0..3 {
        settle(h);
    }
    assert!(!vid(h).unwrap().2);
    shot(h, "66-video-paused");
    // View: full screen without the app around it; ○ leaves it.
    press(h, &[Action::View]);
    assert!(vid(h).unwrap().4 && h.state().video_full());
    shot(h, "67-video-full");
    // Playing, controls due to hide, the pointer resting on the volume
    // slider: they stay, frame after frame (they used to flicker).
    press(h, &[Action::Accept]);
    let screen = h.state().ctx.content_rect();
    let on_volume = egui::pos2(screen.max.x - 18.0 - 28.0 - 16.0 - 45.0, screen.max.y - 80.0 + 58.0);
    h.event(egui::Event::PointerMoved(on_volume));
    // (Moving shows them for a while: that settles first.)
    h.step();
    h.step();
    let tab = h.state().tab;
    if let Some(Content::Video(m)) = h.state_mut().tabs[tab].viewer.as_mut().map(|v| &mut v.content) {
        m.controls_until = std::time::Instant::now() - std::time::Duration::from_secs(1);
    }
    for _ in 0..8 {
        h.step();
        assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::None, "the controls hid under the pointer");
    }
    press(h, &[Action::Accept]);
    press(h, &[Action::Back]);
    assert!(!vid(h).unwrap().4 && h.state().cur_tab().viewer.is_some());
    // □ tap: subtitles off; the menu offers the audio track.
    press(h, &[Action::Select]);
    assert_eq!(vid(h).unwrap().5, crate::viewer::video::Subs::Off);
    h.state_mut().run(crate::app::Cmd::VideoAudio);
    assert_eq!(vid(h).unwrap().6, 1);
    press(h, &[Action::Actions]);
    assert!(matches!(h.state().overlay, Some(Overlay::Menu(_))));
    shot(h, "68-video-menu");
    h.state_mut().overlay = None;
    // R2: the next video of the folder.
    press(h, &[Action::RightTrigger]);
    for _ in 0..5 {
        settle(h);
    }
    assert!(h.state().cur_tab().viewer.as_ref().unwrap().name.starts_with("Клип"));
    shot(h, "69-video-next");
    press(h, &[Action::Back]);
    assert!(h.state().cur_tab().viewer.is_none());
    h.state_mut().run(crate::app::Cmd::SetView(crate::config::ViewMode::Grid));
    for _ in 0..15 {
        settle(h);
    }
    shot(h, "70-video-thumbs");
    h.state_mut().run(crate::app::Cmd::SetView(crate::config::ViewMode::List));
}

/// Just the video player (quick to rerun).
#[test]
#[ignore]
fn video_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    video(&mut h);
}

fn replacing(h: &mut Harness<'static, App>) {
    use crate::app::Overlay;
    use crate::vfs::Location;
    use egui::{Key, Modifiers};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("repl");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("list.txt"), "кот кошка кот\nдата 2024-10-14\nдата 2025-01-02\n").unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let i = h.state().pane().entries.iter().position(|e| e.name == "list.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    let text = |h: &mut Harness<'static, App>| h.state_mut().text_mut().unwrap().doc.text.clone();
    // Keyboard: Ctrl+F the word (the first match gets selected), Ctrl+H,
    // the replacement, Enter replaces that match; Ctrl+Enter the rest.
    key(h, Key::F, Modifiers::COMMAND);
    type_text(h, "кот");
    key(h, Key::H, Modifiers::COMMAND);
    type_text(h, "пёс");
    settle(h);
    shot(h, "71-replace-bar");
    key(h, Key::Enter, Modifiers::NONE);
    assert_eq!(text(h), "пёс кошка кот\nдата 2024-10-14\nдата 2025-01-02\n");
    key(h, Key::Enter, Modifiers::COMMAND);
    assert_eq!(text(h), "пёс кошка пёс\nдата 2024-10-14\nдата 2025-01-02\n");
    h.state_mut().text_mut().unwrap().undo();
    assert!(text(h).ends_with("кот\nдата 2024-10-14\nдата 2025-01-02\n"));
    // Gamepad: a regex with groups through the △ menu and the on-screen keyboard.
    {
        let t = h.state_mut().text_mut().unwrap();
        let f = t.ed.find.as_mut().unwrap();
        f.regex = true;
        f.query = r"(\d{4})-(\d\d)-(\d\d)".into();
        f.typing = false;
        f.typing_replace = false;
        // The keyboard starts from the last replacement; begin afresh.
        f.replace = Some(String::new());
        t.find_refresh();
    }
    press(h, &[Action::Actions]);
    menu_pick(h, crate::i18n::tr("Replace with…"));
    assert!(matches!(h.state().overlay, Some(Overlay::Osk(_))));
    if let Some(Overlay::Osk(k)) = h.state_mut().overlay.as_mut() {
        k.insert("$3.$2.$1");
    }
    h.state_mut().osk_submit();
    settle(h);
    press(h, &[Action::Actions]);
    shot(h, "72-replace-menu");
    menu_pick(h, crate::i18n::tr("Replace all"));
    assert!(text(h).contains("дата 14.10.2024\nдата 02.01.2025"), "{}", text(h));
    settle(h);
    shot(h, "73-replaced");
    // Close without saving.
    h.state_mut().text_mut().unwrap().ed.dirty = false;
    press(h, &[Action::Back, Action::Back]);
    assert!(h.state().cur_tab().viewer.is_none());
}

/// Just replacing (quick to rerun).
#[test]
#[ignore]
fn replace_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    replacing(&mut h);
}

/// Markdown pictures from the internet (a local server stands in for it),
/// inline `data:` pictures, and the switch that keeps them all offline.
fn web_pictures(h: &mut Harness<'static, App>) {
    use crate::vfs::Location;
    use std::io::{BufRead, BufReader, Write};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("web");
    let _ = std::fs::remove_dir_all(crate::vfs::temp_root().join("web-images"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_fn(320, 160, |x, y| image::Rgba([40 + (x % 64) as u8 * 3, 120, 200 - (y % 50) as u8 * 2, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let badge = r##"<svg xmlns="http://www.w3.org/2000/svg" width="110" height="20"><rect width="60" height="20" fill="#555"/><rect x="60" width="50" height="20" fill="#4c1"/><g fill="#fff" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11"><text x="8" y="14">build</text><text x="66" y="14">passing</text></g></svg>"##;
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let hits = hits.clone();
        std::thread::spawn(move || {
            for s in server.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                while r.read_line(&mut String::new()).map(|n| n > 2).unwrap_or(false) {}
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let path = line.split(' ').nth(1).unwrap_or("").split('?').next().unwrap_or("").to_string();
                // No extension in the URL: the type comes from the header.
                let (status, ct, body): (&str, &str, Vec<u8>) = match path.as_str() {
                    "/shot" => ("200 OK", "image/png", png.clone()),
                    "/badge" => ("200 OK", "image/svg+xml;charset=utf-8", badge.as_bytes().to_vec()),
                    _ => ("404 Not Found", "text/plain", b"nope".to_vec()),
                };
                let mut s = s;
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
            }
        });
    }
    use base64::Engine;
    let mut dot = Vec::new();
    image::RgbaImage::from_pixel(48, 48, image::Rgba([230, 80, 160, 255])).write_to(&mut std::io::Cursor::new(&mut dot), image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(&dot);
    let base = format!("http://127.0.0.1:{port}");
    std::fs::write(
        dir.join("README.md"),
        format!("# Сеть\n\n[![build]({base}/badge)](https://example.org) [![tests]({base}/badge?2)](#)\n\nСкриншот:\n\n![скрин]({base}/shot)\n\n![нет такой]({base}/missing.png)\n\nВстроенная: ![точка](data:image/png;base64,{data})\n"),
    )
    .unwrap();
    std::fs::write(dir.join("offline.md"), format!("# Без сети\n\n![скрин]({base}/shot?offline)\n")).unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let open = |h: &mut Harness<'static, App>, name: &str| {
        let i = h.state().pane().entries.iter().position(|e| e.name == name).unwrap();
        h.state_mut().pane_mut().set_cursor(i);
        press(h, &[Action::Accept]);
    };
    open(h, "README.md");
    for _ in 0..10 {
        settle(h);
    }
    shot(h, "81-md-web-pictures");
    assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 4, "pictures not fetched");
    let cached: Vec<String> = std::fs::read_dir(crate::vfs::temp_root().join("web-images")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(cached.iter().filter(|n| n.ends_with(".svg")).count() >= 2, "{cached:?}");
    assert!(cached.iter().any(|n| n.ends_with(".png")), "{cached:?}");
    assert!(!cached.iter().any(|n| n.ends_with(".part")), "{cached:?}");
    // Switched off: nothing is asked for.
    press(h, &[Action::Back]);
    h.state_mut().cfg.viewer.web_images = false;
    let before = hits.load(std::sync::atomic::Ordering::SeqCst);
    open(h, "offline.md");
    for _ in 0..4 {
        settle(h);
    }
    shot(h, "82-md-web-off");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before, "fetched while switched off");
    h.state_mut().cfg.viewer.web_images = true;
    press(h, &[Action::Back]);
}

#[test]
#[ignore]
fn web_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    web_pictures(&mut h);
}

/// Disk usage: opened from the sidebar, measured, walked with the pad in
/// every mode, shown in the panel, measured again.
fn disk_usage(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay};
    use crate::usage::{Mode, State};
    use crate::vfs::Location;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("usage");
    let _ = std::fs::remove_dir_all(&dir);
    let file = |p: &str, kib: usize| {
        let p = dir.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![7u8; kib * 1024]).unwrap();
    };
    file("Games/Townfall/data.pak", 6000);
    file("Games/Townfall/game.exe", 900);
    file("Games/Other/level1.bin", 1500);
    file("Games/Other/level2.bin", 1200);
    file("Music/album/01.flac", 1800);
    file("Music/album/02.flac", 1600);
    file("Music/single.mp3", 500);
    file("Photos/2025/a.jpg", 700);
    file("Photos/2025/b.jpg", 650);
    file("Photos/2026/c.jpg", 900);
    for i in 0..80 {
        file(&format!("Code/src/m{i:02}.rs"), 4 + i % 9);
    }
    file("video.mkv", 2500);
    file("notes.txt", 8);
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().cfg.usage_mode = 0;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    // From the sidebar's Tools section.
    h.state_mut().focus_sidebar();
    let i = h.state().side_items.iter().position(|s| s.usage).expect("no disk usage entry");
    h.state_mut().side_cursor = i;
    press(h, &[Action::Accept]);
    assert!(h.state().usage_shown());
    // The sidebar keeps the focus; → goes over to the page.
    assert!(h.state().focus == crate::app::Focus::Sidebar);
    press(h, &[Action::Right]);
    shot(h, "90-usage-start");
    let this = match &h.state().cur_tab().usage.as_ref().unwrap().state {
        State::Start { targets, .. } => targets.iter().position(|t| t.path == dir).expect("this folder not offered"),
        _ => panic!("not the start screen"),
    };
    for _ in 0..this {
        press(h, &[Action::Down]);
    }
    press(h, &[Action::Accept]);
    let ready = |h: &mut Harness<'static, App>| {
        for _ in 0..100 {
            settle(h);
            if h.state().cur_tab().usage.as_ref().unwrap().ready().is_some() {
                return;
            }
        }
        panic!("measuring never finished");
    };
    ready(h);
    let sel = |h: &Harness<'static, App>| {
        let r = h.state().cur_tab().usage.as_ref().unwrap().ready().unwrap();
        (r.tree.names(&r.center).join("/"), r.tree.names(&r.sel).join("/"))
    };
    {
        let r = h.state().cur_tab().usage.as_ref().unwrap().ready().unwrap();
        assert!(r.tree.size >= 18_000 * 1024, "{}", r.tree.size);
        assert_eq!(r.tree.children[0].name.as_ref(), "Games");
        let code = r.tree.children.iter().find(|c| c.name.as_ref() == "Code").unwrap();
        assert_eq!(code.files, 80);
    }
    assert_eq!(sel(h), (String::new(), "Games".into()));
    shot(h, "91-usage-rings");
    // Around the ring, out a ring, into a folder and back.
    press(h, &[Action::Right]);
    assert_eq!(sel(h).1, "Music");
    press(h, &[Action::Left, Action::Down]);
    assert_eq!(sel(h).1, "Games/Townfall");
    press(h, &[Action::Down]);
    assert_eq!(sel(h).1, "Games/Townfall/data.pak");
    shot(h, "92-usage-rings-deep");
    press(h, &[Action::Up, Action::Up, Action::Accept]);
    assert_eq!(sel(h), ("Games".into(), "Games/Townfall".into()));
    shot(h, "93-usage-rings-zoomed");
    press(h, &[Action::Back]);
    assert_eq!(sel(h), (String::new(), "Games".into()));
    // The mouse: a click picks a piece, a double click opens it.
    {
        let (c, r0, wr): (egui::Pos2, f32, f32) = h.ctx.data(|d| d.get_temp(egui::Id::new("usage-rings-geometry"))).expect("rings not drawn");
        // The middle of Music (the second biggest), on the first ring.
        let (f0, f1) = {
            let r = h.state().cur_tab().usage.as_ref().unwrap().ready().unwrap();
            crate::usage::frac_of(r.center_node(), &[1])
        };
        let a = ((f0 + f1) / 2.0) as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let at = c + egui::vec2(a.cos(), a.sin()) * (r0 + wr / 2.0);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        settle(h);
        assert_eq!(sel(h), (String::new(), "Music".into()), "one click only picks");
        for pressed in [true, false, true, false] {
            h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        }
        h.step();
        settle(h);
        assert_eq!(sel(h).0, "Music", "a double click opens");
        press(h, &[Action::Back]);
        h.hover_at(egui::pos2(5.0, 600.0));
        press(h, &[Action::Left]);
        assert_eq!(sel(h), (String::new(), "Games".into()));
    }
    // □: tiles, then the list.
    press(h, &[Action::Select]);
    assert_eq!(h.state().cur_tab().usage.as_ref().unwrap().mode, Mode::Tiles);
    press(h, &[Action::Down]);
    shot(h, "94-usage-tiles");
    press(h, &[Action::Select]);
    assert_eq!(h.state().cfg.usage_mode, Mode::List.index());
    assert_eq!(sel(h).1, "Games", "the list shows one level");
    press(h, &[Action::Down, Action::Down]);
    assert_eq!(sel(h).1, "video.mkv");
    press(h, &[Action::Down, Action::Down]);
    assert_eq!(sel(h).1, "Code");
    press(h, &[Action::Right]);
    assert_eq!(sel(h).0, "Code");
    shot(h, "95-usage-list-code");
    press(h, &[Action::Left]);
    shot(h, "96-usage-list");
    // △: show the picked folder in the panel; the page waits in the tab.
    press(h, &[Action::Actions]);
    assert!(matches!(&h.state().overlay, Some(Overlay::Menu(_))));
    shot(h, "97-usage-menu");
    press(h, &[Action::Accept]);
    assert!(!h.state().usage_shown());
    assert_eq!(h.state().pane().loc, Location::Local(dir.join("Code")));
    h.state_mut().focus_sidebar();
    h.state_mut().side_cursor = i;
    press(h, &[Action::Accept]);
    assert!(h.state().usage_shown());
    assert_eq!(sel(h).1, "Code", "the result was kept");
    // Measured again: still in the same place; a new file shows up. (The
    // first → only leaves the sidebar.)
    press(h, &[Action::Right, Action::Right]);
    file("Code/big.dat", 9000);
    h.state_mut().run(Cmd::UsageRescan);
    ready(h);
    assert_eq!(sel(h).0, "Code");
    assert_eq!(sel(h).1, "Code/src", "the pick is found again by name");
    {
        let r = h.state().cur_tab().usage.as_ref().unwrap().ready().unwrap();
        assert_eq!(r.tree.children[0].name.as_ref(), "Code");
        assert_eq!(r.center_node().children[0].name.as_ref(), "big.dat");
    }
    h.state_mut().run(Cmd::UsageMode(0));
    shot(h, "98-usage-rings-rescanned");
    // Back to the top, the start screen, closed.
    press(h, &[Action::Back, Action::Back]);
    assert!(matches!(h.state().cur_tab().usage.as_ref().unwrap().state, State::Start { .. }));
    press(h, &[Action::Back]);
    assert!(h.state().cur_tab().usage.is_none());
}

#[test]
#[ignore]
fn usage_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    disk_usage(&mut h);
}

/// The light look: switched from the settings with the pad, then the main
/// screens drawn in it.
fn light_look(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay, row};
    use crate::vfs::Location;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    std::fs::write(dir.join("light.rs"), "// A comment\nfn main() {\n    let x = \"text\";\n    println!(\"{x} {}\", 42);\n}\n").unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    h.state_mut().run(Cmd::Settings);
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::APPEARANCE;
    }
    press(h, &[Action::Right, Action::Right]);
    assert_eq!(h.state().cfg.appearance, crate::appearance::LIGHT);
    settle(h);
    assert!(!(h.state().ctx.theme() == egui::Theme::Dark), "still dark");
    shot(h, "a0-light-settings");
    press(h, &[Action::Back]);
    shot(h, "a1-light-list");
    h.state_mut().pane_mut().selected.insert("note.txt".into());
    press(h, &[Action::Actions]);
    shot(h, "a2-light-menu");
    press(h, &[Action::Back]);
    h.state_mut().pane_mut().selected.clear();
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::Grid));
    settle(h);
    shot(h, "a3-light-grid");
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    h.state_mut().pane_mut().reload(&cfg);
    settle(h);
    let i = h.state().pane().entries.iter().position(|e| e.name == "light.rs").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    settle(h);
    shot(h, "a4-light-text");
    press(h, &[Action::Back]);
    h.state_mut().run(Cmd::UsageOpen(Some(dir.clone())));
    for _ in 0..50 {
        settle(h);
        if h.state().cur_tab().usage.as_ref().unwrap().ready().is_some() {
            break;
        }
    }
    shot(h, "a5-light-usage");
    h.state_mut().run(Cmd::UsageMode(1));
    settle(h);
    shot(h, "a6-light-tiles");
    h.state_mut().run(Cmd::UsageClose);
    h.state_mut().run(Cmd::UsageMode(0));
    // Back to dark from the keyboard-free path: a click on the segment.
    h.state_mut().cfg.appearance = crate::appearance::DARK;
    h.state_mut().apply_appearance();
    settle(h);
    assert!((h.state().ctx.theme() == egui::Theme::Dark));
}

#[test]
#[ignore]
fn light_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    light_look(&mut h);
}

/// The mouse: a selection rectangle, a click on empty space, dragging onto
/// a folder (a move), onto an archive with Ctrl (a copy into it) and into
/// the other pane.
fn mouse(h: &mut Harness<'static, App>) {
    use crate::app::Cmd;
    use crate::i18n::tr;
    use crate::vfs::Location;
    use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("mouse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("A")).unwrap();
    std::fs::create_dir_all(dir.join("B")).unwrap();
    std::fs::write(dir.join("B/keep.txt"), "so B isn't empty\n").unwrap();
    for i in 1..=6 {
        std::fs::write(dir.join(format!("f{i}.txt")), format!("file {i}\n")).unwrap();
    }
    std::fs::write(dir.join("inner.txt"), "in the box\n").unwrap();
    let st = std::process::Command::new("tar").arg("czf").arg(dir.join("box.tar.gz")).arg("-C").arg(&dir).arg("inner.txt").status().unwrap();
    assert!(st.success());
    std::fs::remove_file(dir.join("inner.txt")).unwrap();
    let mut cfg = h.state().cfg.clone();
    cfg.dual = false;
    cfg.view = crate::config::ViewMode::List;
    h.state_mut().cfg = cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let rect_of = |h: &Harness<'static, App>, name: &str| {
        let p = h.state().pane();
        let i = p.entries.iter().position(|e| e.name == name).unwrap_or_else(|| panic!("{name} not shown"));
        let (_, row, hot) = *p.drawn.iter().find(|d| d.0 == i).expect("not drawn");
        (row, hot)
    };
    let press_at = |h: &mut Harness<'static, App>, pos: Pos2, down: bool, m: Modifiers| {
        h.event(Event::PointerMoved(pos));
        h.event(Event::ModifiersChanged(m));
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: down, modifiers: m });
        h.step();
    };
    let move_to = |h: &mut Harness<'static, App>, pos: Pos2| {
        h.event(Event::PointerMoved(pos));
        h.step();
    };
    let (r2, _) = rect_of(h, "f2.txt");
    // A rectangle from the empty right side of f2 down to f4.
    let (r4, _) = rect_of(h, "f4.txt");
    let a = pos2(r2.max.x - 60.0, r2.center().y);
    let b = pos2(r4.max.x - 200.0, r4.center().y);
    press_at(h, a, true, Modifiers::NONE);
    move_to(h, a + vec2(10.0, 10.0));
    move_to(h, b);
    move_to(h, b);
    shot(h, "b0-band");
    press_at(h, b, false, Modifiers::NONE);
    let mut sel: Vec<String> = h.state().pane().selected.iter().cloned().collect();
    sel.sort();
    assert_eq!(sel, ["f2.txt", "f3.txt", "f4.txt"]);
    // All three picked up: a stack with a count; let go back in the same
    // folder nothing happens and the selection stays.
    let (_, hot3) = rect_of(h, "f3.txt");
    press_at(h, hot3.center(), true, Modifiers::NONE);
    move_to(h, hot3.center() + vec2(8.0, 8.0));
    move_to(h, hot3.center() + vec2(-200.0, 120.0));
    move_to(h, hot3.center() + vec2(-180.0, 140.0));
    assert_eq!(h.state().drag.as_ref().map(|d| d.shown.len()), Some(3));
    shot(h, "b0b-drag-three");
    press_at(h, hot3.center() + vec2(-180.0, 140.0), false, Modifiers::NONE);
    settle(h);
    assert_eq!(h.state().pane().selected.len(), 3);
    assert!(!h.state().pane().cursor_hidden);
    // A click on empty space below the entries clears it.
    let (last, _) = rect_of(h, "f6.txt");
    let empty = pos2(last.center().x, last.max.y + 60.0);
    press_at(h, empty, true, Modifiers::NONE);
    press_at(h, empty, false, Modifiers::NONE);
    settle(h);
    assert!(h.state().pane().selected.is_empty(), "click on empty space");

    // f1 by its name onto folder A: moved (same disk).
    let (_, hot1) = rect_of(h, "f1.txt");
    let (ra, _) = rect_of(h, "A");
    press_at(h, hot1.center(), true, Modifiers::NONE);
    move_to(h, hot1.center() + vec2(8.0, 8.0));
    // A quick swing sideways tilts the stack following the pointer.
    move_to(h, hot1.center() + vec2(300.0, 8.0));
    move_to(h, ra.center());
    assert!(h.state().drag.is_some(), "drag did not start");
    assert!(h.state().drag.as_ref().unwrap().angle.abs() > 0.02, "no sway: {}", h.state().drag.as_ref().unwrap().angle);
    shot(h, "b1-drag-onto-folder");
    move_to(h, ra.center());
    move_to(h, ra.center());
    shot(h, "b1b-drag-settled");
    press_at(h, ra.center(), false, Modifiers::NONE);
    // Let go: what to do? Moving comes first on the same disk.
    let menu = |h: &Harness<'static, App>| match &h.state().overlay {
        Some(crate::app::Overlay::Menu(m)) => (m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>(), m.cursor),
        _ => (Vec::new(), 0),
    };
    let (labels, cursor) = menu(h);
    assert_eq!(labels, [tr("Move here"), tr("Copy here"), tr("Create link here"), tr("Cancel")], "{labels:?}");
    assert_eq!(cursor, 0);
    shot(h, "b1c-drop-menu");
    press(h, &[Action::Accept]);
    wait_jobs(h);
    assert!(dir.join("A/f1.txt").exists() && !dir.join("f1.txt").exists(), "not moved");
    assert!(h.state().drag.is_none());
    // No cursor jumping to the top in the moved item's place; the pad's
    // first press only brings the cursor back.
    assert!(h.state().pane().cursor_hidden && h.state().pane().selected.is_empty());
    let before = h.state().pane().cursor;
    press(h, &[Action::Down]);
    assert!(!h.state().pane().cursor_hidden);
    assert_eq!(h.state().pane().cursor, before);
    // f5 with Ctrl onto the archive: copied into it.
    settle(h);
    let (_, hot5) = rect_of(h, "f5.txt");
    let (rbox, _) = rect_of(h, "box.tar.gz");
    press_at(h, hot5.center(), true, Modifiers::NONE);
    move_to(h, hot5.center() + vec2(8.0, 8.0));
    move_to(h, rbox.center());
    h.event(Event::ModifiersChanged(Modifiers::COMMAND));
    move_to(h, rbox.center());
    shot(h, "b2-drag-onto-archive");
    press_at(h, rbox.center(), false, Modifiers::COMMAND);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    wait_jobs(h);
    assert!(dir.join("f5.txt").exists(), "a copy keeps the file");
    let listed = std::process::Command::new("tar").arg("tzf").arg(dir.join("box.tar.gz")).output().unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains("f5.txt") && listed.contains("inner.txt"), "{listed}");
    // Two panes: f6 from the left one onto the right one (folder B).
    h.state_mut().run(Cmd::ToggleDual);
    settle(h);
    {
        let cfg = h.state().cfg.clone();
        let t = h.state_mut().tab;
        h.state_mut().tabs[t].panes[1].navigate(Location::Local(dir.join("B")), None, &cfg);
        h.state_mut().tabs[t].active = 0;
    }
    settle(h);
    // A click in the left pane reaches it (the right pane's background
    // once covered it).
    let (r3, _) = rect_of(h, "f3.txt");
    let t = h.state().tab;
    h.state_mut().tabs[t].active = 1;
    press_at(h, r3.center(), true, Modifiers::NONE);
    press_at(h, r3.center(), false, Modifiers::NONE);
    assert_eq!(h.state().cur_tab().active, 0);
    assert_eq!(h.state().pane().current().map(|e| e.name.as_str()), Some("f3.txt"));
    let (_, hot6) = rect_of(h, "f6.txt");
    // The right half of the window, below the tabs.
    let right = pos2(900.0, 400.0);
    press_at(h, hot6.center(), true, Modifiers::NONE);
    move_to(h, hot6.center() + vec2(8.0, 8.0));
    move_to(h, right);
    move_to(h, right);
    shot(h, "b3-drag-to-other-pane");
    press_at(h, right, false, Modifiers::NONE);
    assert_eq!(menu(h).0.first().map(String::as_str), Some(tr("Move here")));
    press(h, &[Action::Accept]);
    wait_jobs(h);
    assert!(dir.join("B/f6.txt").exists() && !dir.join("f6.txt").exists(), "not moved to the other pane");
    h.state_mut().run(Cmd::ToggleDual);
    settle(h);
    // Shift+click: everything from the clicked one to the next.
    let click = |h: &mut Harness<'static, App>, pos: Pos2, button: PointerButton, m: Modifiers| {
        h.event(Event::PointerMoved(pos));
        h.event(Event::ModifiersChanged(m));
        h.event(Event::PointerButton { pos, button, pressed: true, modifiers: m });
        h.step();
        h.event(Event::PointerButton { pos, button, pressed: false, modifiers: m });
        h.step();
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.step();
    };
    let (r2, _) = rect_of(h, "f2.txt");
    let (r4, _) = rect_of(h, "f4.txt");
    click(h, r2.center(), PointerButton::Primary, Modifiers::NONE);
    click(h, r4.center(), PointerButton::Primary, Modifiers::SHIFT);
    let mut sel: Vec<String> = h.state().pane().selected.iter().cloned().collect();
    sel.sort();
    assert_eq!(sel, ["f2.txt", "f3.txt", "f4.txt"]);
    // Middle click: the folder in a new tab, this one stays.
    let (tabs, here) = (h.state().tabs.len(), h.state().tab);
    let (ra, _) = rect_of(h, "A");
    click(h, ra.center(), PointerButton::Middle, Modifiers::NONE);
    assert_eq!((h.state().tabs.len(), h.state().tab), (tabs + 1, here));
    assert_eq!(h.state().tabs[tabs].panes[0].loc, Location::Local(dir.join("A")));
    // A double click: kittest runs a frame per queued event and its clock
    // jumps between frames, so the four go into one frame directly (well
    // after the middle click, or egui counts a triple click).
    settle(h);
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: ra.center(), button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
    settle(h);
    assert_eq!(h.state().pane().loc, Location::Local(dir.join("A")), "double click opens");
    click(h, ra.center(), PointerButton::Extra1, Modifiers::NONE);
    settle(h);
    assert_eq!(h.state().pane().loc, Location::Local(dir.clone()), "back button");
    // Grid: a rectangle from the empty space after the last item.
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::Grid));
    settle(h);
    h.state_mut().pane_mut().selected.clear();
    let (first, _) = rect_of(h, "A");
    let n = h.state().pane().entries.len();
    let last_name = h.state().pane().entries[n - 1].name.clone();
    let (last, _) = rect_of(h, &last_name);
    let from = pos2(1080.0, last.max.y + 40.0);
    press_at(h, from, true, Modifiers::NONE);
    move_to(h, from - vec2(10.0, 10.0));
    move_to(h, first.center());
    move_to(h, first.center());
    shot(h, "b4-grid-band");
    press_at(h, first.center(), false, Modifiers::NONE);
    assert_eq!(h.state().pane().selected.len(), n, "the band covers everything from the first item to after the last");
    h.state_mut().run(Cmd::SetView(crate::config::ViewMode::List));
    settle(h);
    h.state_mut().pane_mut().selected.clear();

    // Dropped on a script: it can run with the files.
    std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho \"$@\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().reload(&cfg);
    settle(h);
    let (_, hot2) = rect_of(h, "f2.txt");
    let (rs, _) = rect_of(h, "run.sh");
    press_at(h, hot2.center(), true, Modifiers::NONE);
    move_to(h, hot2.center() + vec2(8.0, 8.0));
    move_to(h, rs.center());
    move_to(h, rs.center());
    press_at(h, rs.center(), false, Modifiers::NONE);
    let (labels, _) = menu(h);
    let run = labels.iter().position(|l| l.starts_with(tr("Open with")) && l.ends_with("run.sh")).unwrap_or_else(|| panic!("{labels:?}"));
    shot(h, "b5-drop-on-script");
    for _ in 0..run {
        press(h, &[Action::Down]);
    }
    crate::launch::LAUNCHED.with(|l| l.borrow_mut().clear());
    press(h, &[Action::Accept]);
    let launched = crate::launch::LAUNCHED.with(|l| l.borrow().clone());
    assert!(launched.iter().any(|c| c.contains("run.sh") && c.contains("f2.txt")), "{launched:?}");
    assert!(dir.join("f2.txt").exists(), "running doesn't move anything");

    // Volume sets: folded, a rectangle takes them whole.
    for i in 1..=3 {
        std::fs::write(dir.join(format!("vol.zip.{i:03}")), vec![1u8; 100]).unwrap();
    }
    h.state_mut().cfg.collapse_volumes = true;
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().refilter_with(None, &cfg);
    h.state_mut().pane_mut().reload(&cfg);
    settle(h);
    let folded = h.state().pane().entries.iter().filter(|e| e.name.starts_with("vol.zip")).count();
    assert_eq!(folded, 1, "the set is shown as one");
    let rep = h.state().pane().entries.iter().find(|e| e.name.starts_with("vol.zip")).unwrap().name.clone();
    let (rv, _) = rect_of(h, &rep);
    // From the row above, across the folded set (under the pointer a set
    // unfolds, but not while a rectangle is drawn).
    h.hover_at(pos2(1000.0, 620.0));
    settle(h);
    let from = pos2(rv.max.x - 40.0, rv.min.y - 10.0);
    press_at(h, from, true, Modifiers::NONE);
    move_to(h, from + vec2(-10.0, 10.0));
    move_to(h, pos2(rv.max.x - 60.0, rv.max.y - 3.0));
    move_to(h, pos2(rv.max.x - 60.0, rv.max.y - 3.0));
    press_at(h, pos2(rv.max.x - 60.0, rv.max.y - 3.0), false, Modifiers::NONE);
    settle(h);
    for i in 1..=3 {
        assert!(h.state().pane().selected.contains(&format!("vol.zip.{i:03}")), "{:?}", h.state().pane().selected);
    }
    // Opened, only what the rectangle covers; and it stays open.
    h.state_mut().pane_mut().selected.clear();
    let key = h.state().pane().volumes.get(&rep).unwrap().key.clone();
    h.state_mut().pane_mut().set_expanded(vec![key]);
    settle(h);
    let (r1, _) = rect_of(h, "vol.zip.001");
    let (r2, _) = rect_of(h, "vol.zip.002");
    let from = pos2(r1.max.x - 40.0, r1.min.y + 3.0);
    let to = pos2(r2.max.x - 60.0, r2.center().y);
    press_at(h, from, true, Modifiers::NONE);
    move_to(h, from + vec2(-10.0, 10.0));
    move_to(h, to);
    move_to(h, to);
    press_at(h, to, false, Modifiers::NONE);
    h.hover_at(pos2(1000.0, 620.0));
    h.state_mut().pane_mut().set_cursor(0);
    settle(h);
    let mut sel: Vec<String> = h.state().pane().selected.iter().cloned().collect();
    sel.sort();
    assert_eq!(sel, ["vol.zip.001", "vol.zip.002"]);
    assert_eq!(h.state().pane().entries.iter().filter(|e| e.name.starts_with("vol.zip")).count(), 3, "a partly selected set stays open");
    shot(h, "b6-volume-set-part");
    h.state_mut().pane_mut().selected.clear();
}

#[test]
#[ignore]
fn mouse_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    mouse(&mut h);
}

/// The sidebar: drives (from a saved lsblk listing; udisks isn't really
/// called), any place's own icon, emptying the trash, the language.
fn sidebar_more(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay, row};
    use crate::i18n::tr;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let fixture = dir.join("lsblk.json");
    std::fs::write(
        &fixture,
        r#"{"blockdevices":[
          {"name":"sdz","path":"/dev/sdz","fstype":null,"mountpoints":[null],"rm":true,"hotplug":true,"type":"disk","rota":true,"size":32000000000,"children":[
            {"name":"sdz1","path":"/dev/sdz1","uuid":"1234-ABCD","label":"STICK","fstype":"exfat","mountpoints":[null],"rm":true,"hotplug":true,"type":"part","rota":true,"size":32000000000}]},
          {"name":"sdy","path":"/dev/sdy","fstype":null,"mountpoints":[null],"rm":false,"hotplug":false,"type":"disk","rota":true,"size":2000000000000,"children":[
            {"name":"sdy1","path":"/dev/sdy1","uuid":"9999","fstype":"ntfs","mountpoints":[null],"rm":false,"hotplug":false,"type":"part","rota":true,"size":2000000000000}]}
        ]}"#,
    )
    .unwrap();
    unsafe { std::env::set_var("POLKA_LSBLK", &fixture) };
    h.state_mut().overlay = None;
    h.state_mut().refresh_sidebar();
    let find = |h: &Harness<'static, App>, label: &str| h.state().side_items.iter().position(|s| s.label == label).unwrap_or_else(|| panic!("no {label} in the sidebar"));
    let stick = find(h, "STICK");
    let big = h.state().side_items.iter().position(|s| s.drive.as_ref().is_some_and(|d| d.dev == "/dev/sdy1")).unwrap();
    assert!(h.state().side_items[big].label.starts_with(tr("Drive")));
    // Drives come before the tools.
    let tools = h.state().side_items.iter().position(|s| s.usage).unwrap();
    assert!(stick < tools && h.state().side_items[stick].section == "Drives");
    h.state_mut().focus_sidebar();
    h.state_mut().side_cursor = stick;
    press(h, &[Action::Actions]);
    let labels = |h: &Harness<'static, App>| match &h.state().overlay {
        Some(Overlay::Menu(m)) => m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let menu = labels(h);
    assert!(menu.contains(&tr("Mount").to_string()) && menu.contains(&tr("Eject").to_string()) && menu.contains(&tr("Choose icon…").to_string()), "{menu:?}");
    shot(h, "c0-drive-menu");
    press(h, &[Action::Back]);
    // ✕ on an unmounted drive mounts it (through udisks).
    crate::drives::RAN.lock().unwrap().clear();
    press(h, &[Action::Accept]);
    for _ in 0..20 {
        settle(h);
        if !crate::drives::RAN.lock().unwrap().is_empty() {
            break;
        }
    }
    assert_eq!(crate::drives::RAN.lock().unwrap().first().map(String::as_str), Some("udisksctl mount -b /dev/sdz1"));
    h.state_mut().run(Cmd::DriveEject(stick));
    settle(h);
    assert!(crate::drives::RAN.lock().unwrap().iter().any(|c| c == "udisksctl power-off -b /dev/sdz"), "{:?}", crate::drives::RAN.lock().unwrap());
    // Mounting opened it in the pane; back to the sidebar.
    assert!(h.state().focus == crate::app::Focus::Panes);
    h.state_mut().focus_sidebar();
    // The internal disk can't be ejected.
    h.state_mut().side_cursor = big;
    press(h, &[Action::Actions]);
    let menu = labels(h);
    assert!(menu.contains(&tr("Mount").to_string()) && !menu.contains(&tr("Eject").to_string()), "{menu:?}");
    press(h, &[Action::Back]);

    // Any place takes its own icon: Home here.
    let home = find(h, tr("Home"));
    h.state_mut().side_cursor = home;
    press(h, &[Action::Actions]);
    assert!(labels(h).contains(&tr("Choose icon…").to_string()));
    press(h, &[Action::Back]);
    let home_path = dirs::home_dir().unwrap();
    h.state_mut().run(Cmd::SetPinIcon(home_path.clone(), Some("folder-red".into())));
    assert_eq!(h.state().side_items[find(h, tr("Home"))].custom_icon.as_deref(), Some("folder-red"));
    // A drive's icon sticks to its UUID.
    h.state_mut().run(Cmd::SetPinIcon("dev:1234-ABCD".into(), Some("media-flash".into())));
    assert_eq!(h.state().side_items[find(h, "STICK")].custom_icon.as_deref(), Some("media-flash"));
    settle(h);
    shot(h, "c1-sidebar-drives-icons");
    h.state_mut().run(Cmd::SetPinIcon(home_path, None));
    h.state_mut().run(Cmd::SetPinIcon("dev:1234-ABCD".into(), None));

    // The trash: a full bin, emptied after asking.
    let trash = dirs::data_dir().unwrap().join("Trash");
    assert!(trash.starts_with(std::env::temp_dir()), "not the test's own trash: {trash:?}");
    for n in ["old.txt", "older.txt"] {
        std::fs::write(trash.join("files").join(n), "x").unwrap();
        std::fs::write(trash.join("info").join(format!("{n}.trashinfo")), "[Trash Info]\n").unwrap();
    }
    std::fs::create_dir_all(trash.join("files/dir/sub")).unwrap();
    h.state_mut().refresh_sidebar();
    let bin = find(h, tr("Trash"));
    assert_eq!(h.state().side_items[bin].icon[0], "user-trash-full");
    h.state_mut().side_cursor = bin;
    press(h, &[Action::Actions]);
    let menu = labels(h);
    let at = menu.iter().position(|l| l == tr("Empty trash…")).expect("no Empty trash");
    for _ in 0..at {
        press(h, &[Action::Down]);
    }
    press(h, &[Action::Accept]);
    shot(h, "c2-empty-trash-ask");
    press(h, &[Action::Accept]);
    for _ in 0..20 {
        settle(h);
        if std::fs::read_dir(trash.join("files")).unwrap().count() == 0 {
            break;
        }
    }
    assert_eq!(std::fs::read_dir(trash.join("files")).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(trash.join("info")).unwrap().count(), 0);
    settle(h);
    assert_eq!(h.state().side_items[find(h, tr("Trash"))].icon[0], "user-trash");

    // Operations under way show their progress.
    h.state_mut().jobs.push(std::sync::Arc::new(crate::ops::Job::shown(tr("Copying"), "Music", 1_400_000_000, 3_600_000_000, "album/03 - Track.flac", 20)));
    h.state_mut().jobs.push(std::sync::Arc::new(crate::ops::Job::shown(tr("Deleting"), "", 0, 0, "", 2)));
    settle(h);
    shot(h, "c7-progress");
    h.state_mut().jobs.clear();

    // Languages: from the settings, live.
    let before = h.state().cfg.language.clone();
    crate::i18n::set(Some("de"));
    h.state_mut().cfg.language = Some("de".into());
    h.state_mut().refresh_sidebar();
    h.state_mut().run(Cmd::Settings);
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::LANGUAGE;
    }
    press(h, &[Action::Right]);
    assert_eq!(h.state().cfg.language.as_deref(), Some("es"));
    assert_eq!(crate::i18n::current(), "es");
    settle(h);
    shot(h, "c3-settings-spanish");
    press(h, &[Action::Back]);
    h.state_mut().run(Cmd::Settings);
    settle(h);
    shot(h, "c3b-settings-spanish-again");
    press(h, &[Action::Back]);
    for code in ["zh", "ja", "ko", "uk", "fr", "de"] {
        crate::i18n::set(Some(code));
        h.state_mut().refresh_sidebar();
        settle(h);
        shot(h, &format!("c4-lang-{code}"));
    }
    // German runs long: the settings and a menu.
    h.state_mut().run(Cmd::Settings);
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::CLOSE;
    }
    h.state_mut().ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-reveal"), true));
    settle(h);
    shot(h, "c5-settings-de");
    press(h, &[Action::Back]);
    press(h, &[Action::Actions]);
    shot(h, "c6-menu-de");
    press(h, &[Action::Back]);
    h.state_mut().cfg.language = before.clone();
    crate::i18n::set(before.as_deref());
    h.state_mut().refresh_sidebar();
    unsafe { std::env::remove_var("POLKA_LSBLK") };
    h.state_mut().refresh_sidebar();
}

#[test]
#[ignore]
fn sidebar_screens() {
    // Switches the language and the drives for the whole process.
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    sidebar_more(&mut h);
}

/// What the current terminal shows (the screen, not the scrollback).
fn term_screen(h: &Harness<'static, App>) -> String {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line, Point};
    let t = h.state().cur_tab().term.as_deref().expect("terminal");
    let s = t.cur().expect("shell");
    let term = s.term.lock();
    let mut out = String::new();
    for l in 0..term.screen_lines() as i32 {
        let row: String = (0..term.columns()).map(|c| term.grid()[Point::new(Line(l), Column(c))].c).collect();
        out.push_str(&row);
        out.push('\n');
    }
    out
}

/// Steps until the terminal shows `what` (`n` times).
fn term_wait(h: &mut Harness<'static, App>, what: &str, n: usize) {
    for _ in 0..150 {
        h.step();
        if term_screen(h).matches(what).count() >= n {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let t = h.state().cur_tab().term.as_deref().unwrap();
    let s = t.cur().unwrap();
    let st = std::fs::read_to_string(format!("/proc/{}/status", s.pid)).unwrap_or_default();
    let cmd = std::fs::read_to_string(format!("/proc/{}/cmdline", s.pid)).unwrap_or_default();
    panic!("terminal never showed {what:?} x{n} (pid {} exited {} title {:?} cmd {cmd:?} {}):\n{}", s.pid, s.exited, s.title, st.lines().take(3).collect::<Vec<_>>().join(" "), term_screen(h));
}

/// The terminal page: typing, running from the pad, interrupting, history,
/// saved commands, quick keys for a program, choosing text to copy, paste,
/// its own tabs and settings, leaving it with a held ○, and exit.
fn terminal(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Focus, Overlay};
    use egui::{Key, Modifiers};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    // A plain shell with a plain prompt, whatever the user has.
    let sh = dir.join("test-shell.sh");
    std::fs::write(&sh, "#!/bin/sh\nexport PS1='$ ' HISTFILE=/dev/null\nexec bash --norc --noprofile\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("pager.txt"), (1..200).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
    h.state_mut().cfg.terminal.shell = Some(sh.display().to_string());
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir.clone()), None, &cfg);
    settle(h);
    // From the sidebar.
    let i = h.state().side_items.iter().position(|it| it.term).expect("terminal in the sidebar");
    h.state_mut().side_cursor = i;
    h.state_mut().focus = Focus::Sidebar;
    press(h, &[Action::Accept]);
    assert!(h.state().term_shown());
    // The sidebar keeps the focus; → goes over.
    assert!(h.state().focus == Focus::Sidebar);
    press(h, &[Action::Right]);
    assert!(h.state().focus == Focus::Panes);
    term_wait(h, "$ ", 1);
    // Keyboard.
    type_text(h, "echo hel''lo");
    key(h, Key::Enter, Modifiers::NONE);
    term_wait(h, "hello", 1);
    shot(h, "t0-terminal");
    // Holding a button: △ history, ⧉ saved commands, ✕ Enter.
    let hold = |h: &mut Harness<'static, App>, a: Action| {
        h.state_mut().handle(Input::Press(a));
        for _ in 0..16 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        h.state_mut().handle(Input::Release(a));
        settle(h);
    };
    // □ erases what's typed: one character, or the part marked with □ held
    // and the arrows.
    type_text(h, "echo abcdef");
    term_wait(h, "abcdef", 1);
    press(h, &[Action::Select]);
    term_wait(h, "echo abcde ", 1);
    h.state_mut().handle(Input::Press(Action::Select));
    press(h, &[Action::Left, Action::Left]);
    h.state_mut().handle(Input::Release(Action::Select));
    settle(h);
    assert!(h.state().cur_tab().term.as_ref().unwrap().cur().unwrap().term.lock().selection.is_some(), "nothing marked");
    shot(h, "t0b-marked");
    press(h, &[Action::Select]);
    hold(h, Action::Accept);
    term_wait(h, "\nabc ", 1);
    // ✕: the pad's keyboard types straight into the shell, keys as pressed;
    // the terminal makes room for it (a full screen keeps its prompt).
    h.state_mut().run(Cmd::TermRun("seq 1 60".into()));
    term_wait(h, "\n60", 1);
    let rows = h.state().cur_tab().term.as_ref().unwrap().grid.1;
    press(h, &[Action::Accept]);
    settle(h);
    let fewer = h.state().cur_tab().term.as_ref().unwrap().grid.1;
    assert!(fewer + 5 < rows, "{rows} rows → {fewer}");
    assert!(matches!(&h.state().overlay, Some(Overlay::Osk(k)) if k.live()));
    use crate::app::OskKey;
    for c in "echo pad-runx".chars() {
        h.state_mut().osk_key(if c == ' ' { OskKey::Space } else { OskKey::Char(c) });
    }
    term_wait(h, "echo pad-runx", 1);
    // □ on the keyboard erases at once; Shift gives the key's other symbol.
    press(h, &[Action::Select]);
    term_wait(h, "echo pad-run ", 1);
    press(h, &[Action::LeftTrigger]);
    if let Some(Overlay::Osk(k)) = h.state_mut().overlay.as_mut() {
        // English: Shift+1 is "!".
        (k.row, k.col, k.layout) = (0, 1, 0);
    }
    press(h, &[Action::Accept]);
    term_wait(h, "echo pad-run!", 1);
    shot(h, "t1-live-keyboard");
    press(h, &[Action::Select]);
    press(h, &[Action::Back]);
    assert!(h.state().overlay.is_none());
    hold(h, Action::Accept);
    term_wait(h, "\npad-run", 1);
    // Interrupt from the menu.
    h.state_mut().run(Cmd::TermRun("clear".into()));
    for _ in 0..50 {
        if term_screen(h).matches("$ ").count() == 1 {
            break;
        }
        settle(h);
    }
    h.state_mut().run(Cmd::TermRun("sleep 30".into()));
    term_wait(h, "sleep 30", 1);
    press(h, &[Action::Actions]);
    shot(h, "t2-menu");
    menu_pick(h, crate::i18n::tr("Interrupt (Ctrl+C)"));
    term_wait(h, "$ ", 2);
    // History: △ held; what was run from here comes back.
    hold(h, Action::Actions);
    let Some(Overlay::Menu(m)) = h.state().overlay.as_ref() else { panic!("no history") };
    assert_eq!(m.title, crate::i18n::tr("Command history"));
    shot(h, "t3-history");
    menu_pick(h, "sleep 30");
    menu_pick(h, crate::i18n::tr("Save"));
    assert_eq!(h.state().cfg.terminal.saved, vec!["sleep 30".to_string()]);
    // Saved commands (⧉ held): a new one from the keyboard, then run.
    hold(h, Action::View);
    let Some(Overlay::Menu(m)) = h.state().overlay.as_ref() else { panic!("no saved commands") };
    assert_eq!(m.title, crate::i18n::tr("Saved commands"));
    menu_pick(h, crate::i18n::tr("Save a new command…"));
    osk_type(h, "echo saved-one");
    press(h, &[Action::Menu]);
    h.state_mut().run(Cmd::TermSavedMenu);
    shot(h, "t4-saved");
    menu_pick(h, "echo saved-one");
    // Put in, not run: Enter is ✕ held.
    menu_pick(h, crate::i18n::tr("Insert"));
    assert!(term_screen(h).matches("saved-one").count() == 1);
    hold(h, Action::Accept);
    term_wait(h, "saved-one", 2);
    // Quick keys know the pager and quit it.
    h.state_mut().run(Cmd::TermRun("less pager.txt".into()));
    term_wait(h, "line 20", 1);
    press(h, &[Action::RightTrigger]);
    // A page on: the first line shown isn't line 1 any more.
    for _ in 0..100 {
        if !term_screen(h).starts_with("line 1 ") {
            break;
        }
        settle(h);
    }
    let paged = term_screen(h);
    assert!(!paged.starts_with("line 1 "), "not paged:\n{paged}");
    press(h, &[Action::View]);
    let Some(Overlay::TermKeys(q)) = h.state().overlay.as_ref() else { panic!("no quick keys") };
    assert!(q.title.contains("less"), "{}", q.title);
    let qi = q.keys.iter().position(|k| k.label == "q").unwrap();
    shot(h, "t5-quick-keys-less");
    if let Some(Overlay::TermKeys(q)) = h.state_mut().overlay.as_mut() {
        q.cursor = qi;
    }
    press(h, &[Action::Select]);
    assert!(h.state().overlay.is_none());
    term_wait(h, "$ ", 1);
    assert!(!term_screen(h).contains(paged.lines().next().unwrap()), "{}", term_screen(h));
    // Choosing text with the pad.
    h.state_mut().run(Cmd::TermRun("echo copy-me-please".into()));
    term_wait(h, "copy-me-please", 2);
    h.state_mut().run(Cmd::TermSelect);
    press(h, &[Action::Up, Action::Home, Action::Select, Action::End]);
    shot(h, "t6-select");
    press(h, &[Action::Accept]);
    assert_eq!(h.state().text_clip.as_deref().map(str::trim), Some("copy-me-please"));
    assert!(!h.state().cur_tab().term.as_ref().unwrap().selecting);
    // Paste (what was copied here: no system clipboard in tests).
    h.state_mut().text_clip = Some("echo pasted-text".into());
    h.state_mut().run(Cmd::TermPaste);
    hold(h, Action::Accept);
    term_wait(h, "pasted-text", 2);
    // Colours and box drawing.
    h.state_mut().run(Cmd::TermRun("printf '\\e[31mred \\e[1;32mbold-green \\e[44m blue-bg \\e[0m\\n┌──┬──┐\\n│ab│cd│\\n└──┴──┘\\n▁▂▃▄▅▆▇█ ⣿⡇⠁\\n'".into()));
    term_wait(h, "blue-bg", 2);
    shot(h, "t7-colours");
    // A real full-screen program, when there is one.
    for prog in crate::terminal::installed(&["htop", "btop"]) {
        h.state_mut().run(Cmd::TermRun(prog.into()));
        for _ in 0..40 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        settle(h);
        shot(h, &format!("t7-{prog}"));
        press(h, &[Action::View]);
        let Some(Overlay::TermKeys(q)) = h.state().overlay.as_ref() else { panic!("no quick keys") };
        assert!(q.title.contains(if prog == "htop" { "htop" } else { "btop" }), "{}", q.title);
        shot(h, &format!("t7-{prog}-keys"));
        press(h, &[Action::Back]);
        h.state_mut().run(Cmd::TermType("q".into()));
        term_wait(h, "$ ", 1);
    }
    // A second shell, its own tabs.
    h.state_mut().run(Cmd::TermNew);
    term_wait(h, "$ ", 1);
    assert_eq!(h.state().cur_tab().term.as_ref().unwrap().sessions.len(), 2);
    shot(h, "t8-two-tabs");
    h.state_mut().run(Cmd::TermSwitch(0));
    settle(h);
    // Settings beside the terminal.
    h.state_mut().run(Cmd::TermSettings);
    press(h, &[Action::Right]);
    assert_ne!(h.state().cfg.terminal.scheme, "Polka");
    shot(h, "t9-settings");
    press(h, &[Action::Down, Action::Down, Action::Right]);
    assert_eq!(h.state().cfg.terminal.font_size, 15.0);
    press(h, &[Action::Back]);
    // Mouse: drag over the first line picks its text.
    let t = h.state().cur_tab().term.as_deref().unwrap();
    let (cw, ch) = t.cell;
    let _ = (cw, ch);
    // Held ○ leaves for the sidebar; the shells keep running.
    h.state_mut().handle(Input::Press(Action::Back));
    for _ in 0..16 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    h.state_mut().handle(Input::Release(Action::Back));
    assert!(h.state().focus == Focus::Sidebar);
    h.state_mut().focus = Focus::Panes;
    // Back to files and in again: the same shells.
    h.state_mut().run(Cmd::TermHide);
    settle(h);
    assert!(!h.state().term_shown());
    h.state_mut().run(Cmd::TermOpen(None));
    assert_eq!(h.state().cur_tab().term.as_ref().unwrap().sessions.len(), 2);
    // exit closes a tab; the last one closes the page.
    for _ in 0..2 {
        type_text(h, "exit");
        key(h, Key::Enter, Modifiers::NONE);
        for _ in 0..50 {
            settle(h);
            let n = h.state().cur_tab().term.as_ref().map_or(0, |t| t.sessions.len());
            if n < 2 {
                break;
            }
        }
    }
    for _ in 0..50 {
        if h.state().cur_tab().term.is_none() {
            break;
        }
        settle(h);
    }
    assert!(h.state().cur_tab().term.is_none());
}

#[test]
#[ignore]
fn terminal_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    terminal(&mut h);
}

/// A Steam install of its own: two libraries, a game without a manifest,
/// prefixes named by a manifest, a non-Steam shortcut and the app cache.
fn fake_steam(root: &std::path::Path) {
    let w = |p: std::path::PathBuf, s: &[u8]| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    };
    let lib2 = root.join("library2");
    w(
        root.join("steamapps/libraryfolders.vdf"),
        format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", root.display(), lib2.display()).as_bytes(),
    );
    let manifest = |id: u32, name: &str, dir: &str| format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{dir}\"\n}}\n");
    w(root.join("steamapps/appmanifest_10.acf"), manifest(10, "Counter-Strike", "Counter-Strike").as_bytes());
    w(root.join("steamapps/common/Counter-Strike/hl.exe"), b"MZ");
    w(root.join("steamapps/common/Orphan Folder/readme.txt"), b"hi");
    w(lib2.join("steamapps/appmanifest_20.acf"), manifest(20, "Half-Life 2", "Half-Life 2").as_bytes());
    w(lib2.join("steamapps/common/Half-Life 2/hl2.exe"), b"MZ");
    for id in ["10", "3000000000", "555", "777", "0"] {
        std::fs::create_dir_all(root.join("steamapps/compatdata").join(id).join("pfx/drive_c/users")).unwrap();
    }
    // shortcuts.vdf: { shortcuts { 0 { appid 3000000000  AppName "My Game" } } }
    let mut b = vec![0x00];
    b.extend(b"shortcuts\0");
    b.push(0x00);
    b.extend(b"0\0");
    b.push(0x02);
    b.extend(b"appid\0");
    b.extend(3_000_000_000u32.to_le_bytes());
    b.push(0x01);
    b.extend(b"AppName\0My Game\0");
    b.extend([0x08, 0x08, 0x08]);
    w(root.join("userdata/42/config/shortcuts.vdf"), &b);
    // appinfo.vdf v29 with one app: 555 is Portal.
    let mut kv = vec![0x00];
    kv.extend(0u32.to_le_bytes()); // appinfo
    kv.push(0x00);
    kv.extend(1u32.to_le_bytes()); // common
    kv.push(0x01);
    kv.extend(2u32.to_le_bytes()); // name
    kv.extend(b"Portal\0");
    kv.extend([0x08, 0x08, 0x08]);
    let mut entry = Vec::new();
    entry.extend([0u8; 4 + 4 + 8 + 20 + 4 + 20]);
    entry.extend(&kv);
    let mut a = Vec::new();
    a.extend(0x0756_4429u32.to_le_bytes());
    a.extend(1u32.to_le_bytes());
    let table_at = 16 + 4 + 4 + entry.len() + 4;
    a.extend((table_at as u64).to_le_bytes());
    a.extend(555u32.to_le_bytes());
    a.extend((entry.len() as u32).to_le_bytes());
    a.extend(&entry);
    a.extend(0u32.to_le_bytes());
    a.extend(3u32.to_le_bytes());
    a.extend(b"appinfo\0common\0name\0");
    w(root.join("appcache/appinfo.vdf"), &a);
    // Counter-Strike's icon in the newer cache layout.
    let icon = root.join("appcache/librarycache/10/0123456789abcdef0123456789abcdef01234567.jpg");
    std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(32, 32, |x, y| image::Rgb([200, (x * 8) as u8, (y * 8) as u8])).save(&icon).unwrap();
}

/// Steam in the sidebar: its games and prefixes by name, in and back out.
fn steam(h: &mut Harness<'static, App>) {
    use crate::app::Focus;
    use crate::i18n::tr;
    use crate::steam::View;
    use crate::vfs::Location;
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let root = dir.join("fake-steam");
    fake_steam(&root);
    // A real exe with an icon, named like the game, gives it its icon.
    let exe = dir.join("Townfall.exe");
    let game_exe = root.join("steamapps/common/Counter-Strike/Counter-Strike.exe");
    if exe.exists() {
        std::fs::copy(&exe, &game_exe).unwrap();
    }
    unsafe { std::env::set_var("POLKA_STEAM_ROOT", &root) };
    h.state_mut().refresh_sidebar();
    let i = h.state().side_items.iter().position(|it| it.loc == Location::Steam(View::Root)).expect("Steam in the sidebar");
    h.state_mut().side_cursor = i;
    h.state_mut().focus = Focus::Sidebar;
    press(h, &[Action::Accept]);
    assert!(h.state().focus == Focus::Sidebar);
    press(h, &[Action::Right]);
    let names = |h: &Harness<'static, App>| h.state().pane().entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(h.state().pane().loc, Location::Steam(View::Root));
    assert_eq!(names(h), vec![tr("Games").to_string(), tr("Prefixes").to_string()]);
    shot(h, "s0-steam");
    // Games from both libraries, one without a manifest by its folder.
    press(h, &[Action::Accept]);
    assert_eq!(h.state().pane().loc, Location::Steam(View::Games));
    assert_eq!(names(h), vec!["Counter-Strike", "Half-Life 2", "Orphan Folder"]);
    if exe.exists() {
        let cs = std::fs::canonicalize(root.join("steamapps/common/Counter-Strike")).unwrap();
        let (picked, _) = crate::steam::icon(&cs).unwrap();
        assert_eq!(picked.as_deref(), Some(std::fs::canonicalize(&game_exe).unwrap().as_path()));
        settle(h);
    }
    shot(h, "s1-games");
    press(h, &[Action::Down, Action::Accept]);
    let hl2 = std::fs::canonicalize(root.join("library2/steamapps/common/Half-Life 2")).unwrap();
    assert_eq!(h.state().pane().loc, Location::Local(hl2));
    assert_eq!(names(h), vec!["hl2.exe"]);
    // Back up lands in the games list on the game.
    press(h, &[Action::Back]);
    assert_eq!(h.state().pane().loc, Location::Steam(View::Games));
    assert_eq!(h.state().pane().current().unwrap().name, "Half-Life 2");
    press(h, &[Action::Back]);
    assert_eq!(h.state().pane().loc, Location::Steam(View::Root));
    // Prefixes by game: a manifest, a non-Steam shortcut, the app cache.
    press(h, &[Action::Down, Action::Accept]);
    assert_eq!(h.state().pane().loc, Location::Steam(View::Prefixes));
    let mut got = names(h);
    got.sort();
    let unknown = format!("{} 777", tr("Unknown game"));
    let mut want = vec!["Counter-Strike".to_string(), "My Game".into(), "Portal".into(), tr("Shared prefix").to_string(), unknown];
    want.sort();
    assert_eq!(got, want);
    shot(h, "s2-prefixes");
    let mg = h.state().pane().entries.iter().position(|e| e.name == "My Game").unwrap();
    h.state_mut().pane_mut().set_cursor(mg);
    press(h, &[Action::Accept, Action::Accept]);
    let pfx = std::fs::canonicalize(root.join("steamapps/compatdata/3000000000/pfx")).unwrap();
    assert_eq!(h.state().pane().loc, Location::Local(pfx));
    let crumbs: Vec<String> = h.state().pane().loc.crumbs().into_iter().map(|(l, _)| l).collect();
    assert_eq!(crumbs, vec!["Steam".to_string(), tr("Prefixes").to_string(), "My Game".into(), "pfx".into()]);
    shot(h, "s3-prefix-inside");
    press(h, &[Action::Back, Action::Back]);
    assert_eq!(h.state().pane().loc, Location::Steam(View::Prefixes));
    unsafe { std::env::remove_var("POLKA_STEAM_ROOT") };
    h.state_mut().refresh_sidebar();
}

#[test]
#[ignore]
fn steam_screens() {
    // Points Steam at a fake one for the whole process.
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    steam(&mut h);
}

/// Drag and drop with other programs: files coming in onto a folder, a
/// drag passing by, ours leaving the window (handed to the system) and
/// coming back. The Wayland side is faked; egui gets what it would.
fn other_programs(h: &mut Harness<'static, App>) {
    use crate::dnd::Ev;
    use crate::i18n::tr;
    use crate::vfs::Location;
    use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
    let base = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let dir = base.join("dnd");
    let ext = base.join("dnd-elsewhere");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ext);
    std::fs::create_dir_all(dir.join("In")).unwrap();
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(dir.join("out.txt"), "going out\n").unwrap();
    for n in ["ext-a.txt", "ext b.txt"] {
        std::fs::write(ext.join(n), n).unwrap();
    }
    let (fake, tx, started) = crate::dnd::Dnd::fake();
    h.state_mut().dnd = Some(fake);
    let mut cfg = h.state().cfg.clone();
    cfg.dual = false;
    cfg.view = crate::config::ViewMode::List;
    h.state_mut().cfg = cfg.clone();
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    // What the app would hand egui (kittest has no raw input hook).
    let feed = |h: &mut Harness<'static, App>| {
        for _ in 0..3 {
            h.step();
            let evs = std::mem::take(&mut h.state_mut().dnd_inject);
            for e in evs {
                h.event(e);
            }
            h.step();
        }
    };
    let rect_of = |h: &Harness<'static, App>, name: &str| {
        let p = h.state().pane();
        let i = p.entries.iter().position(|e| e.name == name).unwrap_or_else(|| panic!("{name} not shown"));
        let (_, row, hot) = *p.drawn.iter().find(|d| d.0 == i).expect("not drawn");
        (row, hot)
    };
    let menu = |h: &Harness<'static, App>| match &h.state().overlay {
        Some(crate::app::Overlay::Menu(m)) => (m.title.clone(), m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()),
        _ => (String::new(), Vec::new()),
    };
    // Two files from elsewhere onto the folder: copy them in.
    let (row_in, _) = rect_of(h, "In");
    let c = row_in.center();
    tx.send(Ev::Enter { x: c.x - 150.0, y: c.y + 80.0, files: Some(vec![ext.join("ext-a.txt"), ext.join("ext b.txt")]) }).unwrap();
    feed(h);
    tx.send(Ev::Motion { x: c.x, y: c.y }).unwrap();
    feed(h);
    assert_eq!(h.state().drag.as_ref().map(|d| d.sources.len()), Some(2));
    shot(h, "d0-drag-in");
    tx.send(Ev::Drop).unwrap();
    feed(h);
    settle(h);
    let (title, labels) = menu(h);
    assert!(title.ends_with("→ In"), "{title}");
    assert_eq!(labels, [tr("Move here"), tr("Copy here"), tr("Create link here"), tr("Cancel")]);
    shot(h, "d1-drop-menu");
    menu_pick(h, tr("Copy here"));
    wait_jobs(h);
    assert!(dir.join("In/ext-a.txt").exists() && dir.join("In/ext b.txt").exists(), "not copied in");
    assert!(ext.join("ext-a.txt").exists(), "the originals stay");
    // A drag that only passes by leaves nothing behind.
    tx.send(Ev::Enter { x: c.x, y: c.y, files: Some(vec![ext.join("ext-a.txt")]) }).unwrap();
    feed(h);
    tx.send(Ev::Leave).unwrap();
    feed(h);
    settle(h);
    assert!(h.state().drag.is_none() && h.state().overlay.is_none());
    assert!(!h.ctx.input(|i| i.pointer.primary_down()), "button left down");
    // Ours pulled out of the window: the system's drag gets its path.
    let press_at = |h: &mut Harness<'static, App>, pos: Pos2, down: bool| {
        h.event(Event::PointerMoved(pos));
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: down, modifiers: Modifiers::NONE });
        h.step();
    };
    let move_to = |h: &mut Harness<'static, App>, pos: Pos2| {
        h.event(Event::PointerMoved(pos));
        h.step();
    };
    let pull_out = |h: &mut Harness<'static, App>| {
        let (_, hot) = rect_of(h, "out.txt");
        press_at(h, hot.center(), true);
        move_to(h, hot.center() + vec2(8.0, 8.0));
        move_to(h, hot.center() + vec2(100.0, 40.0));
        move_to(h, pos2(1300.0, 300.0));
        move_to(h, pos2(1310.0, 305.0));
    };
    pull_out(h);
    let files = started.recv_timeout(std::time::Duration::from_secs(2)).expect("no drag started");
    assert_eq!(files, vec![dir.join("out.txt")]);
    // Dropped in another program: done here, the button up again.
    tx.send(Ev::OutEnded).unwrap();
    feed(h);
    assert!(h.state().drag.is_none());
    assert!(!h.ctx.input(|i| i.pointer.primary_down()), "button left down");
    assert!(dir.join("out.txt").exists());
    // Out and back in onto the folder: the usual question.
    pull_out(h);
    started.recv_timeout(std::time::Duration::from_secs(2)).expect("no drag started");
    tx.send(Ev::Enter { x: c.x + 40.0, y: c.y, files: None }).unwrap();
    tx.send(Ev::Motion { x: c.x, y: c.y }).unwrap();
    feed(h);
    tx.send(Ev::Drop).unwrap();
    tx.send(Ev::OutEnded).unwrap();
    feed(h);
    settle(h);
    let (title, labels) = menu(h);
    assert_eq!(title, "out.txt → In");
    assert_eq!(labels[0], tr("Move here"));
    press(h, &[Action::Back]);
    assert!(h.state().drag.is_none());
    h.state_mut().dnd = None;
}

#[test]
#[ignore]
fn dnd_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    other_programs(&mut h);
}

/// Administrator rights through a fake sudo (password "testpw"; it runs the
/// command in a user namespace, where "root" may write the locked folder):
/// saving a locked file, a wrong password asked again, then rename, new
/// folder, copy, delete and a link with the password kept.
fn administrator(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay, OskPurpose};
    use crate::vfs::Location;
    use std::os::unix::fs::PermissionsExt;
    let base = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let bin = base.join("fake-sudo-bin");
    let dir = base.join("locked");
    let _ = std::process::Command::new("chmod").arg("-R").arg("u+w").arg(&dir).status();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        bin.join("sudo"),
        "#!/bin/sh\n# test sudo: -S -p '' then -k -v or -- cmd\nread pw\n[ \"$pw\" = testpw ] || { echo 'Sorry, try again.' >&2; exit 1; }\nwhile [ $# -gt 0 ] && [ \"$1\" != -- ]; do shift; done\n[ $# -eq 0 ] && exit 0\nshift\nexec unshare -r \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("sudo"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    unsafe { std::env::set_var("PATH", format!("{}:{path}", bin.display())) };
    std::fs::write(dir.join("conf.txt"), "a=1\n").unwrap();
    std::fs::write(base.join("extra.txt"), "extra\n").unwrap();
    std::fs::set_permissions(dir.join("conf.txt"), std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let sudo_asked = |h: &Harness<'static, App>| matches!(&h.state().overlay, Some(Overlay::Osk(k)) if matches!(k.purpose, OskPurpose::Sudo { .. }));
    // Edit the locked file and save: the password is asked.
    let i = h.state().pane().entries.iter().position(|e| e.name == "conf.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().view_current();
    settle(h);
    let tab = h.state().tab;
    if let Some(crate::viewer::Content::Text(t)) = h.state_mut().tabs[tab].viewer.as_mut().map(|v| &mut v.content) {
        t.ed.caret = t.doc.text.len();
        assert!(t.insert("b=2\n"));
    } else {
        panic!("not opened as text");
    }
    h.state_mut().run(Cmd::ViewerSave);
    settle(h);
    assert!(sudo_asked(h), "no password asked");
    // Shown as asterisks by default.
    osk_type(h, "bad");
    shot(h, "e0-sudo-password");
    press(h, &[Action::Menu]);
    wait_jobs(h);
    settle(h);
    assert!(sudo_asked(h), "a wrong password isn't asked again");
    let title = match &h.state().overlay {
        Some(Overlay::Osk(k)) => k.title.clone(),
        _ => String::new(),
    };
    assert!(title.contains(crate::i18n::tr("Wrong password — administrator password (sudo)")), "{title}");
    osk_type(h, "testpw");
    press(h, &[Action::Menu]);
    wait_jobs(h);
    settle(h);
    assert_eq!(std::fs::read_to_string(dir.join("conf.txt")).unwrap(), "a=1\nb=2\n");
    let mode = std::fs::metadata(dir.join("conf.txt")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444, "the mode is kept");
    assert!(matches!(h.state().cur_tab().viewer.as_ref().map(|v| &v.content), Some(crate::viewer::Content::Text(t)) if !t.ed.dirty), "still unsaved");
    h.state_mut().run(Cmd::CloseViewer);
    settle(h);
    // The password is kept a while: no more asking.
    let i = h.state().pane().entries.iter().position(|e| e.name == "conf.txt").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::Rename);
    osk_type(h, "renamed.txt");
    press(h, &[Action::Menu]);
    wait_jobs(h);
    assert!(dir.join("renamed.txt").exists() && !dir.join("conf.txt").exists(), "not renamed");
    h.state_mut().run(Cmd::NewFolder);
    osk_type(h, "made");
    press(h, &[Action::Menu]);
    wait_jobs(h);
    assert!(dir.join("made").is_dir(), "no folder");
    h.state_mut().request_transfer(vec![Location::Local(base.join("extra.txt"))], crate::vfs::Dest::Dir(dir.clone()), false);
    wait_jobs(h);
    settle(h);
    wait_jobs(h);
    assert!(dir.join("extra.txt").exists(), "not copied");
    assert!(!sudo_asked(h));
    h.state_mut().start(crate::ops::Op::Delete { paths: vec![dir.join("extra.txt")] });
    wait_jobs(h);
    settle(h);
    wait_jobs(h);
    assert!(!dir.join("extra.txt").exists(), "not deleted");
    h.state_mut().run(Cmd::DropLink { sources: vec![base.join("extra.txt")], dir: dir.clone() });
    wait_jobs(h);
    settle(h);
    wait_jobs(h);
    assert!(dir.join("extra.txt").symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()), "no link");
    // Fully hidden passwords: nothing shows as it's typed.
    h.state_mut().cfg.password_hidden = true;
    h.state_mut().sudo_pw = None;
    h.state_mut().run(Cmd::NewFolder);
    osk_type(h, "second");
    press(h, &[Action::Menu]);
    settle(h);
    assert!(sudo_asked(h));
    osk_type(h, "testpw");
    shot(h, "e1-sudo-hidden");
    press(h, &[Action::Menu]);
    wait_jobs(h);
    assert!(dir.join("second").is_dir());
    h.state_mut().cfg.password_hidden = false;
    unsafe { std::env::set_var("PATH", path) };
    let _ = std::process::Command::new("chmod").arg("-R").arg("u+w").arg(&dir).status();
}

#[test]
#[ignore]
fn admin_screens() {
    // Puts a fake sudo first in PATH for the whole process.
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    administrator(&mut h);
}

/// What the terminal window shows.
fn window_screen(h: &Harness<'static, App>) -> String {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line, Point};
    let Some(t) = h.state().term_window.as_deref() else { return String::new() };
    let Some(s) = t.cur() else { return String::new() };
    let term = s.term.lock();
    let mut out = String::new();
    for l in 0..term.screen_lines() as i32 {
        out.extend((0..term.columns()).map(|c| term.grid()[Point::new(Line(l), Column(c))].c));
        out.push('\n');
    }
    out
}

/// Installing a package (a fake pacman behind a fake sudo) in a terminal
/// window that closes when done, and a failing script whose window waits.
fn packages(h: &mut Harness<'static, App>) {
    use crate::i18n::tr;
    use crate::vfs::Location;
    use std::os::unix::fs::PermissionsExt;
    let base = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let bin = base.join("fake-pkg-bin");
    let dir = base.join("downloads");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let script = |name: &str, body: &str| {
        std::fs::write(bin.join(name), body).unwrap();
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("sudo", "#!/bin/sh\nexec \"$@\"\n");
    script("pacman", "#!/bin/sh\necho \"loading $2\"\nprintf ':: Proceed with installation? [Y/n] '\nread a\n[ \"$a\" = y ] && echo installed > \"$(dirname \"$2\")/installed.flag\"\n");
    let path = std::env::var("PATH").unwrap_or_default();
    unsafe { std::env::set_var("PATH", format!("{}:{path}", bin.display())) };
    // A real-looking package (.PKGINFO inside) and a plain tar named like one.
    let src = base.join("pkg-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join(".PKGINFO"), "pkgname = tool\n").unwrap();
    std::fs::write(src.join("tool"), "x").unwrap();
    let tar = |out: &str, files: &[&str]| {
        let ok = std::process::Command::new("tar").arg("--zstd").arg("-cf").arg(dir.join(out)).arg("-C").arg(&src).args(files).status().unwrap();
        assert!(ok.success());
    };
    tar("tool-1.0-1-x86_64.pkg.tar.zst", &[".PKGINFO", "tool"]);
    tar("fake-1.0-1-x86_64.pkg.tar.zst", &["tool"]);
    std::fs::write(dir.join("broken.sh"), "echo going wrong\nexit 3\n").unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let wait_for = |h: &mut Harness<'static, App>, what: &str| {
        for _ in 0..150 {
            settle(h);
            if window_screen(h).contains(what) {
                return;
            }
        }
        panic!("window never showed {what:?}:\n{}", window_screen(h));
    };
    let at = |h: &Harness<'static, App>, name: &str| h.state().pane().entries.iter().position(|e| e.name == name).unwrap();
    let label = format!("{} (pacman)", tr("Install package"));
    let labels = |h: &Harness<'static, App>| match &h.state().overlay {
        Some(crate::app::Overlay::Menu(m)) => m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    // Checked by what's inside: the fake gets no badge and no installing.
    for _ in 0..20 {
        settle(h);
    }
    h.state_mut().cfg.view = crate::config::ViewMode::Grid;
    settle(h);
    shot(h, "p0-badges-grid");
    h.state_mut().cfg.view = crate::config::ViewMode::List;
    settle(h);
    shot(h, "p0-badges-list");
    let fake = at(h, "fake-1.0-1-x86_64.pkg.tar.zst");
    h.state_mut().pane_mut().set_cursor(fake);
    press(h, &[Action::Actions]);
    assert!(!labels(h).contains(&label), "{:?}", labels(h));
    press(h, &[Action::Back]);
    // Not installing on ✕ (a setting): the package opens as a folder.
    h.state_mut().cfg.install_on_open = false;
    let real = at(h, "tool-1.0-1-x86_64.pkg.tar.zst");
    h.state_mut().pane_mut().set_cursor(real);
    press(h, &[Action::Accept]);
    assert!(h.state().pane().loc.is_archive(), "not opened as a folder");
    assert!(h.state().term_window.is_none());
    press(h, &[Action::Back]);
    h.state_mut().cfg.install_on_open = true;
    // △ still has opening as a folder; ✕ installs with pacman.
    let real = at(h, "tool-1.0-1-x86_64.pkg.tar.zst");
    h.state_mut().pane_mut().set_cursor(real);
    press(h, &[Action::Actions]);
    assert!(labels(h).contains(&label) && labels(h).contains(&tr("Open as folder").to_string()), "{:?}", labels(h));
    shot(h, "p0-menu");
    press(h, &[Action::Back]);
    press(h, &[Action::Accept]);
    assert!(h.state().term_window.is_some(), "no window");
    wait_for(h, "Proceed with installation?");
    assert!(window_screen(h).contains("sudo pacman -U"), "the command isn't shown");
    shot(h, "p1-installing");
    // Answered from the keyboard; done, the window goes.
    type_text(h, "y");
    key(h, egui::Key::Enter, egui::Modifiers::NONE);
    for _ in 0..100 {
        settle(h);
        if h.state().term_window.is_none() {
            break;
        }
    }
    assert!(h.state().term_window.is_none(), "window left open:\n{}", window_screen(h));
    assert!(dir.join("installed.flag").exists());
    std::fs::remove_file(dir.join("installed.flag")).ok();
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0.contains(tr("Done"))));
    // A script that fails: the window waits for Enter (✕ held).
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().reload(&cfg);
    settle(h);
    let i = h.state().pane().entries.iter().position(|e| e.name == "broken.sh").unwrap();
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Actions]);
    menu_pick(h, tr("Run in terminal"));
    wait_for(h, "(3)");
    shot(h, "p2-script-failed");
    settle(h);
    assert!(h.state().term_window.is_some(), "a failed run closed at once");
    h.state_mut().handle(Input::Press(Action::Accept));
    for _ in 0..16 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    h.state_mut().handle(Input::Release(Action::Accept));
    for _ in 0..60 {
        settle(h);
        if h.state().term_window.is_none() {
            break;
        }
    }
    assert!(h.state().term_window.is_none(), "Enter didn't close it");
    // Failing at once, before anything is shown: the window stays to say so.
    let cfg = h.state().cfg.clone();
    let t = crate::terminal::Terminal::with_command(&cfg.terminal, &dir, &h.state().ctx, "exit 5", "x").unwrap();
    h.state_mut().term_window = Some(Box::new(t));
    h.state_mut().term_window_title = "Quick".into();
    for _ in 0..40 {
        settle(h);
        if h.state().term_window.as_ref().is_some_and(|w| w.failed) {
            break;
        }
    }
    assert!(h.state().term_window.as_ref().is_some_and(|w| w.failed), "closed without a word");
    assert!(h.state().term_window_title.contains("(5)"), "{}", h.state().term_window_title);
    shot(h, "p3-failed-at-once");
    press(h, &[Action::Back]);
    assert!(h.state().term_window.is_none());
    unsafe { std::env::set_var("PATH", path) };
}

#[test]
#[ignore]
fn package_screens() {
    // Puts fake tools first in PATH for the whole process.
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    packages(&mut h);
}

/// Keyboard layouts: chosen in the settings, ⇄ goes through them; Hangul
/// builds syllables, kana takes ゛ and Caps writes katakana.
fn layouts(h: &mut Harness<'static, App>) {
    use crate::app::{Cmd, Overlay, OskKey, row};
    let codes = crate::keyboard::CODES;
    let cfg = h.state().cfg.clone();
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    h.state_mut().pane_mut().navigate(crate::vfs::Location::Local(dir), None, &cfg);
    settle(h);
    for code in ["KO", "JA"] {
        let i = codes.iter().position(|c| *c == code).unwrap();
        if !h.state().cfg.layouts().iter().any(|c| c == code) {
            h.state_mut().settings_toggle_layout(i);
        }
    }
    h.state_mut().run(Cmd::Settings);
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::LAYOUTS;
    }
    h.state_mut().ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-reveal"), true));
    settle(h);
    shot(h, "k0-settings-layouts");
    press(h, &[Action::Back]);
    h.state_mut().run(Cmd::NewFolder);
    let to = |h: &mut Harness<'static, App>, name: &str| {
        for _ in 0..10 {
            if let Some(Overlay::Osk(k)) = h.state().overlay.as_ref() {
                if crate::keyboard::LAYOUTS[k.layout].name == name {
                    return;
                }
            }
            press(h, &[Action::View]);
        }
        panic!("no {name} layout");
    };
    let text = |h: &Harness<'static, App>| match h.state().overlay.as_ref() {
        Some(Overlay::Osk(k)) => k.string(),
        _ => String::new(),
    };
    to(h, "한");
    for c in "ㅎㅏㄴㄱㅡㄹ".chars() {
        h.state_mut().osk_key(OskKey::Char(c));
    }
    settle(h);
    assert_eq!(text(h), "한글");
    shot(h, "k1-korean");
    to(h, "あ");
    for c in "か゛".chars() {
        h.state_mut().osk_key(OskKey::Char(c));
    }
    assert_eq!(text(h), "한글が");
    h.state_mut().osk_key(OskKey::Caps);
    settle(h);
    shot(h, "k2-katakana");
    if let Some(Overlay::Osk(k)) = h.state_mut().overlay.as_mut() {
        (k.row, k.col) = (1, 5);
    }
    press(h, &[Action::Accept]);
    assert_eq!(text(h), "한글がカ");
    press(h, &[Action::Back]);
    crate::keyboard::set_last_layout(0);
    crate::keyboard::set_enabled(&cfg.layouts());
}

#[test]
#[ignore]
fn layout_screens() {
    // The keyboard's layouts are process-wide.
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    layouts(&mut h);
}

/// "Add to Steam": an .exe (with its icon and Proton) and a launcher into a
/// fake Steam's library, once; a running Steam asks first; no Steam, no
/// Steam things at all.
fn add_to_steam(h: &mut Harness<'static, App>) {
    use crate::i18n::tr;
    use crate::vfs::Location;
    let base = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap();
    let root = base.join("fake-steam-add");
    let _ = std::fs::remove_dir_all(&root);
    fake_steam(&root);
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/loginusers.vdf"), "\"users\"\n{\n\t\"76561197960265770\"\n\t{\n\t\t\"AccountName\"\t\t\"t\"\n\t\t\"MostRecent\"\t\t\"1\"\n\t}\n}\n").unwrap();
    std::fs::write(root.join("config/config.vdf"), "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n").unwrap();
    std::fs::create_dir_all(root.join("steamapps/common/Proton - Experimental")).unwrap();
    let apps = base.join("apps");
    let _ = std::fs::remove_dir_all(&apps);
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::copy(std::env::var("HOME").unwrap() + "/Games/sh/Townfall.exe", apps.join("Townfall.exe")).unwrap();
    std::fs::write(apps.join("tool.desktop"), "[Desktop Entry]\nType=Application\nName=Tool Thing\nExec=/usr/bin/tool --flag %U\nIcon=utilities-terminal\n").unwrap();
    unsafe {
        std::env::set_var("POLKA_STEAM_ROOT", &root);
        std::env::set_var("POLKA_STEAM_RUNNING", "0");
    }
    let cfg = h.state().cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(apps.clone()), None, &cfg);
    settle(h);
    let pick = |h: &mut Harness<'static, App>, name: &str| {
        let i = h.state().pane().entries.iter().position(|e| e.name == name).unwrap();
        h.state_mut().pane_mut().set_cursor(i);
        press(h, &[Action::Actions]);
    };
    let entries = || {
        let b = std::fs::read(root.join("userdata/42/config/shortcuts.vdf")).unwrap();
        let tree = crate::steam_add::parse(&b).expect("unreadable shortcuts.vdf");
        let Some((_, crate::steam_add::Node::Map(list))) = tree.into_iter().next() else { panic!() };
        list.into_iter()
            .map(|(_, e)| match e {
                crate::steam_add::Node::Map(f) => f.into_iter().filter_map(|(k, v)| match v {
                    crate::steam_add::Node::Str(s) => Some((k, s)),
                    crate::steam_add::Node::Int(i) => Some((k, i.to_string())),
                    _ => None,
                }).collect::<std::collections::HashMap<_, _>>(),
                _ => Default::default(),
            })
            .collect::<Vec<_>>()
    };
    pick(h, "Townfall.exe");
    shot(h, "g0-add-menu");
    menu_pick(h, tr("Add to Steam"));
    let list = entries();
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!(list[0]["AppName"], "My Game", "the old one kept");
    let new = &list[1];
    assert_eq!(new["AppName"], "Townfall");
    assert_eq!(new["Exe"], format!("\"{}\"", apps.join("Townfall.exe").display()));
    assert!(std::path::Path::new(&new["icon"]).exists(), "no icon: {}", new["icon"]);
    let cfgvdf = std::fs::read_to_string(root.join("config/config.vdf")).unwrap();
    assert!(cfgvdf.contains(&format!("\"{}\"", new["appid"])) && cfgvdf.contains("proton_experimental"), "{cfgvdf}");
    // Once only.
    pick(h, "Townfall.exe");
    menu_pick(h, tr("Add to Steam"));
    assert_eq!(entries().len(), 2);
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0 == tr("Already in Steam")));
    // A launcher: its program and arguments, its theme icon.
    pick(h, "tool.desktop");
    menu_pick(h, tr("Add to Steam"));
    let list = entries();
    let t = &list[2];
    assert_eq!((t["AppName"].as_str(), t["Exe"].as_str(), t["LaunchOptions"].as_str()), ("Tool Thing", "\"/usr/bin/tool\"", "--flag"));
    // Steam running: it asks.
    unsafe { std::env::set_var("POLKA_STEAM_RUNNING", "1") };
    std::fs::write(apps.join("other.exe"), b"MZ").unwrap();
    let cfg = h.state().cfg.clone();
    h.state_mut().pane_mut().reload(&cfg);
    settle(h);
    pick(h, "other.exe");
    menu_pick(h, tr("Add to Steam"));
    let labels: Vec<String> = match &h.state().overlay {
        Some(crate::app::Overlay::Menu(m)) => m.items.iter().map(|i| i.label.clone()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(labels, [tr("Restart Steam and add"), tr("Add (shows after Steam restarts)"), tr("Cancel")]);
    shot(h, "g1-steam-running");
    menu_pick(h, tr("Add (shows after Steam restarts)"));
    assert_eq!(entries().len(), 4);
    // No Steam: nothing about it.
    unsafe { std::env::set_var("POLKA_STEAM_ROOT", base.join("no-steam-here")) };
    h.state_mut().refresh_sidebar();
    pick(h, "Townfall.exe");
    let labels: Vec<String> = match &h.state().overlay {
        Some(crate::app::Overlay::Menu(m)) => m.items.iter().map(|i| i.label.clone()).collect(),
        _ => Vec::new(),
    };
    assert!(!labels.contains(&tr("Add to Steam").to_string()), "{labels:?}");
    assert!(!h.state().side_items.iter().any(|it| matches!(it.loc, Location::Steam(_))));
    press(h, &[Action::Back]);
    unsafe {
        std::env::remove_var("POLKA_STEAM_ROOT");
        std::env::remove_var("POLKA_STEAM_RUNNING");
    }
    h.state_mut().refresh_sidebar();
}

#[test]
#[ignore]
fn steam_add_screens() {
    // Points Steam at a fake one for the whole process.
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    add_to_steam(&mut h);
}

/// The list: a type column, headers that sort (again: the other way), and
/// check boxes that take items in and out of the selection with the mouse.
fn list_columns(h: &mut Harness<'static, App>) {
    use crate::config::{SortKey, ViewMode};
    use crate::vfs::Location;
    use egui::{Event, Modifiers, PointerButton, Pos2};
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").unwrap()).unwrap().join("columns");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Folder")).unwrap();
    std::fs::write(dir.join("big.txt"), "x".repeat(5000)).unwrap();
    std::fs::write(dir.join("small.md"), "# x").unwrap();
    image::RgbImage::new(8, 8).save(dir.join("picture.png")).unwrap();
    std::fs::write(dir.join("setup.exe"), b"MZ").unwrap();
    let mut cfg = h.state().cfg.clone();
    cfg.view = ViewMode::List;
    cfg.dual = false;
    cfg.sort = SortKey::Name;
    cfg.sort_reverse = false;
    h.state_mut().cfg = cfg.clone();
    h.state_mut().overlay = None;
    h.state_mut().pane_mut().navigate(Location::Local(dir.clone()), None, &cfg);
    settle(h);
    let click = |h: &mut Harness<'static, App>, pos: Pos2| {
        h.event(Event::PointerMoved(pos));
        h.step();
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
        settle(h);
    };
    let head = |h: &Harness<'static, App>, k: SortKey| h.state().pane().heads.iter().find(|(x, _)| *x == k).map(|(_, r)| r.center()).expect("no such header");
    let names = |h: &Harness<'static, App>| h.state().pane().entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    shot(h, "l0-type-column");
    // Size: biggest first; again: smallest first.
    click(h, head(h, SortKey::Size));
    assert_eq!((h.state().cfg.sort, h.state().cfg.sort_reverse), (SortKey::Size, false));
    let files: Vec<String> = names(h).into_iter().filter(|n| n != "Folder").collect();
    assert_eq!(files.first().map(String::as_str), Some("big.txt"), "{files:?}");
    click(h, head(h, SortKey::Size));
    assert!(h.state().cfg.sort_reverse);
    shot(h, "l1-sorted-by-size-reversed");
    click(h, head(h, SortKey::Type));
    assert_eq!((h.state().cfg.sort, h.state().cfg.sort_reverse), (SortKey::Type, false));
    click(h, head(h, SortKey::Name));
    assert_eq!(h.state().cfg.sort, SortKey::Name);
    // Check boxes: two in, then one out again; the other stays.
    let row = |h: &Harness<'static, App>, name: &str| {
        let p = h.state().pane();
        let i = p.entries.iter().position(|e| e.name == name).unwrap();
        p.drawn.iter().find(|d| d.0 == i).unwrap().1
    };
    let check = |h: &Harness<'static, App>, name: &str| {
        let r = row(h, name);
        egui::pos2(r.min.x + 6.0 + 11.0, r.center().y)
    };
    click(h, check(h, "big.txt"));
    click(h, check(h, "picture.png"));
    let mut sel: Vec<String> = h.state().pane().selected.iter().cloned().collect();
    sel.sort();
    assert_eq!(sel, ["big.txt", "picture.png"]);
    shot(h, "l2-checked");
    click(h, check(h, "big.txt"));
    let sel: Vec<String> = h.state().pane().selected.iter().cloned().collect();
    assert_eq!(sel, ["picture.png"], "unchecking took the wrong ones");
    h.state_mut().cfg.view = ViewMode::Grid;
    settle(h);
    shot(h, "l3-grid-checked");
    h.state_mut().cfg.view = ViewMode::List;
    h.state_mut().pane_mut().selected.clear();
    settle(h);
}

#[test]
#[ignore]
fn list_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    list_columns(&mut h);
}

// ------------------------------------------------------------------ demo

/// Pictures for the README: a made-up home (no real files, names, mounts
/// or processes), English, PlayStation glyphs, 1280×800. Not part of the
/// usual run; POLKA_DEMO_HOME puts the home where it looks real (an empty
/// folder bound over /home/alex, so paths read /home/alex):
///   bwrap --dev-bind / / --tmpfs /home --bind ~ ~ --bind "$(mktemp -d)" /home/alex \
///     env POLKA_DEMO_HOME=/home/alex POLKA_DEMO_DIR=$PWD/docs/screenshots \
///     cargo test demo_shots -- --ignored
#[test]
#[ignore]
fn demo_shots() {
    let _alone = GLOBALS.write().unwrap_or_else(|e| e.into_inner());
    let out = std::env::var_os("POLKA_DEMO_DIR").map(PathBuf::from);
    let base = std::env::temp_dir().join(format!("polka-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = std::env::var_os("POLKA_DEMO_HOME").map(PathBuf::from).unwrap_or_else(|| base.join("home/alex"));
    demo_home(&home);
    let steam = base.join("steam");
    demo_steam(&steam);
    let bin = base.join("bin");
    demo_bin(&bin);
    let lsblk = base.join("lsblk.json");
    std::fs::write(
        &lsblk,
        r#"{"blockdevices":[
          {"name":"sdz","path":"/dev/sdz","fstype":null,"mountpoints":[null],"rm":true,"hotplug":true,"type":"disk","rota":true,"size":64000000000,"children":[
            {"name":"sdz1","path":"/dev/sdz1","uuid":"5A1E-2C3D","label":"TRAVEL","fstype":"exfat","mountpoints":[null],"rm":true,"hotplug":true,"type":"part","rota":true,"size":64000000000}]}
        ]}"#,
    )
    .unwrap();
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = ["HOME", "PATH", "POLKA_STEAM_ROOT", "POLKA_LSBLK"].iter().map(|k| (*k, std::env::var_os(k))).collect();
    let path = std::env::var("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", format!("{}:{path}", bin.display()));
        std::env::set_var("POLKA_STEAM_ROOT", &steam);
        std::env::set_var("POLKA_LSBLK", &lsblk);
    }
    crate::i18n::set(Some("en"));
    let mut h = harness_with(home.clone(), [1280.0, 800.0], "-demo");
    // Places come from the made-up user-dirs.dirs, in the harness's config.
    let xdg = PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::write(
        xdg.join("user-dirs.dirs"),
        ["DESKTOP", "DOCUMENTS", "DOWNLOAD", "MUSIC", "PICTURES", "VIDEOS"]
            .iter()
            .zip(["Desktop", "Documents", "Downloads", "Music", "Pictures", "Videos"])
            .map(|(k, d)| format!("XDG_{k}_DIR=\"$HOME/{d}\"\n"))
            .collect::<String>(),
    )
    .unwrap();
    demo(&mut h, &home, &base, out.as_deref());
    let _ = std::fs::remove_file(xdg.join("user-dirs.dirs"));
    drop(h);
    crate::i18n::set(None);
    for (k, v) in saved {
        unsafe {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    if std::env::var_os("POLKA_DEMO_HOME").is_some() {
        for e in std::fs::read_dir(&home).unwrap().flatten() {
            let _ = if e.path().is_dir() { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
        }
    }
}

fn demo(h: &mut Harness<'static, App>, home: &std::path::Path, base: &std::path::Path, out: Option<&std::path::Path>) {
    use crate::app::{Cmd, Focus, Overlay, row};
    use crate::config::ViewMode;
    use crate::vfs::Location;
    let snap = |h: &mut Harness<'static, App>, name: &str| {
        // Thumbnails and icons arrive from other threads.
        for _ in 0..4 {
            settle(h);
        }
        let img = h.render().expect("render");
        if let Some(dir) = out {
            std::fs::create_dir_all(dir).unwrap();
            img.save(dir.join(format!("{name}.png"))).unwrap();
        }
    };
    let go = |h: &mut Harness<'static, App>, p: &std::path::Path| {
        let cfg = h.state().cfg.clone();
        h.state_mut().pane_mut().navigate(Location::Local(p.to_path_buf()), None, &cfg);
        settle(h);
    };
    let at = |h: &Harness<'static, App>, name: &str| h.state().pane().entries.iter().position(|e| e.name == name).unwrap_or_else(|| panic!("no {name}"));
    h.state_mut().pad = crate::input::PadKind::PlayStation;
    h.state_mut().pad_seen = true;
    // The config the app read set its own language; English for these.
    crate::i18n::set(Some("en"));
    h.state_mut().cfg.language = Some("en".into());
    h.state_mut().cfg.grid_icon = 96.0;
    h.state_mut().cfg.pinned = vec![home.join("Projects")];
    h.state_mut().refresh_sidebar();
    // Only the made-up drive: nothing the machine has mounted.
    h.state_mut().side_items.retain(|s| s.section != "Drives" || s.drive.is_some());
    assert!(h.state().side_items.iter().any(|s| s.label == "TRAVEL"));
    assert!(h.state().side_items.iter().any(|s| s.label == "Steam"));

    // Home, as a list.
    go(h, home);
    let i = at(h, "Documents");
    h.state_mut().pane_mut().set_cursor(i);
    snap(h, "01-home");

    // Pictures as a grid of thumbnails.
    go(h, &home.join("Pictures"));
    h.state_mut().run(Cmd::SetView(ViewMode::Grid));
    for _ in 0..10 {
        settle(h);
    }
    let i = at(h, "lagoon.png");
    h.state_mut().pane_mut().set_cursor(i);
    snap(h, "02-pictures-grid");
    h.state_mut().run(Cmd::SetView(ViewMode::List));

    // Two panes: Downloads, and inside an archive.
    go(h, &home.join("Downloads"));
    for _ in 0..10 {
        settle(h);
    }
    press(h, &[Action::View, Action::RightTrigger]);
    go(h, &home.join("Downloads"));
    let i = at(h, "wallpapers.zip");
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Accept]);
    assert!(h.state().pane().loc.is_archive());
    press(h, &[Action::Accept]);
    snap(h, "03-dual-archive");
    press(h, &[Action::LeftTrigger, Action::View]);

    // △ on a package: installing it, or looking inside.
    go(h, &home.join("Downloads"));
    let i = at(h, "aurora-notes-2.4.1-1-x86_64.pkg.tar.zst");
    h.state_mut().pane_mut().set_cursor(i);
    press(h, &[Action::Actions]);
    assert!(matches!(h.state().overlay, Some(Overlay::Menu(_))));
    snap(h, "04-actions");
    press(h, &[Action::Back]);
    // ✕ installs it in a window (pacman and sudo are stand-ins).
    press(h, &[Action::Accept]);
    for _ in 0..150 {
        settle(h);
        if window_screen(h).contains("Proceed with installation?") {
            break;
        }
    }
    assert!(window_screen(h).contains("Proceed with installation?"), "{}", window_screen(h));
    snap(h, "05-install");
    type_text(h, "n");
    key(h, egui::Key::Enter, egui::Modifiers::NONE);
    for _ in 0..100 {
        settle(h);
        if h.state().term_window.as_ref().is_some_and(|w| w.failed) || h.state().term_window.is_none() {
            break;
        }
    }
    h.state_mut().term_window = None;

    // The viewer: code, then Markdown.
    let project = home.join("Projects/starmap");
    go(h, &project.join("src"));
    let i = at(h, "main.rs");
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::OpenInViewer);
    snap(h, "06-code");
    h.state_mut().run(Cmd::CloseViewer);
    go(h, &project);
    let i = at(h, "README.md");
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::OpenInViewer);
    snap(h, "07-markdown");
    h.state_mut().run(Cmd::CloseViewer);
    settle(h);

    // The terminal, in the project.
    // Named bash, for the tab's title; outside the home, out of the shots.
    let sh = base.join("shell/bash");
    std::fs::create_dir_all(sh.parent().unwrap()).unwrap();
    std::fs::write(
        &sh,
        "#!/bin/sh\nexport PS1='\\[\\e[1;32m\\]alex@polka\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[0m\\]$ ' HISTFILE=/dev/null LS_COLORS= LC_ALL=C.UTF-8\nexec /usr/bin/bash --norc --noprofile\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    h.state_mut().cfg.terminal.shell = Some(sh.display().to_string());
    let i = h.state().side_items.iter().position(|it| it.term).unwrap();
    h.state_mut().side_cursor = i;
    h.state_mut().focus = Focus::Sidebar;
    press(h, &[Action::Accept, Action::Right]);
    term_wait(h, "$ ", 1);
    for cmd in ["ls --color=always -F", "wc -l src/*.rs", "grep -n fn src/stars.rs"] {
        type_text(h, cmd);
        key(h, egui::Key::Enter, egui::Modifiers::NONE);
        settle(h);
    }
    term_wait(h, "$ ", 4);
    snap(h, "08-terminal");
    h.state_mut().run(Cmd::TermClose);
    settle(h);

    // Disk usage of the home folder.
    h.state_mut().focus_sidebar();
    let i = h.state().side_items.iter().position(|s| s.usage).unwrap();
    h.state_mut().side_cursor = i;
    press(h, &[Action::Accept, Action::Right]);
    match &h.state().cur_tab().usage.as_ref().unwrap().state {
        crate::usage::State::Start { targets, .. } => assert_eq!(targets[0].path, home),
        _ => panic!("not the start screen"),
    }
    press(h, &[Action::Accept]);
    for _ in 0..100 {
        settle(h);
        if h.state().cur_tab().usage.as_ref().unwrap().ready().is_some() {
            break;
        }
    }
    snap(h, "09-disk-usage");
    h.state_mut().run(Cmd::UsageClose);
    settle(h);

    // Steam: games by name, with their icons.
    let i = h.state().side_items.iter().position(|s| s.label == "Steam").unwrap();
    h.state_mut().focus_sidebar();
    h.state_mut().side_cursor = i;
    press(h, &[Action::Accept, Action::Right, Action::Accept]);
    assert_eq!(h.state().pane().loc, Location::Steam(crate::steam::View::Games));
    h.state_mut().run(Cmd::SetView(ViewMode::Grid));
    press(h, &[Action::Right]);
    snap(h, "10-steam");
    h.state_mut().run(Cmd::SetView(ViewMode::List));

    // The on-screen keyboard, renaming.
    go(h, &home.join("Documents"));
    let i = at(h, "trip-plan.odt");
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().run(Cmd::Rename);
    settle(h);
    assert!(matches!(h.state().overlay, Some(Overlay::Osk(_))));
    snap(h, "11-keyboard");
    press(h, &[Action::Back]);

    // Settings, then the light look.
    h.state_mut().run(Cmd::Settings);
    snap(h, "12-settings");
    if let Some(Overlay::Settings(st)) = h.state_mut().overlay.as_mut() {
        st.row = row::APPEARANCE;
    }
    press(h, &[Action::Right, Action::Right]);
    assert_eq!(h.state().cfg.appearance, crate::appearance::LIGHT);
    press(h, &[Action::Back]);
    go(h, &home.join("Pictures"));
    let i = at(h, "dunes.png");
    h.state_mut().pane_mut().set_cursor(i);
    h.state_mut().pane_mut().selected.insert("dunes.png".into());
    h.state_mut().pane_mut().selected.insert("meadow.png".into());
    snap(h, "13-light");
}

/// A landscape: sky, sun, two ridges of hills, as a demo picture.
fn demo_picture(path: &std::path::Path, sky: [[f32; 3]; 2], hills: [[f32; 3]; 2], sun: f32) {
    let (w, hgt) = (640u32, 400u32);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * t) as u8);
    image::RgbImage::from_fn(w, hgt, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / hgt as f32);
        let far = 0.55 + 0.08 * (fx * 7.0 + sun * 3.0).sin() + 0.04 * (fx * 17.0).sin();
        let near = 0.72 + 0.07 * (fx * 4.0 + 1.0 + sun).sin() + 0.03 * (fx * 23.0).cos();
        let d = ((fx - sun).powi(2) * 2.6 + (fy - 0.38).powi(2)).sqrt();
        if fy > near {
            image::Rgb(mix(hills[1], [10.0, 10.0, 18.0], (fy - near) * 1.5))
        } else if fy > far {
            image::Rgb(mix(hills[0], hills[1], 0.3))
        } else if d < 0.09 {
            image::Rgb([255, 236, 190])
        } else {
            let glow = (0.25 - d).max(0.0) * 1.6;
            image::Rgb(mix(mix(sky[0], sky[1], fy / far).map(f32::from), [255.0, 220.0, 170.0], glow))
        }
    })
    .save(path)
    .unwrap();
}

fn demo_home(home: &std::path::Path) {
    let w = |p: &str, s: &[u8]| {
        let p = home.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    };
    let big = |p: &str, mib: usize| w(p, &vec![0x5a; mib << 20]);
    for d in ["Desktop", "Documents", "Downloads", "Music", "Pictures", "Videos", "Projects", "Games/Saves"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    w("Desktop/todo.txt", b"- water the plants\n- back up photos\n");
    w("Documents/trip-plan.odt", &[0x50; 18_000]);
    w("Documents/budget-2026.ods", &[0x50; 24_000]);
    w("Documents/recipes.md", b"# Recipes\n\n## Pancakes\n\n- flour\n- milk\n- eggs\n");
    w("Documents/manual.pdf", b"%PDF-1.4\n%demo\n");
    big("Documents/scans/passport-copy.pdf", 2);
    big("Music/Night Drive/01 - Neon Coast.flac", 9);
    big("Music/Night Drive/02 - Overpass.flac", 8);
    big("Music/Night Drive/03 - Last Exit.flac", 11);
    big("Music/podcast-episode-12.mp3", 6);
    big("Videos/summer-trip.mkv", 48);
    big("Videos/cat-vs-laser.mp4", 14);
    big("Games/Saves/backup-2026-09.tar", 5);
    w(".bashrc", b"# demo\n");
    let pics: [(&str, [[f32; 3]; 2], [[f32; 3]; 2], f32); 8] = [
        ("dunes.png", [[250.0, 170.0, 90.0], [255.0, 225.0, 160.0]], [[200.0, 120.0, 60.0], [150.0, 80.0, 40.0]], 0.7),
        ("lagoon.png", [[60.0, 150.0, 220.0], [170.0, 225.0, 245.0]], [[40.0, 140.0, 130.0], [20.0, 90.0, 80.0]], 0.25),
        ("meadow.png", [[120.0, 190.0, 240.0], [220.0, 240.0, 255.0]], [[110.0, 170.0, 70.0], [60.0, 120.0, 40.0]], 0.5),
        ("aurora.png", [[10.0, 30.0, 60.0], [40.0, 160.0, 130.0]], [[30.0, 40.0, 70.0], [15.0, 20.0, 40.0]], 0.85),
        ("sunset.png", [[90.0, 50.0, 130.0], [250.0, 140.0, 90.0]], [[70.0, 40.0, 90.0], [40.0, 20.0, 60.0]], 0.4),
        ("glacier.png", [[150.0, 190.0, 230.0], [235.0, 245.0, 255.0]], [[200.0, 220.0, 240.0], [120.0, 150.0, 190.0]], 0.15),
        ("canyon.png", [[230.0, 130.0, 80.0], [250.0, 200.0, 140.0]], [[170.0, 70.0, 40.0], [110.0, 40.0, 30.0]], 0.6),
        ("forest.png", [[90.0, 140.0, 120.0], [200.0, 220.0, 190.0]], [[40.0, 90.0, 60.0], [20.0, 50.0, 35.0]], 0.3),
    ];
    for (name, sky, hills, sun) in pics {
        demo_picture(&home.join("Pictures").join(name), sky, hills, sun);
    }
    // A project to read and to list in the terminal.
    let p = "Projects/starmap";
    w(&format!("{p}/Cargo.toml"), b"[package]\nname = \"starmap\"\nversion = \"0.3.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n");
    w(
        &format!("{p}/README.md"),
        b"# Starmap\n\nA tiny **night sky** planner: which constellations are up tonight, and when.\n\n> [!TIP]\n> Run it with `--city` to skip the location prompt.\n\n## Features\n\n- [x] Rise and set times\n- [x] Moon phase\n- [ ] Planet tracking\n\n| Key | Action |\n|:--|:--|\n| `n` | Next night |\n| `p` | Previous night |\n| `q` | Quit |\n\n```sh\ncargo run -- --city Lisbon\n```\n",
    );
    w(
        &format!("{p}/src/main.rs"),
        b"//! Starmap: what's up in the sky tonight.\n\nmod stars;\n\nuse std::env;\n\nuse stars::{Catalog, Sky};\n\nfn main() {\n    let city = env::args().skip_while(|a| a != \"--city\").nth(1).unwrap_or_else(|| \"Lisbon\".into());\n    let catalog = Catalog::load(\"data/stars.csv\").expect(\"no star catalog\");\n    let sky = Sky::tonight(&city);\n    // Brightest first, only what clears the horizon.\n    let mut up: Vec<_> = catalog.iter().filter(|s| sky.altitude(s) > 5.0).collect();\n    up.sort_by(|a, b| a.magnitude.total_cmp(&b.magnitude));\n    for star in up.iter().take(10) {\n        println!(\"{:<12} {:>5.1}\\u{b0}  mag {:.1}\", star.name, sky.altitude(star), star.magnitude);\n    }\n    println!(\"{} stars above {city}\", up.len());\n}\n",
    );
    w(
        &format!("{p}/src/stars.rs"),
        b"pub struct Star {\n    pub name: String,\n    pub magnitude: f32,\n}\n\npub struct Catalog(Vec<Star>);\n\nimpl Catalog {\n    pub fn load(path: &str) -> std::io::Result<Self> {\n        let text = std::fs::read_to_string(path)?;\n        Ok(Catalog(text.lines().filter_map(parse).collect()))\n    }\n    pub fn iter(&self) -> impl Iterator<Item = &Star> {\n        self.0.iter()\n    }\n}\n\nfn parse(line: &str) -> Option<Star> {\n    let (name, mag) = line.split_once(',')?;\n    Some(Star { name: name.into(), magnitude: mag.trim().parse().ok()? })\n}\n",
    );
    w(&format!("{p}/data/stars.csv"), b"Sirius,-1.46\nCanopus,-0.74\nArcturus,-0.05\nVega,0.03\n");
    // Downloads: a pacman package (the real thing, .PKGINFO inside), a
    // flatpak, a script, an archive of the pictures.
    let src = home.join(".cache/pkg-src");
    std::fs::create_dir_all(src.join("usr/bin")).unwrap();
    std::fs::write(src.join(".PKGINFO"), "pkgname = aurora-notes\npkgver = 2.4.1-1\n").unwrap();
    std::fs::write(src.join("usr/bin/aurora-notes"), "#!/bin/sh\n").unwrap();
    let dl = home.join("Downloads");
    let ok = std::process::Command::new("tar").arg("--zstd").arg("-cf").arg(dl.join("aurora-notes-2.4.1-1-x86_64.pkg.tar.zst")).arg("-C").arg(&src).args([".PKGINFO", "usr"]).status().unwrap();
    assert!(ok.success());
    std::fs::remove_dir_all(home.join(".cache")).unwrap();
    std::fs::write(dl.join("Pixel-Garden.flatpak"), vec![0x5au8; 300_000]).unwrap();
    std::fs::write(dl.join("setup-printer.sh"), "#!/bin/sh\necho configuring\n").unwrap();
    std::fs::write(dl.join("invoice-0815.pdf"), b"%PDF-1.4\n").unwrap();
    let ok = std::process::Command::new("bsdtar").arg("--format").arg("zip").arg("-cf").arg(dl.join("wallpapers.zip")).arg("-C").arg(home).arg("Pictures").status().unwrap();
    assert!(ok.success());
}

/// Stand-ins for sudo and pacman, for the install window.
fn demo_bin(bin: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(bin).unwrap();
    let script = |name: &str, body: &str| {
        std::fs::write(bin.join(name), body).unwrap();
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("sudo", "#!/bin/sh\nexec \"$@\"\n");
    script(
        "pacman",
        "#!/bin/sh\necho 'loading packages...'\necho 'resolving dependencies...'\necho 'looking for conflicting packages...'\necho\necho 'Package (1)       New Version  Net Change'\necho\necho 'aurora-notes      2.4.1-1        3.21 MiB'\necho\necho 'Total Installed Size:  3.21 MiB'\necho\nprintf ':: Proceed with installation? [Y/n] '\nread a\n[ \"$a\" = n ] && exit 1\nexit 0\n",
    );
}

/// A Steam with made-up games, each with an icon.
fn demo_steam(root: &std::path::Path) {
    let w = |p: std::path::PathBuf, s: &[u8]| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    };
    w(root.join("steamapps/libraryfolders.vdf"), format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", root.display()).as_bytes());
    let games: [(u32, &str, [u8; 3], [u8; 3]); 6] = [
        (910010, "Starfall Drift", [40, 60, 160], [250, 200, 80]),
        (910020, "Hollow Lantern", [30, 30, 40], [250, 160, 60]),
        (910030, "Crimson Orbit", [120, 20, 40], [250, 120, 120]),
        (910040, "Tiny Harbor", [40, 140, 180], [250, 250, 250]),
        (910050, "Moss & Marrow", [40, 90, 50], [190, 230, 120]),
        (910060, "Neon Courier", [60, 20, 90], [80, 240, 230]),
    ];
    for (i, (id, name, bg, fg)) in games.into_iter().enumerate() {
        w(root.join(format!("steamapps/appmanifest_{id}.acf")), format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{name}\"\n}}\n").as_bytes());
        w(root.join(format!("steamapps/common/{name}/data.pak")), b"demo");
        std::fs::create_dir_all(root.join(format!("steamapps/compatdata/{id}/pfx/drive_c"))).unwrap();
        // An emblem: a ring, a dot, a bar, by game.
        let icon = root.join(format!("appcache/librarycache/{id}/{:040x}.jpg", id as u128 * 7919));
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(128, 128, |x, y| {
            let (dx, dy) = (x as f32 - 64.0, y as f32 - 64.0);
            let r = (dx * dx + dy * dy).sqrt();
            // Distance outside the shape, in pixels: soft edges.
            let out = match i % 3 {
                0 => (r - 44.0).max(28.0 - r),
                1 => (r - 26.0).min((dy.abs() - 6.0).max(dx.abs() - 52.0)),
                _ => ((dx + dy).abs() / 1.414 - 10.0).min(r - 16.0),
            };
            let t = (0.5 - out).clamp(0.0, 1.0);
            let shade = 1.0 - y as f32 / 280.0;
            image::Rgb([0, 1, 2].map(|k| (bg[k] as f32 * shade * (1.0 - t) + fg[k] as f32 * t) as u8))
        })
        .save(&icon)
        .unwrap();
    }
}

/// A small menu, then the app menu: it shows all its items (an area once
/// kept the small one's size and the app menu showed two).
#[test]
#[ignore]
fn menu_size_screens() {
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    settle(&mut h);
    h.state_mut().focus_sidebar();
    let i = h.state().side_items.iter().position(|it| it.term).unwrap();
    h.state_mut().side_cursor = i;
    press(&mut h, &[Action::Actions]);
    press(&mut h, &[Action::Back]);
    h.state_mut().focus = crate::app::Focus::Panes;
    press(&mut h, &[Action::Menu]);
    let (id, n) = match &h.state().overlay {
        Some(o @ crate::app::Overlay::Menu(m)) => (egui::Id::new(("overlay", Some(std::mem::discriminant(o)), Some((m.title.clone(), m.items.len())))), m.items.len()),
        _ => panic!("no app menu"),
    };
    let rect = h.state().ctx.memory(|m| m.area_rect(id)).expect("no area");
    assert!(rect.height() >= n as f32 * 34.0, "app menu squeezed to {} for {n} items", rect.height());
    shot(&mut h, "m0-app-menu-after-small");
}

/// "Show in folder" from another program: the folder in a new tab with the
/// file under the cursor; several files selected; a folder already shown
/// reuses its tab; properties.
#[test]
#[ignore]
fn show_in_folder_screens() {
    use crate::fm1::Show;
    let _shared = GLOBALS.read().unwrap_or_else(|e| e.into_inner());
    let dir = std::fs::canonicalize(std::env::var_os("POLKA_TEST_DIR").expect("POLKA_TEST_DIR")).unwrap();
    let work = dir.join("shown");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("sub")).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(work.join(n), n).unwrap();
    }
    let mut h = harness(dir.clone());
    settle(&mut h);
    let tabs = h.state().tabs.len();
    h.state_mut().show_from_dbus(Show::Items(vec![work.join("b.txt")]), "");
    settle(&mut h);
    assert_eq!(h.state().tabs.len(), tabs + 1);
    assert_eq!(h.state().pane().loc, crate::vfs::Location::Local(work.clone()));
    assert_eq!(h.state().pane().current().unwrap().name, "b.txt");
    shot(&mut h, "fm0-show-item");
    // Same folder again: same tab; two files: both selected.
    h.state_mut().show_from_dbus(Show::Items(vec![work.join("c.txt"), work.join("a.txt")]), "");
    settle(&mut h);
    assert_eq!(h.state().tabs.len(), tabs + 1);
    assert_eq!(h.state().pane().current().unwrap().name, "c.txt");
    let mut sel: Vec<_> = h.state().pane().selected.iter().cloned().collect();
    sel.sort();
    assert_eq!(sel, vec!["a.txt", "c.txt"]);
    // A folder opens itself.
    h.state_mut().show_from_dbus(Show::Folders(vec![work.join("sub")]), "");
    settle(&mut h);
    assert_eq!(h.state().tabs.len(), tabs + 2);
    assert_eq!(h.state().pane().loc, crate::vfs::Location::Local(work.join("sub")));
    // Properties.
    h.state_mut().show_from_dbus(Show::Properties(vec![work.join("a.txt")]), "");
    settle(&mut h);
    assert!(matches!(h.state().overlay, Some(crate::app::Overlay::Props(_))));
    assert_eq!(h.state().pane().current().unwrap().name, "a.txt");
    shot(&mut h, "fm1-properties");
    // Started by D-Bus: the first request takes the first tab.
    let mut h = harness(dir);
    settle(&mut h);
    h.state_mut().dbus_fresh = true;
    h.state_mut().show_from_dbus(Show::Items(vec![work.join("a.txt")]), "");
    settle(&mut h);
    assert_eq!(h.state().tabs.len(), 1);
    assert_eq!(h.state().pane().current().unwrap().name, "a.txt");
}
