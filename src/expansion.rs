use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc};

pub type SharedSnippetIndex = Arc<RwLock<HashMap<String, String>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub backspaces: usize,
    pub replacement: String,
    pub trailing: char,
}

pub struct ExpansionMatcher {
    index: SharedSnippetIndex,
    token: String,
    max_trigger_len: usize,
}

impl ExpansionMatcher {
    pub fn new(index: SharedSnippetIndex) -> Self {
        Self {
            index,
            token: String::new(),
            max_trigger_len: 128,
        }
    }

    pub fn feed_char(&mut self, ch: char) -> Option<Expansion> {
        if is_delimiter(ch) {
            let trigger = std::mem::take(&mut self.token);
            if trigger.is_empty() {
                return None;
            }

            let replacement = self.index.read().get(&trigger).cloned()?;
            return Some(Expansion {
                backspaces: trigger.chars().count(),
                replacement,
                trailing: ch,
            });
        }

        if ch.is_control() {
            self.token.clear();
            return None;
        }

        self.token.push(ch);
        if self.token.chars().count() > self.max_trigger_len {
            self.token.clear();
        }
        None
    }

    pub fn backspace(&mut self) {
        self.token.pop();
    }

    pub fn reset(&mut self) {
        self.token.clear();
    }
}

fn is_delimiter(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_trigger_on_space() {
        let index = SharedSnippetIndex::default();
        index.write().insert(";brb".into(), "Be right back.".into());
        let mut matcher = ExpansionMatcher::new(index);

        for ch in ";brb".chars() {
            assert!(matcher.feed_char(ch).is_none());
        }

        let expansion = matcher.feed_char(' ').unwrap();
        assert_eq!(expansion.backspaces, 4);
        assert_eq!(expansion.replacement, "Be right back.");
        assert_eq!(expansion.trailing, ' ');
    }

    #[test]
    fn backspace_changes_candidate() {
        let index = SharedSnippetIndex::default();
        index.write().insert(";sig".into(), "Signature".into());
        let mut matcher = ExpansionMatcher::new(index);

        for ch in ";six".chars() {
            matcher.feed_char(ch);
        }
        matcher.backspace();
        matcher.feed_char('g');

        assert_eq!(matcher.feed_char(' ').unwrap().replacement, "Signature");
    }
}
