//! The add / edit / delete bookmark wizard for [`App`].

use std::path::PathBuf;

use super::{App, Prompt, PromptKind};
use crate::config::Connection;

/// `(label, secret, optional)` for each field collected by the add/edit bookmark wizard, in
/// the order they're asked.
pub const BOOKMARK_FIELDS: [(&str, bool, bool); 8] = [
    ("protocol (s3 / s3_private_link, default: s3)", false, true),
    ("name", false, false),
    ("server", false, false),
    ("access_key_id", false, false),
    ("secret_access_key", true, false),
    ("path (bucket or bucket/prefix)", false, false),
    ("web_url (optional)", false, true),
    ("region (optional)", false, true),
];

pub struct BookmarkWizard {
    /// `Some(path)` to overwrite an existing bookmark; `None` derives a filename from the name field.
    pub editing_path: Option<String>,
    pub values: Vec<String>,
    pub field_index: usize,
}

impl App {
    pub fn start_add_bookmark(&mut self) {
        self.bookmark_wizard = Some(BookmarkWizard {
            editing_path: None,
            values: vec![String::new(); BOOKMARK_FIELDS.len()],
            field_index: 0,
        });
        self.open_bookmark_field_prompt();
    }

    pub fn start_edit_bookmark(&mut self, index: usize) {
        let Some((path, conn)) = self.connections.get(index).cloned() else {
            return;
        };
        // protocol, name, server, access_key_id, secret_access_key, path, web_url, region
        let values = vec![
            conn.protocol.clone().unwrap_or_default(),
            conn.name.clone(),
            conn.server.clone(),
            conn.access_key_id.clone(),
            String::new(), // secret left blank; blank on save means "keep the existing one"
            conn.path.clone(),
            conn.web_url.clone().unwrap_or_default(),
            conn.region.clone().unwrap_or_default(),
        ];
        self.bookmark_wizard = Some(BookmarkWizard { editing_path: Some(path), values, field_index: 0 });
        self.open_bookmark_field_prompt();
    }

    fn open_bookmark_field_prompt(&mut self) {
        let Some(wizard) = &self.bookmark_wizard else { return };
        let (_, secret, _) = BOOKMARK_FIELDS[wizard.field_index];
        let buffer = wizard.values[wizard.field_index].clone();
        self.prompt = Some(Prompt { kind: PromptKind::BookmarkField, cursor: buffer.len(), buffer, mask: secret });
    }

    /// Advances to the next wizard field, or saves once the last field is submitted.
    pub fn submit_bookmark_field(&mut self, value: String) {
        let Some(wizard) = &mut self.bookmark_wizard else { return };
        wizard.values[wizard.field_index] = value;
        wizard.field_index += 1;
        if wizard.field_index < wizard.values.len() {
            self.open_bookmark_field_prompt();
        } else {
            self.save_bookmark();
        }
    }

