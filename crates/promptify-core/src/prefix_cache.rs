//! Least-recently-used cache of model states keyed by the exact prefix tokens, bounded by bytes and entries.

pub struct PrefixCache<K, S> {
    entries: Vec<Entry<K, S>>,
    capacity: usize,
    max_entries: usize,
    used: usize,
}

struct Entry<K, S> {
    key: Vec<K>,
    state: S,
    bytes: usize,
}

impl<K: PartialEq, S> PrefixCache<K, S> {
    /// A zero capacity disables caching.
    pub fn new(capacity: usize, max_entries: usize) -> Self {
        Self { entries: Vec::new(), capacity, max_entries, used: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Only an exact key match counts: a state for different tokens would silently corrupt the output.
    pub fn get(&mut self, key: &[K]) -> Option<&S> {
        let index = self.entries.iter().position(|e| e.key == key)?;
        let entry = self.entries.remove(index);
        self.entries.push(entry);
        self.entries.last().map(|e| &e.state)
    }

    pub fn insert(&mut self, key: Vec<K>, state: S, bytes: usize) {
        if let Some(index) = self.entries.iter().position(|e| e.key == key) {
            self.used -= self.entries.remove(index).bytes;
        }
        if bytes > self.capacity || self.max_entries == 0 {
            return;
        }
        self.used += bytes;
        self.entries.push(Entry { key, state, bytes });
        while self.used > self.capacity || self.entries.len() > self.max_entries {
            self.used -= self.entries.remove(0).bytes;
        }
    }

    #[cfg(test)]
    fn used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_keys_only() {
        let mut cache = PrefixCache::new(100, 4);
        cache.insert(vec![1, 2, 3], "abc", 10);
        assert_eq!(cache.get(&[1, 2, 3]), Some(&"abc"));
        assert_eq!(cache.get(&[1, 2]), None, "a shorter prefix must not match");
        assert_eq!(cache.get(&[1, 2, 3, 4]), None, "a longer prompt must not match");
        assert_eq!(cache.get(&[1, 2, 4]), None);
    }

    #[test]
    fn evicts_least_recently_used_within_bytes_and_entries() {
        let mut cache = PrefixCache::new(30, 3);
        cache.insert(vec![1], "a", 10);
        cache.insert(vec![2], "b", 10);
        cache.insert(vec![3], "c", 10);
        assert!(cache.get(&[1]).is_some());
        cache.insert(vec![4], "d", 10);
        assert!(cache.get(&[2]).is_none(), "least recently used is evicted first");
        assert!(cache.get(&[1]).is_some() && cache.get(&[3]).is_some() && cache.get(&[4]).is_some());
        assert_eq!(cache.used(), 30);
        cache.insert(vec![5], "e", 25);
        assert_eq!(cache.used(), 25);
        assert!(cache.get(&[5]).is_some() && cache.get(&[4]).is_none());
        cache.insert(vec![6], "f", 31);
        assert!(cache.get(&[6]).is_none(), "an entry larger than the whole cache is not stored");
        assert_eq!(cache.used(), 25);
    }

    #[test]
    fn reinserting_a_key_replaces_it_and_zero_capacity_disables() {
        let mut cache = PrefixCache::new(30, 3);
        cache.insert(vec![1], "old", 10);
        cache.insert(vec![1], "new", 12);
        assert_eq!(cache.get(&[1]), Some(&"new"));
        assert_eq!(cache.used(), 12);
        let mut off = PrefixCache::new(0, 3);
        off.insert(vec![1], "x", 1);
        assert!(off.get(&[1]).is_none());
    }
}
