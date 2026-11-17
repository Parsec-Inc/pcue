use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Campo de texto de uma linha com cursor.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    /// Campo já preenchido, cursor no fim.
    pub fn with_value(value: &str) -> Self {
        Self {
            value: value.to_string(),
            cursor: value.chars().count(),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Posição do cursor em caracteres.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Aplica a tecla; devolve `false` se não for de edição.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                self.value = self.value.chars().skip(self.cursor).collect();
                self.cursor = 0;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            KeyCode::Char(ch) if !ctrl => {
                let at = self.byte_index(self.cursor);
                self.value.insert(at, ch);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_index(self.cursor);
                self.value.remove(at);
            }
            KeyCode::Delete if self.cursor < self.len() => {
                let at = self.byte_index(self.cursor);
                self.value.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut TextInput, code: KeyCode) {
        input.handle(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn edits_multibyte_text() {
        let mut input = TextInput::with_value("olá");
        press(&mut input, KeyCode::Left);
        press(&mut input, KeyCode::Char('x'));
        assert_eq!(input.value(), "olxá");
        press(&mut input, KeyCode::End);
        press(&mut input, KeyCode::Backspace);
        assert_eq!(input.value(), "olx");
    }
}
