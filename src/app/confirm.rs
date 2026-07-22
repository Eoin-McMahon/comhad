//! "Are you sure?" confirmation for every write action (upload, download, rename, sync, delete,
//! paste). Each request captures context to build the prompt and carry out the action once
//! confirmed (see the confirm handler in `input`).

use super::{App, Focus};

/// What a pending confirmation will do if the user says yes.
pub enum ConfirmKind {
    Download,
    Upload,
    Rename(String),
    Sync,
    Delete,
    Paste,
}

pub struct ConfirmAction {
    /// The question shown in the popup, may be multiple lines.
    pub prompt: String,
    /// Destination/source path, shown on its own highlighted line below `prompt`.
    pub destination: Option<String>,
    pub kind: ConfirmKind,
    /// Which button is currently highlighted, `tab`/arrows flip it, `enter` activates it.
    pub yes_selected: bool,
}

impl App {
    /// `default_yes` picks the starting button, `false` for irreversible actions (delete) so
    /// `enter` alone can't accidentally confirm one.
    pub(super) fn request_confirm(&mut self, prompt: String, kind: ConfirmKind, default_yes: bool) {
        self.confirm_action = Some(ConfirmAction { prompt, destination: None, kind, yes_selected: default_yes });
    }

    /// Like `request_confirm`, but with a path/location called out on its own line.
    pub(super) fn request_confirm_with_destination(
        &mut self,
        prompt: String,
        destination: String,
        kind: ConfirmKind,
        default_yes: bool,
    ) {
        self.confirm_action =
            Some(ConfirmAction { prompt, destination: Some(destination), kind, yes_selected: default_yes });
    }

    /// Asks before downloading the marked (or hovered) remote object(s).
    pub fn request_confirm_download(&mut self) {
        let count = if self.marked.is_empty() {
            usize::from(self.current_entry().is_some())
        } else {
            self.marked.len()
        };
        if count == 0 {
            self.set_status("nothing selected to download", true);
            return;
        }
        let noun = if count == 1 { "item" } else { "items" };
        let mut prompt = format!("Download {count} {noun} to:");
        // The local pane picks the destination, so spell it out when it's not visible; once
        // shown, the hint just repeats what's on screen.
        if !self.show_local {
            prompt.push_str("\n(press L to browse and pick a different folder first)");
        }
        // A single non-directory item lands at an exact path, so show that rather than just
        // the directory; multiple items or a lone directory zip to a generated name instead.
        let destination = match self.single_download_target() {
            Some(entry) => self.local_cwd.join(&entry.name).display().to_string(),
            None => match (self.marked.is_empty(), self.current_entry()) {
                (true, Some(entry)) => self.local_cwd.join(format!("{}.zip", entry.name)).display().to_string(),
                _ => self.local_cwd.display().to_string(),
            },
        };
        self.request_confirm_with_destination(prompt, destination, ConfirmKind::Download, true);
    }

    /// Asks before uploading the marked (or hovered) local file(s).
    pub fn request_confirm_upload(&mut self) {
        let count = if self.local_marked.is_empty() {
            usize::from(self.current_local_entry().is_some())
        } else {
            self.local_marked.len()
        };
        if count == 0 {
            self.set_status("nothing selected to upload", true);
            return;
        }
        let noun = if count == 1 { "item" } else { "items" };
        let prompt = format!("Upload {count} {noun} to:");
        // Mirrors `request_confirm_download`: a single hovered file uploads to an exact key.
        let destination = match (self.local_marked.is_empty(), self.current_local_entry()) {
            (true, Some(entry)) if !entry.is_dir => format!("s3://{}/{}{}", self.bucket, self.prefix, entry.name),
            (true, Some(entry)) => format!("s3://{}/{}{}/", self.bucket, self.prefix, entry.name),
            _ => format!("s3://{}/{}", self.bucket, self.prefix),
        };
        self.request_confirm_with_destination(prompt, destination, ConfirmKind::Upload, true);
    }

    /// Asks before renaming the hovered item to `new_name`.
    pub fn request_confirm_rename(&mut self, new_name: String) {
        if new_name.is_empty() {
            return;
        }
        let old = match self.focus {
            Focus::Remote => self.current_entry().map(|e| e.name.clone()),
            Focus::Local => self.current_local_entry().map(|e| e.name.clone()),
            Focus::Preview | Focus::Transfers => None,
        };
        let Some(old) = old else { return };
        let prompt = format!("Rename '{old}' to '{new_name}'?");
        self.request_confirm(prompt, ConfirmKind::Rename(new_name), true);
    }

    /// Asks before permanently deleting the marked (or hovered) item(s) in the focused pane.
    pub fn request_confirm_delete(&mut self) {
        let (count, where_) = match self.focus {
            Focus::Remote => {
                let count = if self.marked.is_empty() {
                    usize::from(self.current_entry().is_some())
                } else {
                    self.marked.len()
                };
                (count, format!("s3://{}/{}", self.bucket, self.prefix))
            }
            Focus::Local => {
                let count = if self.local_marked.is_empty() {
                    usize::from(self.current_local_entry().is_some())
                } else {
                    self.local_marked.len()
                };
                (count, self.local_cwd.display().to_string())
            }
            Focus::Preview | Focus::Transfers => (0, String::new()),
        };
        if count == 0 {
            self.set_status("nothing selected to delete", true);
            return;
        }
        let noun = if count == 1 { "item" } else { "items" };
        let prompt = format!("Permanently delete {count} {noun} (no undo) from:");
        self.request_confirm_with_destination(prompt, where_, ConfirmKind::Delete, false);
    }

    /// Asks before running the currently-previewed sync plan.
    pub fn request_confirm_sync(&mut self) {
        let Some(state) = &self.sync else { return };
        let count = state.actionable();
        if count == 0 {
            self.set_status("sync: already up to date", false);
            self.sync = None;
            return;
        }
        let noun = if count == 1 { "file" } else { "files" };
        let prompt = format!("Sync {count} {noun}, {}?", state.direction.label());
        self.request_confirm(prompt, ConfirmKind::Sync, true);
    }
}
