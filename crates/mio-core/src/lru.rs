use std::{borrow::Borrow, collections::HashMap, hash::Hash};

/// Bounded single-item-evicting LRU cache backed by monotonic access ticks.
pub(crate) struct BoundedLruCache<K, V> {
    capacity: usize,
    tick: u64,
    entries: HashMap<K, (V, u64)>,
}

impl<K, V> BoundedLruCache<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "LRU cache capacity must be non-zero");
        Self {
            capacity,
            tick: 0,
            entries: HashMap::with_capacity(capacity.min(256)),
        }
    }

    pub(crate) fn get<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Hash + Eq + ?Sized,
        V: Clone,
    {
        if let Some((value, last_used)) = self.entries.get_mut(key) {
            self.tick = self.tick.wrapping_add(1);
            *last_used = self.tick;
            return Some(value.clone());
        }
        None
    }

    pub(crate) fn insert(&mut self, key: K, value: V)
    where
        K: Clone + Eq + Hash,
    {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        if let Some(slot) = self.entries.get_mut(&key) {
            *slot = (value, tick);
            return;
        }
        if self.entries.len() >= self.capacity
            && let Some(evict_key) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, last_used))| *last_used)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&evict_key);
        }
        self.entries.insert(key, (value, tick));
    }

    #[cfg(test)]
    pub(crate) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Hash + Eq + ?Sized,
    {
        self.entries.contains_key(key)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
