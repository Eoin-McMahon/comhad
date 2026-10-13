//! Line editing for the single-line text prompts (rename, save as, filter, upload path, bookmark
//! fields). Supports the usual readline/macOS movements so the prompt behaves like a normal text box.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Prompt, PromptKind};

impl Prompt {
    /// A prompt prefilled with `buffer`, the cursor at its end.
    pub fn new(kind: PromptKind, buffer: String, mask: bool) -> Self {
        let cursor = buffer.chars().count();
        Self { kind, buffer, cursor, mask }
    }

    /// The text before and after the cursor, for rendering the caret in place.
    pub fn split_at_cursor(&self) -> (String, String) {
        let byte = self.byte_index(self.cursor);
        (self.buffer[..byte].to_string(), self.buffer[byte..].to_string())
    }

    /// Inserts `text` at the cursor, used for typed characters and pastes.
    pub fn insert_str(&mut self, text: &str) {
        let byte = self.byte_index(self.cursor);
        self.buffer.insert_str(byte, text);
        self.cursor += text.chars().count();
    }

    /// Applies an editing or movement key. Returns whether the key was one; `enter`, `esc` and
    /// anything else the caller owns come back `false`.
    pub fn edit(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let len = self.len();
        match key.code {
            KeyCode::Left if ctrl || alt => self.cursor = self.word_start_before(self.cursor),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_end_after(self.cursor),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace if ctrl || alt => self.delete_range(self.word_start_before(self.cursor), self.cursor),
            KeyCode::Backspace => self.delete_range(self.cursor.saturating_sub(1), self.cursor),
            KeyCode::Delete if ctrl || alt => self.delete_range(self.cursor, self.word_end_after(self.cursor)),
            KeyCode::Delete => self.delete_range(self.cursor, (self.cursor + 1).min(len)),
            KeyCode::Char(c) if ctrl => match c {
                'a' => self.cursor = 0,
                'e' => self.cursor = len,
                'b' => self.cursor = self.cursor.saturating_sub(1),
                'f' => self.cursor = (self.cursor + 1).min(len),
                'h' => self.delete_range(self.cursor.saturating_sub(1), self.cursor),
                'd' => self.delete_range(self.cursor, (self.cursor + 1).min(len)),
                'w' => self.delete_range(self.word_start_before(self.cursor), self.cursor),
                'u' => self.delete_range(0, self.cursor),
                'k' => self.delete_range(self.cursor, len),
                _ => return false,
            },
            KeyCode::Char(c) if alt => match c {
                'b' => self.cursor = self.word_start_before(self.cursor),
                'f' => self.cursor = self.word_end_after(self.cursor),
                'd' => self.delete_range(self.cursor, self.word_end_after(self.cursor)),
                _ => return false,
            },
            KeyCode::Char(c) => self.insert_str(c.encode_utf8(&mut [0; 4])),
            _ => return false,
        }
        true
    }

    fn len(&self) -> usize {
        self.buffer.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.buffer.char_indices().nth(char_index).map(|(b, _)| b).unwrap_or(self.buffer.len())
    }

    /// Removes chars `from..to` and leaves the cursor at `from`.
    fn delete_range(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (start, end) = (self.byte_index(from), self.byte_index(to));
        self.buffer.replace_range(start..end, "");
        self.cursor = from;
    }

    /// Start of the word before `pos`: skips separators, then the word itself. A word is a run of
    /// letters and digits, so `ctrl+w` on `report_final.txt` removes just `txt`.
    fn word_start_before(&self, pos: usize) -> usize {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = pos.min(chars.len());
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    /// End of the word after `pos`, the mirror of [`Self::word_start_before`].
    fn word_end_after(&self, pos: usize) -> usize {
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut i = pos.min(chars.len());
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(text: &str) -> Prompt {
        Prompt::new(PromptKind::Rename, text.to_string(), false)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut p = prompt("report.txt");
        for _ in 0..4 {
            p.edit(&key(KeyCode::Left, KeyModifiers::NONE));
        }
        for c in "_v2".chars() {
            p.edit(&key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(p.buffer, "report_v2.txt");
        assert_eq!(p.cursor, 9);
    }

    #[test]
    fn ctrl_w_deletes_the_word_before_the_cursor() {
        let mut p = prompt("report_final.txt");
        p.edit(&key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(p.buffer, "report_final.");
        p.edit(&key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(p.buffer, "report_");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_cut_to_either_end() {
        let mut p = prompt("abcdef");
        p.edit(&key(KeyCode::Char('a'), KeyModifiers::CONTROL));
        p.edit(&key(KeyCode::Right, KeyModifiers::NONE));
        p.edit(&key(KeyCode::Right, KeyModifiers::NONE));
        p.edit(&key(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(p.buffer, "ab");
        p.edit(&key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!((p.buffer.as_str(), p.cursor), ("", 0));
    }

    #[test]
    fn word_jumps_and_deletes_handle_multibyte_text() {
        let mut p = prompt("café menu.txt");
        p.edit(&key(KeyCode::Char('b'), KeyModifiers::ALT));
        p.edit(&key(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(p.cursor, 5);
        p.edit(&key(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(p.buffer, "menu.txt");
        p.edit(&key(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(p.buffer, "enu.txt");
    }
}
