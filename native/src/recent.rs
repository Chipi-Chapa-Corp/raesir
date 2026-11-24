//! Least-recently-used values kept in memory up to a caller-defined cost budget.
use std::collections::{HashMap, VecDeque};

pub(crate) struct Recent<V> {
    entries: HashMap<String, (V, usize)>,
    order: VecDeque<String>,
    cost: usize,
    budget: usize,
}

impl<V: Clone> Recent<V> {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            cost: 0,
            budget,
        }
    }

    pub(crate) fn get(&mut self, key: &str) -> Option<V> {
        let value = self.entries.get(key)?.0.clone();
        self.touch(key);
        Some(value)
    }

    /// Stores `value`, evicting the least recently used entries over budget.
    pub(crate) fn insert(&mut self, key: String, value: V, cost: usize) {
        if let Some((_, old)) = self.entries.remove(&key) {
            self.cost -= old;
        }
        self.cost += cost;
        self.entries.insert(key.clone(), (value, cost));
        self.touch(&key);
        while self.cost > self.budget && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, cost)) = self.entries.remove(&oldest) {
                self.cost -= cost;
            }
        }
    }

    fn touch(&mut self, key: &str) {
        if self.order.back().is_some_and(|last| last == key) {
            return;
        }
        self.order.retain(|item| item != key);
        self.order.push_back(key.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_entries_over_budget() {
        let mut recent = Recent::new(3);
        recent.insert("a".into(), 1, 1);
        recent.insert("b".into(), 2, 1);
        recent.insert("c".into(), 3, 1);
        assert_eq!(recent.get("a"), Some(1));
        recent.insert("d".into(), 4, 1);
        assert_eq!(recent.get("b"), None);
        assert_eq!(recent.get("a"), Some(1));
        recent.insert("a".into(), 5, 2);
        assert_eq!(recent.get("c"), None);
        assert_eq!(recent.get("a"), Some(5));
        assert_eq!(recent.get("d"), Some(4));
    }

    #[test]
    fn keeps_a_single_entry_larger_than_the_budget() {
        let mut recent = Recent::new(1);
        recent.insert("large".into(), 1, 10);
        assert_eq!(recent.get("large"), Some(1));
        recent.insert("next".into(), 2, 1);
        assert_eq!(recent.get("large"), None);
        assert_eq!(recent.get("next"), Some(2));
    }
}
