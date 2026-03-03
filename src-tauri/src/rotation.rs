use crate::settings::RotationMode;
use rand::seq::SliceRandom;
use rand::Rng;

#[derive(Debug, Clone, PartialEq)]
pub struct RotationResult {
    pub page: usize,
    pub random_seed: Option<u64>,
}

#[derive(Debug)]
pub struct RotationState {
    current_index: usize,
    shuffle_order: Vec<usize>,
    shuffle_position: usize,
    last_count: usize,
    random_seed: Option<u64>,
    random_page: usize,
}

impl RotationState {
    pub fn new() -> Self {
        Self {
            current_index: 0,
            shuffle_order: Vec::new(),
            shuffle_position: 0,
            last_count: 0,
            random_seed: None,
            random_page: 0,
        }
    }

    pub fn reset(&mut self) {
        self.current_index = 0;
        self.shuffle_order.clear();
        self.shuffle_position = 0;
        self.last_count = 0;
        self.random_seed = None;
        self.random_page = 0;
    }

    /// Select the next page number (1-based) based on the rotation mode.
    /// Returns None if count is 0.
    pub fn select_next(&mut self, mode: RotationMode, count: usize) -> Option<RotationResult> {
        if count == 0 {
            return None;
        }

        // If count changed, handle resets
        if count != self.last_count {
            self.last_count = count;
            if mode == RotationMode::Shuffle {
                self.regenerate_shuffle(count);
            }
            // For sequential, clamp index
            if self.current_index > count {
                self.current_index = 0;
            }
            // For random, if count shrank below current page, regenerate seed
            if mode == RotationMode::Random && self.random_page > count {
                self.random_seed = None;
                self.random_page = 0;
            }
        }

        Some(match mode {
            RotationMode::Random => {
                let mut rng = rand::rng();
                // Generate seed on first use
                if self.random_seed.is_none() {
                    self.random_seed = Some(rng.random_range(0..100_000_000u64));
                    self.random_page = 0;
                }
                self.random_page += 1;
                // Exhausted all pages — new seed, start over
                if self.random_page > count {
                    self.random_seed = Some(rng.random_range(0..100_000_000u64));
                    self.random_page = 1;
                }
                RotationResult {
                    page: self.random_page,
                    random_seed: self.random_seed,
                }
            }
            RotationMode::Sequential => {
                self.current_index += 1;
                if self.current_index > count {
                    self.current_index = 1;
                }
                RotationResult {
                    page: self.current_index,
                    random_seed: None,
                }
            }
            RotationMode::Shuffle => {
                if self.shuffle_position >= self.shuffle_order.len() {
                    self.regenerate_shuffle(count);
                }
                let page = self.shuffle_order[self.shuffle_position];
                self.shuffle_position += 1;
                RotationResult {
                    page,
                    random_seed: None,
                }
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
    fn test_random_generates_seed_and_pages_sequentially() {
        let mut state = RotationState::new();
        let count = 10;

        let r1 = state.select_next(RotationMode::Random, count).unwrap();
        assert!(r1.random_seed.is_some());
        assert_eq!(r1.page, 1);

        let r2 = state.select_next(RotationMode::Random, count).unwrap();
        // Same seed, next page
        assert_eq!(r2.random_seed, r1.random_seed);
        assert_eq!(r2.page, 2);
    }

    #[test]
    fn test_random_seed_within_range() {
        let mut state = RotationState::new();
        let r = state.select_next(RotationMode::Random, 100).unwrap();
        assert!(r.random_seed.unwrap() < 100_000_000);
    }

    #[test]
    fn test_random_regenerates_seed_on_exhaustion() {
        let mut state = RotationState::new();
        let count = 3;

        // Exhaust all pages
        let r1 = state.select_next(RotationMode::Random, count).unwrap();
        let seed1 = r1.random_seed;
        state.select_next(RotationMode::Random, count);
        state.select_next(RotationMode::Random, count);

        // Next call should regenerate seed and reset to page 1
        let r4 = state.select_next(RotationMode::Random, count).unwrap();
        assert_eq!(r4.page, 1);
        // Seed may or may not change (random), but it should be Some
        assert!(r4.random_seed.is_some());
        // Very unlikely to get the same seed twice in 100M range, but not impossible
        let _ = seed1; // just confirm it compiled
    }

    #[test]
    fn test_random_pages_through_all_before_repeating() {
        let mut state = RotationState::new();
        let count = 5;
        let mut pages = Vec::new();

        for _ in 0..count {
            let r = state.select_next(RotationMode::Random, count).unwrap();
            pages.push(r.page);
        }
        // Should have pages 1..=5 in order
        assert_eq!(pages, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_sequential_cycles_through_all() {
        let mut state = RotationState::new();
        let count = 5;

        for expected in 1..=5 {
            let r = state.select_next(RotationMode::Sequential, count).unwrap();
            assert_eq!(r.page, expected);
            assert_eq!(r.random_seed, None);
        }
        // Should wrap around
        let r = state.select_next(RotationMode::Sequential, count).unwrap();
        assert_eq!(r.page, 1);
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
        let r = state.select_next(RotationMode::Sequential, 2).unwrap();
        assert!(r.page >= 1 && r.page <= 2);
    }

    #[test]
    fn test_shuffle_visits_all_before_repeating() {
        let mut state = RotationState::new();
        let count = 5;
        let mut seen = std::collections::HashSet::new();

        for _ in 0..count {
            let r = state.select_next(RotationMode::Shuffle, count).unwrap();
            assert!(r.page >= 1 && r.page <= count);
            assert_eq!(r.random_seed, None);
            seen.insert(r.page);
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
        let r = state.select_next(RotationMode::Shuffle, count).unwrap();
        assert!(r.page >= 1 && r.page <= count);
    }

    #[test]
    fn test_reset_clears_state() {
        let mut state = RotationState::new();
        state.select_next(RotationMode::Sequential, 5);
        state.select_next(RotationMode::Sequential, 5);

        state.reset();
        assert_eq!(state.current_index, 0);
        assert!(state.shuffle_order.is_empty());
        assert!(state.random_seed.is_none());
        assert_eq!(state.random_page, 0);

        // After reset, sequential should start from 1 again
        let r = state.select_next(RotationMode::Sequential, 5).unwrap();
        assert_eq!(r.page, 1);
    }

    #[test]
    fn test_reset_clears_random_seed() {
        let mut state = RotationState::new();

        // Generate a seed
        state.select_next(RotationMode::Random, 10);
        assert!(state.random_seed.is_some());

        state.reset();
        assert!(state.random_seed.is_none());

        // After reset, should generate new seed
        let r = state.select_next(RotationMode::Random, 10).unwrap();
        assert!(r.random_seed.is_some());
        assert_eq!(r.page, 1);
    }
}
