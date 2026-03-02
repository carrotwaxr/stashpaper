use crate::settings::RotationMode;
use rand::seq::SliceRandom;
use rand::Rng;

#[derive(Debug)]
pub struct RotationState {
    current_index: usize,
    shuffle_order: Vec<usize>,
    shuffle_position: usize,
    last_count: usize,
}

impl RotationState {
    pub fn new() -> Self {
        Self {
            current_index: 0,
            shuffle_order: Vec::new(),
            shuffle_position: 0,
            last_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.current_index = 0;
        self.shuffle_order.clear();
        self.shuffle_position = 0;
        self.last_count = 0;
    }

    /// Select the next page number (1-based) based on the rotation mode.
    /// Returns None if count is 0.
    pub fn select_next(&mut self, mode: RotationMode, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }

        // If count changed, reset shuffle
        if count != self.last_count {
            self.last_count = count;
            if mode == RotationMode::Shuffle {
                self.regenerate_shuffle(count);
            }
            // For sequential, clamp index
            if self.current_index > count {
                self.current_index = 0;
            }
        }

        Some(match mode {
            RotationMode::Random => {
                let mut rng = rand::rng();
                rng.random_range(1..=count)
            }
            RotationMode::Sequential => {
                self.current_index += 1;
                if self.current_index > count {
                    self.current_index = 1;
                }
                self.current_index
            }
            RotationMode::Shuffle => {
                if self.shuffle_position >= self.shuffle_order.len() {
                    self.regenerate_shuffle(count);
                }
                let page = self.shuffle_order[self.shuffle_position];
                self.shuffle_position += 1;
                page
            }
        })
    }

    fn regenerate_shuffle(&mut self, count: usize) {
        let mut rng = rand::rng();
        self.shuffle_order = (1..=count).collect();
        self.shuffle_order.shuffle(&mut rng);
        self.shuffle_position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_next_returns_none_for_zero_count() {
        let mut state = RotationState::new();
        assert_eq!(state.select_next(RotationMode::Random, 0), None);
        assert_eq!(state.select_next(RotationMode::Sequential, 0), None);
        assert_eq!(state.select_next(RotationMode::Shuffle, 0), None);
    }

    #[test]
    fn test_random_returns_valid_range() {
        let mut state = RotationState::new();
        for _ in 0..100 {
            let page = state.select_next(RotationMode::Random, 10).unwrap();
            assert!(page >= 1 && page <= 10);
        }
    }

    #[test]
    fn test_sequential_cycles_through_all() {
        let mut state = RotationState::new();
        let count = 5;

        for expected in 1..=5 {
            assert_eq!(
                state.select_next(RotationMode::Sequential, count),
                Some(expected)
            );
        }
        // Should wrap around
        assert_eq!(
            state.select_next(RotationMode::Sequential, count),
            Some(1)
        );
    }

    #[test]
    fn test_sequential_handles_count_change() {
        let mut state = RotationState::new();

        // Advance to position 3
        for _ in 0..3 {
            state.select_next(RotationMode::Sequential, 5);
        }
        assert_eq!(state.current_index, 3);

        // Count shrinks to 2 — index should clamp
        let page = state.select_next(RotationMode::Sequential, 2).unwrap();
        assert!(page >= 1 && page <= 2);
    }

    #[test]
    fn test_shuffle_visits_all_before_repeating() {
        let mut state = RotationState::new();
        let count = 5;
        let mut seen = std::collections::HashSet::new();

        for _ in 0..count {
            let page = state.select_next(RotationMode::Shuffle, count).unwrap();
            assert!(page >= 1 && page <= count);
            seen.insert(page);
        }

        // All pages should have been visited
        assert_eq!(seen.len(), count);
    }

    #[test]
    fn test_shuffle_regenerates_after_exhaustion() {
        let mut state = RotationState::new();
        let count = 3;

        // Exhaust first shuffle
        for _ in 0..count {
            state.select_next(RotationMode::Shuffle, count);
        }

        // Next call should start a new shuffle
        let page = state.select_next(RotationMode::Shuffle, count).unwrap();
        assert!(page >= 1 && page <= count);
    }

    #[test]
    fn test_reset_clears_state() {
        let mut state = RotationState::new();
        state.select_next(RotationMode::Sequential, 5);
        state.select_next(RotationMode::Sequential, 5);

        state.reset();
        assert_eq!(state.current_index, 0);
        assert!(state.shuffle_order.is_empty());

        // After reset, sequential should start from 1 again
        assert_eq!(
            state.select_next(RotationMode::Sequential, 5),
            Some(1)
        );
    }
}
