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
    press(h, &[Action::Accept]);
    assert!(h.state().overlay.is_none());
    if target.is_some() {
        assert_eq!(h.state().cfg.icon_theme.as_deref(), Some("breeze"));
    }
    settle(h);
    shot(h, "15-theme-applied");
}
