//! The add / edit / delete bookmark wizard for [`App`].

use std::path::PathBuf;

use super::{App, Prompt, PromptKind};
use crate::config::Connection;

/// `(label, secret, optional)` for each field collected by the add/edit bookmark wizard, in
/// the order they're asked.
pub const BOOKMARK_FIELDS: [(&str, bool, bool); 9] = [
    ("profile (s3 / s3_private_link, default: s3)", false, true),
    ("name", false, false),
    ("endpoint", false, false),
    ("access_key_id", false, false),
    ("secret_access_key", true, false),
    ("remote_path (bucket or bucket/prefix)", false, false),
    ("local_path (optional, e.g. ~/work/exports)", false, true),
    ("web_url (optional)", false, true),
    ("region (optional)", false, true),
];

pub struct BookmarkWizard {
    /// `Some(path)` to overwrite an existing bookmark; `None` derives a filename from the name field.
    pub editing_path: Option<String>,
    pub values: Vec<String>,
    pub field_index: usize,
}

/// Short field labels for the inline editor, indexed to match [`BOOKMARK_FIELDS`]. The wizard's
/// own labels carry hint text that would be noise once each field is laid out on its own line.
pub const EDITOR_LABELS: [&str; BOOKMARK_FIELDS.len()] = [
    "Profile",
    "Name",
    "Endpoint",
    "Access key",
    "Secret key",
    "Remote path",
    "Local path",
    "Web URL",
    "Region",
];

/// Editor field indices that hold credentials (`access_key_id`, `secret_access_key`). These are
/// designated explicitly by the S3 schema, the same way Cyberduck's protocol profile declares
/// which field is the password, rather than guessed from the field name. They're shown masked,
/// never pre-filled with the stored value (which is already env-interpolated in memory), and left
/// blank to mean "keep what's on disk" (preserving any `${VAR}` reference) unless retyped.
pub const CREDENTIAL_FIELDS: [usize; 2] = [3, 4];

pub fn is_credential_field(index: usize) -> bool {
    CREDENTIAL_FIELDS.contains(&index)
}

/// Value indices the inline editor exposes, in display order. Profile (index 0) is intentionally
/// omitted (it's changed via the picker's rename-profile action `r`, not this dialog), but the
/// underlying `values` vec keeps all nine slots so profile round-trips through save untouched.
pub const EDITOR_FIELDS: [usize; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// Which control the inline bookmark editor's focus is on: one of the value fields, or one of the
/// two buttons at the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorFocus {
    Field(usize),
    Save,
    Discard,
}

/// Inline "edit this bookmark's fields in place" editor, an alternative to the step-by-step
/// [`BookmarkWizard`]. Reuses the same field order and [`BOOKMARK_FIELDS`] metadata, but every
/// field is visible and editable at once inside the connection picker's info pane (or, on a
/// terminal too narrow for that pane, an overlay popup).
pub struct BookmarkEditor {
    /// The bookmark being overwritten. Always `Some`, the editor only ever opens on an existing
    /// bookmark (add still goes through the wizard).
    pub editing_path: Option<String>,
    pub values: Vec<String>,
    pub focus: EditorFocus,
    /// Char cursor position within the focused field's value; ignored while a button is focused.
    pub cursor: usize,
    /// Whether credential fields are un-masked (toggled with `ctrl+r`). Off by default so secrets
    /// stay hidden; revealing also shows `${VAR}` references so you can check what you typed.
    pub reveal_secrets: bool,
}

impl BookmarkEditor {
    /// Linear position of the current focus for up/down movement: the exposed fields in
    /// [`EDITOR_FIELDS`] order, then `Save`, then `Discard`.
    fn focus_index(&self) -> usize {
        match self.focus {
            EditorFocus::Field(i) => EDITOR_FIELDS.iter().position(|&f| f == i).unwrap_or(0),
            EditorFocus::Save => EDITOR_FIELDS.len(),
            EditorFocus::Discard => EDITOR_FIELDS.len() + 1,
        }
    }

    fn focus_from_index(&self, index: usize) -> EditorFocus {
        let n = EDITOR_FIELDS.len();
        if index < n {
            EditorFocus::Field(EDITOR_FIELDS[index])
        } else if index == n {
            EditorFocus::Save
        } else {
            EditorFocus::Discard
        }
    }

    /// Char length of the field the cursor currently sits in, or 0 while on a button.
    fn current_len(&self) -> usize {
        match self.focus {
            EditorFocus::Field(i) => self.values[i].chars().count(),
            _ => 0,
        }
    }

