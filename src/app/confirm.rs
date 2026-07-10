//! "Are you sure?" confirmation for every action that writes something — upload, download,
//! rename, and sync. Each request captures enough context to build a human-readable prompt
//! *and* to carry out the action once confirmed (see the confirm handler in `input`).

use super::{App, Focus};

/// What a pending confirmation will do if the user says yes.
pub enum ConfirmKind {
    Download,
    Upload,
    Rename(String),
    Sync,
}

pub struct ConfirmAction {
    /// One-line question shown in the popup.
    pub prompt: String,
    pub kind: ConfirmKind,
}

impl App {
    fn request_confirm(&mut self, prompt: String, kind: ConfirmKind) {
        self.confirm_action = Some(ConfirmAction { prompt, kind });
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
        let prompt = format!("Download {count} {noun} to {}?", self.local_cwd.display());
        self.request_confirm(prompt, ConfirmKind::Download);
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
        let prompt = format!("Upload {count} {noun} to s3://{}/{}?", self.bucket, self.prefix);
        self.request_confirm(prompt, ConfirmKind::Upload);
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
        self.request_confirm(prompt, ConfirmKind::Rename(new_name));
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
        let prompt = format!("Sync {count} {noun} — {}?", state.direction.label());
        self.request_confirm(prompt, ConfirmKind::Sync);
    }
}
