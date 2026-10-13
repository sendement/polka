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
    // Measured again: still in the same place; a new file shows up.
    press(h, &[Action::Right]);
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
    let dir = std::env::var_os("POLKA_TEST_DIR").map(PathBuf::from).expect("POLKA_TEST_DIR");
    let mut h = harness(dir);
    disk_usage(&mut h);
}