    /// Snaps the text cursor to the end of the focused field, called whenever focus lands on a
    /// (possibly different) field.
    fn cursor_to_end(&mut self) {
        self.cursor = self.current_len();
    }
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

    /// Opens the inline editor on the selected bookmark, pre-filled with its current values. Focus
    /// starts on the first field with the text cursor at its end. Unlike the wizard, every field is
    /// shown and editable at once inside the info pane (or an overlay when the pane is hidden).
    pub fn start_edit_bookmark(&mut self, index: usize) {
        let Some((path, conn)) = self.connections.get(index).cloned() else {
            return;
        };
        // profile, name, endpoint, access_key_id, secret_access_key, remote_path, local_path, web_url, region
        // Credential fields start blank (see `CREDENTIAL_FIELDS`): the in-memory values are the
        // env-interpolated real secrets, so showing or re-saving them would leak the secret and
        // clobber any `${VAR}` reference. Blank means "keep what's on disk" unless retyped.
        let values = vec![
            conn.profile.clone().unwrap_or_default(),
            conn.name.clone(),
            conn.endpoint.clone(),
            String::new(), // access_key_id, kept unless retyped
            String::new(), // secret_access_key, kept unless retyped
            conn.remote_path.clone(),
            conn.local_path.clone().unwrap_or_default(),
            conn.web_url.clone().unwrap_or_default(),
            conn.region.clone().unwrap_or_default(),
        ];
        let first = EDITOR_FIELDS[0];
        let cursor = values[first].chars().count();
        self.bookmark_editor = Some(BookmarkEditor {
            editing_path: Some(path),
            values,
            focus: EditorFocus::Field(first),
            cursor,
            reveal_secrets: false,
        });
    }

    /// Toggles plaintext display of the editor's credential fields (`ctrl+r`).
    pub fn toggle_editor_reveal(&mut self) {
        if let Some(editor) = &mut self.bookmark_editor {
            editor.reveal_secrets = !editor.reveal_secrets;
        }
    }

    /// Moves editor focus by `delta` rows through the fields and the two buttons (negative goes
    /// up), clamped at both ends. Landing on a field snaps the text cursor to its end.
    pub fn editor_move(&mut self, delta: i32) {
        let Some(editor) = &mut self.bookmark_editor else { return };
        let last = EDITOR_FIELDS.len() + 1; // Save = len, Discard = len + 1
        let next = (editor.focus_index() as i32 + delta).clamp(0, last as i32) as usize;
        editor.focus = editor.focus_from_index(next);
        editor.cursor_to_end();
    }

    /// Whether the editor's focus is on the Save/Discard buttons rather than a text field.
    pub fn editor_on_button(&self) -> bool {
        self.bookmark_editor.as_ref().is_some_and(|e| !matches!(e.focus, EditorFocus::Field(_)))
    }

    /// Horizontal movement: within a field it walks the text cursor; on the button row it flips
    /// between Save and Discard.
    pub fn editor_horizontal(&mut self, delta: i32) {
        let Some(editor) = &mut self.bookmark_editor else { return };
        match editor.focus {
            EditorFocus::Field(_) => {
                let len = editor.current_len();
                editor.cursor = (editor.cursor as i32 + delta).clamp(0, len as i32) as usize;
            }
            EditorFocus::Save if delta > 0 => editor.focus = EditorFocus::Discard,
            EditorFocus::Discard if delta < 0 => editor.focus = EditorFocus::Save,
            _ => {}
        }
    }

    pub fn editor_cursor_home(&mut self) {
        if let Some(editor) = &mut self.bookmark_editor {
            editor.cursor = 0;
        }
    }

    pub fn editor_cursor_end(&mut self) {
        if let Some(editor) = &mut self.bookmark_editor {
            editor.cursor_to_end();
        }
    }

    /// Inserts a typed character at the cursor of the focused field. No-op while a button is focused.
    pub fn editor_insert(&mut self, c: char) {
        let Some(editor) = &mut self.bookmark_editor else { return };
        let EditorFocus::Field(i) = editor.focus else { return };
        let byte = editor.values[i].char_indices().nth(editor.cursor).map(|(b, _)| b).unwrap_or(editor.values[i].len());
        editor.values[i].insert(byte, c);
        editor.cursor += 1;
    }

