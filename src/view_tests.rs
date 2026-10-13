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
    compress_dialog(h);
    volumes(h);
    viewer(h);
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
        "# Polka\n\nФайловый менеджер для **геймпада**, с [ссылкой](https://example.org).\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n- пункт\n- `код`\n",
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
                Some(Content::Text(t)) => t.doc.lang.is_none() || t.doc.spans.get().is_some(),
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
