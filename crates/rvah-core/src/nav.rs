/// Linear back/forward navigation over opaque page references (FR-042).
#[derive(Debug, Default)]
pub struct NavHistory {
    back: Vec<String>,
    current: Option<String>,
    forward: Vec<String>,
}

impl NavHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Navigate to a new page; any forward history is discarded.
    pub fn push(&mut self, page_ref: impl Into<String>) {
        if let Some(prev) = self.current.take() {
            self.back.push(prev);
        }
        self.current = Some(page_ref.into());
        self.forward.clear();
    }

    /// Step back; returns the new current page, or `None` at the beginning.
    pub fn back(&mut self) -> Option<&str> {
        let prev = self.back.pop()?;
        if let Some(cur) = self.current.take() {
            self.forward.push(cur);
        }
        self.current = Some(prev);
        self.current.as_deref()
    }

    /// Step forward; returns the new current page, or `None` at the end.
    pub fn forward(&mut self) -> Option<&str> {
        let next = self.forward.pop()?;
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
        }
        self.current = Some(next);
        self.current.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_history_back_forward() {
        let mut nav = NavHistory::new();
        assert_eq!(nav.back(), None);

        nav.push("a");
        nav.push("b");
        nav.push("c");
        assert_eq!(nav.current(), Some("c"));

        assert_eq!(nav.back(), Some("b"));
        assert_eq!(nav.back(), Some("a"));
        assert_eq!(nav.back(), None);
        assert_eq!(nav.current(), Some("a"));

        assert_eq!(nav.forward(), Some("b"));
        // A new navigation discards the forward stack.
        nav.push("d");
        assert_eq!(nav.forward(), None);
        assert_eq!(nav.back(), Some("b"));
    }
}