    /// Deletes the character before the cursor in the focused field (backspace).
    pub fn editor_backspace(&mut self) {
        let Some(editor) = &mut self.bookmark_editor else { return };
        let EditorFocus::Field(i) = editor.focus else { return };
        if editor.cursor == 0 {
            return;
        }
        let byte = editor.values[i].char_indices().nth(editor.cursor - 1).map(|(b, _)| b).unwrap_or(0);
        editor.values[i].remove(byte);
        editor.cursor -= 1;
    }

    /// Deletes the character at the cursor in the focused field (forward delete).
    pub fn editor_delete(&mut self) {
        let Some(editor) = &mut self.bookmark_editor else { return };
        let EditorFocus::Field(i) = editor.focus else { return };
        if let Some((byte, _)) = editor.values[i].char_indices().nth(editor.cursor) {
            editor.values[i].remove(byte);
        }
    }

    /// Handles `enter`: on a field it advances to the next field (or the Save button past the
    /// last); on a button it activates it (save / discard).
    pub fn editor_activate(&mut self) {
        let Some(editor) = &self.bookmark_editor else { return };
        match editor.focus {
            EditorFocus::Field(_) => self.editor_move(1),
            EditorFocus::Save => self.save_bookmark_editor(),
            EditorFocus::Discard => self.cancel_edit_bookmark(),
        }
    }

    /// Discards the inline editor without saving.
    pub fn cancel_edit_bookmark(&mut self) {
        self.bookmark_editor = None;
    }

    /// Persists the inline editor's values and closes it.
    pub fn save_bookmark_editor(&mut self) {
        let Some(editor) = self.bookmark_editor.take() else { return };
        self.persist_bookmark(editor.values, editor.editing_path);
    }

    /// The selected bookmark's current profile value (`""` if unset), to pre-fill the rename prompt.
    pub fn selected_profile(&self) -> String {
        self.connections
            .get(self.conn_selected)
            .and_then(|(_, c)| c.profile.clone())
            .unwrap_or_default()
    }

    /// Changes just the `profile` field of the selected bookmark (the `r` action on the picker),
    /// leaving every other field alone. Writes through the *raw* on-disk copy so `${VAR}` credential
    /// references survive rather than being overwritten with their resolved values.
    pub fn rename_profile(&mut self, value: String) {
        let Some((path, _)) = self.connections.get(self.conn_selected).cloned() else { return };
        let Some(mut conn) = crate::config::read_bookmark_raw(&path) else {
            self.set_status("failed to read bookmark", true);
            return;
        };
        let trimmed = value.trim();
        conn.profile = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
        match crate::config::write_bookmark(&PathBuf::from(&path), &conn) {
            Ok(()) => {
                if let Some((_, existing)) = self.connections.get_mut(self.conn_selected) {
                    existing.profile = conn.profile.clone();
                }
                self.set_status("profile updated", false);
            }
            Err(err) => self.set_status(format!("failed to save bookmark: {err}"), true),
        }
    }

    fn open_bookmark_field_prompt(&mut self) {
        let Some(wizard) = &self.bookmark_wizard else { return };
        let (_, secret, _) = BOOKMARK_FIELDS[wizard.field_index];
        let buffer = wizard.values[wizard.field_index].clone();
        self.prompt = Some(Prompt::new(PromptKind::BookmarkField, buffer, secret));
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
        self.persist_bookmark(wizard.values, wizard.editing_path);
    }

