//! Black-box integration test: exercises comhad's public `config`/`keys` API the way
//! `run_app` does at startup, write a realistic `~/.comhad/` tree to a tempdir, load it
//! through the same functions `main` calls, and check the pieces compose correctly. No
//! mocking: real files, real TOML/JSON parsing, real keybind-table construction.

use std::fs;

use comhad::config;
use comhad::keys::{BrowserAction, ConnPickerAction, Keybinds};
use crossterm::event::KeyCode;

#[test]
fn a_full_comhad_dir_loads_config_and_bookmarks_together() {
    let home = tempfile::tempdir().expect("tempdir");
    let comhad_dir = home.path().join(".comhad");
    let bookmarks_dir = comhad_dir.join("bookmarks");
    fs::create_dir_all(&bookmarks_dir).expect("mkdir bookmarks");

    fs::write(
        comhad_dir.join("config.toml"),
        r##"
            [defaults]
            show_local = true
            show_preview = false

            [theme]
            mode = "dark"

            [theme.dark]
            accent = "#ff8800"

            [keybinds.browser]
            quit = "Q"

            [keybinds.connection_picker]
            select = "l"
        "##,
    )
    .expect("write config.toml");

    fs::write(
        bookmarks_dir.join("work.json"),
        r#"{
            "name": "work",
            "server": "s3.example.com",
            "access_key_id": "AKIDEXAMPLE",
            "secret_access_key": "${COMHAD_INTEGRATION_TEST_SECRET}",
            "path": "work-bucket/data"
        }"#,
    )
    .expect("write work.json");
    fs::write(
        bookmarks_dir.join("personal.json"),
        r#"{
            "name": "personal",
            "server": "s3.example.com",
            "access_key_id": "AKIDPERSONAL",
            "secret_access_key": "literal-secret",
            "path": "personal-bucket"
        }"#,
    )
    .expect("write personal.json");

    // SAFETY: unique-enough var name for this test; no other test reads or writes it.
    unsafe { std::env::set_var("COMHAD_INTEGRATION_TEST_SECRET", "resolved-secret") };
    let connections = config::load_connections_from(&bookmarks_dir).expect("load_connections_from");
    unsafe { std::env::remove_var("COMHAD_INTEGRATION_TEST_SECRET") };

    // Loaded alphabetically by file path, and env-var interpolation resolved.
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[0].1.name, "personal");
    assert_eq!(connections[1].1.name, "work");
    assert_eq!(connections[1].1.secret_access_key, "resolved-secret");
    assert_eq!(connections[1].1.bucket_and_prefix(), ("work-bucket".to_string(), "data/".to_string()));

    let app_config = config::load_app_config_from(&comhad_dir.join("config.toml")).expect("load_app_config_from");
    assert_eq!(app_config.defaults.show_local, Some(true));
    assert_eq!(app_config.defaults.show_preview, Some(false));
    assert_eq!(app_config.theme.mode.as_deref(), Some("dark"));

    let binds = Keybinds::load(&app_config.keybinds);
    // Overridden actions land on their new key...
    assert_eq!(binds.browser.get(&KeyCode::Char('Q')), Some(&BrowserAction::Quit));
    assert_eq!(binds.conn_picker.get(&KeyCode::Char('l')), Some(&ConnPickerAction::Select));
    // ...while every action nobody touched keeps comhad's built-in default.
    assert_eq!(binds.browser.get(&KeyCode::Char('t')), Some(&BrowserAction::ToggleTheme));
    assert_eq!(binds.conn_picker.get(&KeyCode::Char('a')), Some(&ConnPickerAction::AddBookmark));
    assert!(binds.bucket_picker.contains_key(&KeyCode::Char('q')));
}

#[test]
fn a_missing_comhad_dir_falls_back_to_every_built_in_default() {
    let home = tempfile::tempdir().expect("tempdir");
    let comhad_dir = home.path().join(".comhad"); // deliberately never created

    let app_config = config::load_app_config_from(&comhad_dir.join("config.toml")).expect("load_app_config_from");
    assert!(app_config.defaults.show_local.is_none());
    assert!(app_config.theme.mode.is_none());

    let binds = Keybinds::load(&app_config.keybinds);
    assert_eq!(binds.browser.get(&KeyCode::Char('q')), Some(&BrowserAction::Quit));
}
