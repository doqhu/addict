//! The history of the field: the texts it held, in the order they came, and
//! where Back and Forward stand in it, as in a browser.
//!
//! A text is recorded when it is submitted, by Enter, a paste or a click on
//! a synonym, and when it replaces the field, recognized in a capture or read
//! from the primary selection or the clipboard; an empty text and the text
//! already at the position are not. A text recorded after Back drops the
//! texts ahead of the position. Typing records nothing, but Back from a typed
//! text that is not the one at the position records it first, so Forward
//! comes back to it. Going Back or Forward records nothing. At most
//! [`LIMIT`] texts are kept, the oldest dropped first.
//!
//! This module owns the texts and the position, and nothing of the window:
//! [`resident`](crate::resident) records and walks the history, and
//! [`widgets`](crate::widgets) puts the text it gives into the field.

/// How many texts the history keeps.
pub(crate) const LIMIT: usize = 100;

/// The texts the field held, the oldest first, and the position in them.
#[derive(Debug, Default)]
pub(crate) struct History {
    entries: Vec<String>,
    /// The index of the text at the position; 0 when there is none.
    at: usize,
}

impl History {
    /// Records `text` at the position: the texts ahead of it go, and `text`
    /// is the newest. An empty text, and the text already at the position,
    /// change nothing.
    pub(crate) fn record(&mut self, text: &str) {
        if text.is_empty() || self.current() == Some(text) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.at + 1);
        }
        self.entries.push(text.to_owned());
        if self.entries.len() > LIMIT {
            let excess = self.entries.len() - LIMIT;
            self.entries.drain(..excess);
        }
        self.at = self.entries.len() - 1;
    }

    /// The text before the position, which becomes the position; `None`
    /// when there is none. The field's `current` text, when it is not the
    /// one at the position, is recorded first.
    pub(crate) fn back(&mut self, current: &str) -> Option<&str> {
        if !self.can_back(current) {
            return None;
        }
        self.record(current);
        self.at -= 1;
        Some(&self.entries[self.at])
    }

    /// The text after the position, which becomes the position; `None`
    /// when there is none.
    pub(crate) fn forward(&mut self) -> Option<&str> {
        if !self.can_forward() {
            return None;
        }
        self.at += 1;
        Some(&self.entries[self.at])
    }

    /// Whether Back from the field's `current` text has somewhere to go: a
    /// text before the position, or the one at it when `current` is another
    /// text, which Back records.
    pub(crate) fn can_back(&self, current: &str) -> bool {
        match self.current() {
            None => false,
            Some(at) => self.at > 0 || (!current.is_empty() && current != at),
        }
    }

    /// Whether there is a text after the position.
    pub(crate) fn can_forward(&self) -> bool {
        self.at + 1 < self.entries.len()
    }

    /// The text at the position, if any.
    fn current(&self) -> Option<&str> {
        self.entries.get(self.at).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(texts: &[&str]) -> History {
        let mut history = History::default();
        for text in texts {
            history.record(text);
        }
        history
    }

    #[test]
    fn back_goes_to_the_text_before() {
        let mut history = of(&["bank", "river"]);
        assert!(history.can_back("river"));
        assert!(!history.can_forward());
        assert_eq!(history.back("river"), Some("bank"));
        assert!(history.can_forward());
        assert!(!history.can_back("bank"));
    }

    #[test]
    fn forward_comes_back() {
        let mut history = of(&["bank", "river"]);
        history.back("river");
        assert_eq!(history.forward(), Some("river"));
        assert!(!history.can_forward());
        assert_eq!(history.forward(), None);
    }

    #[test]
    fn a_text_recorded_after_back_drops_the_ones_ahead() {
        let mut history = of(&["bank", "river"]);
        history.back("river");
        history.record("loan");
        assert_eq!(history.entries, ["bank", "loan"]);
        assert!(!history.can_forward());
        assert_eq!(history.back("loan"), Some("bank"));
    }

    #[test]
    fn back_from_a_typed_text_records_it() {
        let mut history = of(&["bank"]);
        assert!(history.can_back("cat"));
        assert_eq!(history.back("cat"), Some("bank"));
        assert_eq!(history.forward(), Some("cat"));
    }

    #[test]
    fn the_edges_go_nowhere() {
        let mut empty = History::default();
        assert!(!empty.can_back("cat"));
        assert!(!empty.can_forward());
        assert_eq!(empty.back("cat"), None);
        assert_eq!(empty.forward(), None);
        assert!(empty.entries.is_empty());

        let mut history = of(&["bank", "river"]);
        assert_eq!(history.forward(), None);
        history.back("river");
        assert_eq!(history.back("bank"), None);
        assert_eq!(history.back(""), None);
        assert_eq!(history.at, 0);
    }

    #[test]
    fn a_repeated_text_is_one_entry() {
        let history = of(&["bank", "bank"]);
        assert_eq!(history.entries, ["bank"]);
    }

    #[test]
    fn an_empty_text_is_not_recorded() {
        let history = of(&["", "bank", ""]);
        assert_eq!(history.entries, ["bank"]);
    }

    #[test]
    fn navigating_records_nothing() {
        let mut history = of(&["bank", "river"]);
        let back = history.back("river").map(str::to_owned).expect("bank");
        // The submit of the text Back put into the field.
        history.record(&back);
        assert_eq!(history.entries, ["bank", "river"]);
        assert!(history.can_forward());
    }

    #[test]
    fn the_oldest_go_past_the_limit() {
        let mut history = History::default();
        for number in 0..LIMIT + 5 {
            history.record(&number.to_string());
        }
        assert_eq!(history.entries.len(), LIMIT);
        assert_eq!(history.entries[0], "5");
        assert_eq!(history.current(), Some((LIMIT + 4).to_string().as_str()));
        assert!(!history.can_forward());
    }
}
