//! "Are you sure?" confirmation for every write action (upload, download, rename, sync, delete,
//! paste). Each request captures context to build the prompt and carry out the action once
//! confirmed (see the confirm handler in `input`).

use super::{App, Focus};
use crate::jobs;

/// What a pending confirmation will do if the user says yes.
pub enum ConfirmKind {
    /// Carries the name the user picked with `r`, `None` for the default name.
    Download(Option<String>),
    /// Same as `Download`, only ever `Some` for a single-item upload.
    Upload(Option<String>),
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
    /// For a rename: `(old name, new name)`, rendered as an inline old→new diff instead of the
    /// plain quoted names in `prompt`.
    pub rename_diff: Option<(String, String)>,
}

impl App {
    /// `default_yes` picks the starting button, `false` for irreversible actions (delete) so
    /// `enter` alone can't accidentally confirm one.
    pub(super) fn request_confirm(&mut self, prompt: String, kind: ConfirmKind, default_yes: bool) {
        self.confirm_action =
            Some(ConfirmAction { prompt, destination: None, kind, yes_selected: default_yes, rename_diff: None });
    }

    /// Like `request_confirm`, but with a path/location called out on its own line.
    pub(super) fn request_confirm_with_destination(
        &mut self,
        prompt: String,
        destination: String,
        kind: ConfirmKind,
        default_yes: bool,
    ) {
        self.confirm_action = Some(ConfirmAction {
            prompt,
            destination: Some(destination),
            kind,
            yes_selected: default_yes,
            rename_diff: None,
        });
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
        // A single file lands at an exact path; multiple items or a lone directory zip to a
        // generated name instead. Either way the full path is shown, and `r` can change the name.
        let destination = match self.download_target_name() {
            Some(name) => self.local_cwd.join(name).display().to_string(),
            None => self.local_cwd.display().to_string(),
        };
        self.request_confirm_with_destination(prompt, destination, ConfirmKind::Download(None), true);
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
        let destination = self.upload_destination(None);
        self.request_confirm_with_destination(prompt, destination, ConfirmKind::Upload(None), true);
    }

    /// Where an upload lands, as shown in the confirm popup. Mirrors `request_confirm_download`:
    /// a single item uploads to an exact key (under `name` if given), several to the prefix.
    fn upload_destination(&self, name: Option<&str>) -> String {
        match self.single_upload_path() {
            Some(path) => format!(
                "s3://{}/{}",
                self.bucket,
                jobs::upload_destination_key_named(&path, &self.prefix, name)
            ),
            None => format!("s3://{}/{}", self.bucket, self.prefix),
        }
    }

    /// The download or upload target name the pending confirmation would use, the one `r`
    /// offers to rename. `None` when the action has no single name to change.
    pub fn confirm_target_name(&self) -> Option<String> {
        match &self.confirm_action.as_ref()?.kind {
            ConfirmKind::Download(name) => name.clone().or_else(|| self.download_target_name()),
            ConfirmKind::Upload(name) => {
                let path = self.single_upload_path()?;
                name.clone().or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()))
            }
            _ => None,
        }
    }

    /// Applies a name typed after `r` in the confirm popup, then shows the popup again with the
    /// new destination. The name is a single path component, so `/` is refused.
    pub fn rename_confirm_target(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            self.set_status(format!("invalid name: {name:?}"), true);
            return;
        }
        let destination = match self.confirm_action.as_ref().map(|a| &a.kind) {
            Some(ConfirmKind::Download(_)) => self.local_cwd.join(&name).display().to_string(),
            Some(ConfirmKind::Upload(_)) => self.upload_destination(Some(&name)),
            _ => return,
        };
        let Some(action) = &mut self.confirm_action else { return };
        match &mut action.kind {
            ConfirmKind::Download(slot) | ConfirmKind::Upload(slot) => *slot = Some(name),
            _ => return,
        }
        action.destination = Some(destination);
        action.yes_selected = true;
    }

    /// Asks before renaming the hovered item to `new_name`.
    pub fn request_confirm_rename(&mut self, new_name: String) {
        if new_name.is_empty() {
            return;
        }
        let old = match self.focus {
            Focus::Remote => self.current_entry().map(|e| e.name.clone()),
            Focus::Local => self.current_local_entry().map(|e| e.name.clone()),
            Focus::Preview | Focus::Jobs => None,
        };
        let Some(old) = old else { return };
        self.request_confirm("Rename this item?".to_string(), ConfirmKind::Rename(new_name.clone()), true);
        if let Some(action) = &mut self.confirm_action {
            action.rename_diff = Some((old, new_name));
        }
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
            Focus::Preview | Focus::Jobs => (0, String::new()),
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