    /// Writes a bookmark to disk from a flat field-value vector, shared by the wizard and the
    /// inline editor. `editing_path` is `Some` to overwrite that file, `None` to create a new one
    /// named after the bookmark. A blank secret means "keep the existing one" when editing.
    fn persist_bookmark(&mut self, values: Vec<String>, editing_path: Option<String>) {
        let [profile, name, endpoint, access_key_id, secret_access_key, remote_path, local_path, web_url, region] =
            match <[String; 9]>::try_from(values) {
                Ok(v) => v,
                Err(_) => return,
            };

        // A blank credential means "keep what's on disk". Read it from the raw file (not
        // `self.connections`, whose copy has already had `${VAR}` interpolated to the real secret)
        // so the un-resolved reference round-trips instead of leaking the resolved value.
        let raw = editing_path.as_deref().and_then(crate::config::read_bookmark_raw);
        let access_key_id = if access_key_id.is_empty() {
            raw.as_ref().map(|c| c.access_key_id.clone()).unwrap_or_default()
        } else {
            access_key_id
        };
        let secret_access_key = if secret_access_key.is_empty() {
            raw.as_ref().map(|c| c.secret_access_key.clone()).unwrap_or_default()
        } else {
            secret_access_key
        };

        let conn = Connection {
            name,
            endpoint,
            access_key_id,
            secret_access_key,
            remote_path,
            local_path: if local_path.is_empty() { None } else { Some(local_path) },
            web_url: if web_url.is_empty() { None } else { Some(web_url) },
            region: if region.is_empty() { None } else { Some(region) },
            profile: if profile.is_empty() { None } else { Some(profile) },
            force_path_style: None,
        };

        let target_path = match &editing_path {
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
    fn start_edit_bookmark_opens_the_inline_editor_prefilled_but_blanks_the_secret() {
        let mut app = test_app();
        let conn = Connection {
            name: "work".to_string(),
            endpoint: "s3.example.com".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "sekret".to_string(),
            remote_path: "bucket".to_string(),
            local_path: None,
            web_url: None,
            region: None,
            profile: None,
            force_path_style: None,
        };
        app.connections.push(("work.json".to_string(), conn));
        app.start_edit_bookmark(0);
        let editor = app.bookmark_editor.as_ref().expect("editor started");
        assert_eq!(editor.editing_path, Some("work.json".to_string()));
        assert_eq!(editor.values[1], "work");
        assert_eq!(editor.values[3], ""); // access_key_id left blank (kept unless retyped)
        assert_eq!(editor.values[4], ""); // secret_access_key left blank
        // Focus starts on the first *exposed* field (Name); Profile (index 0) isn't editable here.
        assert_eq!(editor.focus, EditorFocus::Field(1));
        assert_eq!(editor.cursor, editor.values[1].chars().count());
        // No prompt or wizard: the inline editor replaces the step-by-step edit flow.
        assert!(app.bookmark_wizard.is_none());
        assert!(app.prompt.is_none());
    }

    #[test]
    fn editor_navigation_moves_through_fields_and_buttons() {
        let mut app = test_app();
        app.connections.push(("work.json".to_string(), Connection {
            name: "work".to_string(),
            endpoint: "s3.example.com".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "sekret".to_string(),
            remote_path: "bucket".to_string(),
            local_path: None,
            web_url: None,
            region: None,
            profile: None,
            force_path_style: None,
        }));
        app.start_edit_bookmark(0);
        // Up at the top stays on the first exposed field (Name); Profile is not editable here.
        app.editor_move(-1);
        assert_eq!(app.bookmark_editor.as_ref().unwrap().focus, EditorFocus::Field(1));
        // Down past the last field lands on Save, then Discard, then clamps.
        for _ in 0..EDITOR_FIELDS.len() {
            app.editor_move(1);
        }
        assert_eq!(app.bookmark_editor.as_ref().unwrap().focus, EditorFocus::Save);
        app.editor_move(1);
        assert_eq!(app.bookmark_editor.as_ref().unwrap().focus, EditorFocus::Discard);
        app.editor_move(1);
        assert_eq!(app.bookmark_editor.as_ref().unwrap().focus, EditorFocus::Discard);
        // Left/right flips between the buttons.
        app.editor_horizontal(-1);
        assert_eq!(app.bookmark_editor.as_ref().unwrap().focus, EditorFocus::Save);
    }

    #[test]
    fn untouched_save_preserves_profile_and_the_raw_env_var_credentials() {
        let mut app = test_app();
        let dir = std::env::temp_dir().join(format!("comhad_edit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("work.json");
        let path_str = path.display().to_string();
        // The on-disk credentials are `${VAR}` references; a real load would interpolate these to
        // the resolved secrets in memory. Simulate that: disk holds the reference, memory the value.
        crate::config::write_bookmark(&path, &Connection {
            name: "work".to_string(),
            endpoint: "s3.example.com".to_string(),
            access_key_id: "${MY_KEY}".to_string(),
            secret_access_key: "${MY_SECRET}".to_string(),
            remote_path: "bucket".to_string(),
            local_path: None,
            web_url: None,
            region: None,
            profile: Some("s3_private_link".to_string()),
            force_path_style: None,
        }).unwrap();
        app.connections.push((path_str.clone(), Connection {
            access_key_id: "RESOLVED_KEY".to_string(),   // as if interpolate_env already ran
            secret_access_key: "RESOLVED_SECRET".to_string(),
            ..crate::config::read_bookmark_raw(&path_str).unwrap()
        }));

        app.start_edit_bookmark(0);
        app.save_bookmark_editor(); // touch nothing

        let saved = crate::config::read_bookmark_raw(&path_str).unwrap();
        assert_eq!(saved.profile.as_deref(), Some("s3_private_link")); // profile not cleared
        // The raw `${VAR}` references survive; the resolved secrets are never written back.
        assert_eq!(saved.access_key_id, "${MY_KEY}");
        assert_eq!(saved.secret_access_key, "${MY_SECRET}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toggle_editor_reveal_flips_and_starts_hidden() {
        let mut app = test_app();
        app.connections.push(("work.json".to_string(), Connection {
            name: "work".to_string(),
            endpoint: "s3.example.com".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "sekret".to_string(),
            remote_path: "bucket".to_string(),
            local_path: None,
            web_url: None,
            region: None,
            profile: None,
            force_path_style: None,
        }));
        app.start_edit_bookmark(0);
        assert!(!app.bookmark_editor.as_ref().unwrap().reveal_secrets);
        app.toggle_editor_reveal();
        assert!(app.bookmark_editor.as_ref().unwrap().reveal_secrets);
    }

    #[test]
    fn rename_profile_changes_only_profile_and_preserves_raw_credentials() {
        let mut app = test_app();
        let dir = std::env::temp_dir().join(format!("comhad_profile_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("work.json");
        let path_str = path.display().to_string();
        crate::config::write_bookmark(&path, &Connection {
            name: "work".to_string(),
            endpoint: "s3.example.com".to_string(),
            access_key_id: "${MY_KEY}".to_string(),
            secret_access_key: "${MY_SECRET}".to_string(),
            remote_path: "bucket".to_string(),
            local_path: None,
            web_url: None,
            region: None,
            profile: None,
            force_path_style: None,
        }).unwrap();
        app.connections.push((path_str.clone(), Connection {
            access_key_id: "RESOLVED_KEY".to_string(),
            secret_access_key: "RESOLVED_SECRET".to_string(),
            ..crate::config::read_bookmark_raw(&path_str).unwrap()
        }));
        app.conn_selected = 0;

        assert_eq!(app.selected_profile(), ""); // unset to start
        app.rename_profile("s3_private_link".to_string());

        // In-memory copy updated, and the on-disk file changed profile only.
        assert_eq!(app.connections[0].1.profile.as_deref(), Some("s3_private_link"));
        let saved = crate::config::read_bookmark_raw(&path_str).unwrap();
        assert_eq!(saved.profile.as_deref(), Some("s3_private_link"));
        assert_eq!(saved.access_key_id, "${MY_KEY}");
        assert_eq!(saved.secret_access_key, "${MY_SECRET}");

        // Blank clears it back to unset.
        app.rename_profile("   ".to_string());
        assert_eq!(app.connections[0].1.profile, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn editor_does_not_expose_the_profile_field() {
        assert!(!EDITOR_FIELDS.contains(&0)); // profile (index 0) is omitted
        assert_eq!(EDITOR_FIELDS.len(), BOOKMARK_FIELDS.len() - 1);
    }

    #[test]
    fn credential_fields_are_designated_by_schema_not_name() {
        assert!(is_credential_field(3)); // access_key_id
        assert!(is_credential_field(4)); // secret_access_key
        assert!(!is_credential_field(1)); // name
        assert!(!is_credential_field(7)); // web_url
    }

    #[test]
    fn editor_edits_text_at_the_cursor() {
        let mut app = test_app();
        app.connections.push(("work.json".to_string(), Connection {
            name: "work".to_string(),
            endpoint: "s3.example.com".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "sekret".to_string(),
            remote_path: "bucket".to_string(),
            local_path: None,
            web_url: None,
            region: None,
            profile: None,
            force_path_style: None,
        }));
        app.start_edit_bookmark(0);
        // Focus starts on the name field (index 1); edit it in place.
        assert_eq!(app.bookmark_editor.as_ref().unwrap().focus, EditorFocus::Field(1));
        app.editor_insert('!'); // appends at end -> "work!"
        assert_eq!(app.bookmark_editor.as_ref().unwrap().values[1], "work!");
        app.editor_cursor_home();
        app.editor_insert('~'); // inserts at start -> "~work!"
        assert_eq!(app.bookmark_editor.as_ref().unwrap().values[1], "~work!");
        app.editor_delete(); // removes 'w' at cursor -> "~ork!"
        assert_eq!(app.bookmark_editor.as_ref().unwrap().values[1], "~ork!");
        app.editor_cursor_end();
        app.editor_backspace(); // removes trailing '!' -> "~ork"
        assert_eq!(app.bookmark_editor.as_ref().unwrap().values[1], "~ork");
    }
}
