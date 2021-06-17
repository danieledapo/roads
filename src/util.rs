use std::time;

use tui::widgets;

pub struct WrappingList<T> {
    data: Vec<T>,
    state: widgets::ListState,
}

impl<T> WrappingList<T> {
    pub fn new(data: Vec<T>) -> Self {
        let mut l = Self {
            data,
            state: widgets::ListState::default(),
        };

        if !l.data.is_empty() {
            l.state.select(Some(0));
        }

        l
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.data.iter()
    }

    pub fn state(&mut self) -> &mut widgets::ListState {
        &mut self.state
    }

    pub fn selected_ix(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn selected(&self) -> Option<&T> {
        Some(&self.data[self.state.selected()?])
    }

    pub fn selected_mut(&mut self) -> Option<&mut T> {
        Some(&mut self.data[self.state.selected()?])
    }

    pub fn down(&mut self) {
        if self.data.is_empty() {
            return;
        }

        let next = (self.state.selected().unwrap_or_default() + 1) % self.data.len();
        self.state.select(Some(next));
    }

    pub fn up(&mut self) {
        if self.data.is_empty() {
            return;
        }

        let next =
            (self.state.selected().unwrap_or_default() + self.data.len() - 1) % self.data.len();
        self.state.select(Some(next));
    }
}

pub struct DotsSpinner {
    state: usize,
    last_tick: Option<time::Instant>,
}

impl DotsSpinner {
    pub const PATTERN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

    pub fn new() -> Self {
        Self {
            state: 0,
            last_tick: None,
        }
    }

    pub fn tick(&mut self) {
        let now = time::Instant::now();

        match self.last_tick {
            None => self.last_tick = Some(now),
            Some(t) => {
                if now - t >= time::Duration::from_millis(80) {
                    self.last_tick = Some(now);
                    self.state = (self.state + 1) % Self::PATTERN.len();
                }
            }
        }
    }

    pub fn pattern(&self) -> char {
        Self::PATTERN[self.state]
    }
}