    fn save_bookmark(&mut self) {
        let Some(wizard) = self.bookmark_wizard.take() else { return };
        let [protocol, name, server, access_key_id, secret_access_key, path, web_url, region] =
            match <[String; 8]>::try_from(wizard.values) {
                Ok(v) => v,
                Err(_) => return,
            };

        let secret_access_key = if secret_access_key.is_empty() {
            match &wizard.editing_path {
                Some(path) => self
                    .connections
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, c)| c.secret_access_key.clone())
                    .unwrap_or_default(),
                None => String::new(),
            }
        } else {
            secret_access_key
        };

        let conn = Connection {
            name,
            server,
            access_key_id,
            secret_access_key,
            path,
            web_url: if web_url.is_empty() { None } else { Some(web_url) },
            region: if region.is_empty() { None } else { Some(region) },
            protocol: if protocol.is_empty() { None } else { Some(protocol) },
            force_path_style: None,
        };

        let target_path = match &wizard.editing_path {
            Some(p) => PathBuf::from(p),
            None => match crate::config::bookmarks_dir() {
                Ok(dir) => dir.join(format!("{}.json", slugify(&conn.name))),
                Err(err) => {
                    self.set_status(format!("failed to save bookmark: {err}"), true);
                    return;
                }
            },
        };

        match crate::config::write_bookmark(&target_path, &conn) {
            Ok(()) => {
                let path_str = target_path.display().to_string();
                if let Some(existing) = self.connections.iter_mut().find(|(p, _)| p == &path_str) {
                    existing.1 = conn;
                } else {
                    self.connections.push((path_str, conn));
                    self.connections.sort_by(|a, b| a.0.cmp(&b.0));
                }
                self.set_status("bookmark saved", false);
            }
            Err(err) => self.set_status(format!("failed to save bookmark: {err}"), true),
        }
    }

    pub fn start_delete_bookmark(&mut self, index: usize) {
        if let Some((path, _)) = self.connections.get(index) {
            self.confirm_bookmark_delete = Some(path.clone());
        }
    }

    pub fn cancel_delete_bookmark(&mut self) {
        self.confirm_bookmark_delete = None;
    }

    pub fn confirm_delete_bookmark_now(&mut self) {
        let Some(path) = self.confirm_bookmark_delete.take() else { return };
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.connections.retain(|(p, _)| p != &path);
                if self.conn_selected >= self.connections.len() {
                    self.conn_selected = self.connections.len().saturating_sub(1);
                }
                self.set_status("bookmark deleted", false);
            }
            Err(err) => self.set_status(format!("failed to delete bookmark: {err}"), true),
        }
    }
}

/// Turns a bookmark name into a safe filename stem, e.g. `"HELLO world!"` -> `"hello_world"`.
fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = slug.trim_matches('_');
    if trimmed.is_empty() { "bookmark".to_string() } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        App::new(Vec::new(), ratatui_image::picker::Picker::halfblocks(), crate::config::AppConfig::default())
    }

    #[test]
    fn slugify_lowercases_and_replaces_non_alphanumerics() {
        assert_eq!(slugify("HELLO world!"), "hello_world");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_underscores() {
        assert_eq!(slugify("  spaced out  "), "spaced_out");
    }

    #[test]
    fn slugify_falls_back_when_nothing_alphanumeric_survives() {
        assert_eq!(slugify("!!!"), "bookmark");
    }

    #[test]
    fn start_add_bookmark_opens_the_first_field_prompt() {
        let mut app = test_app();
        app.start_add_bookmark();
        let wizard = app.bookmark_wizard.as_ref().expect("wizard started");
        assert_eq!(wizard.field_index, 0);
        assert!(wizard.editing_path.is_none());
        assert!(app.prompt.is_some());
    }

    #[test]
    fn submit_bookmark_field_advances_to_the_next_field() {
        let mut app = test_app();
        app.start_add_bookmark();
        app.submit_bookmark_field("s3".to_string());
        let wizard = app.bookmark_wizard.as_ref().expect("wizard still open");
        assert_eq!(wizard.field_index, 1);
        assert_eq!(wizard.values[0], "s3");
    }

    #[test]
    fn start_edit_bookmark_prefills_values_but_blanks_the_secret() {
        let mut app = test_app();
        let conn = Connection {
            name: "work".to_string(),
            server: "s3.example.com".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "sekret".to_string(),
            path: "bucket".to_string(),
            web_url: None,
            region: None,
            protocol: None,
            force_path_style: None,
        };
        app.connections.push(("work.json".to_string(), conn));
        app.start_edit_bookmark(0);
        let wizard = app.bookmark_wizard.as_ref().expect("wizard started");
        assert_eq!(wizard.editing_path, Some("work.json".to_string()));
        assert_eq!(wizard.values[1], "work");
        assert_eq!(wizard.values[4], ""); // secret_access_key left blank
    }
}
