//! Bounded session data which can be fetched again from Steam.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Debug)]
pub struct ReadCache<K, V, const LIMIT: usize> {
    entries: HashMap<K, (V, AtomicU64)>,
    clock: AtomicU64,
}

impl<K, V, const LIMIT: usize> Default for ReadCache<K, V, LIMIT> {
    fn default() -> Self {
        Self { entries: HashMap::new(), clock: AtomicU64::new(0) }
    }
}

impl<K: Clone + Eq + Hash, V: Clone, const LIMIT: usize> Clone for ReadCache<K, V, LIMIT> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.iter()
                .map(|(key, (value, touched))| (key.clone(), (value.clone(), AtomicU64::new(touched.load(Relaxed)))))
                .collect(),
            clock: AtomicU64::new(self.clock.load(Relaxed)),
        }
    }
}

impl<K: Clone + Eq + Hash, V, const LIMIT: usize> ReadCache<K, V, LIMIT> {
    pub fn get<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where K: Borrow<Q> {
        let (value, touched) = self.entries.get(key)?;
        touched.store(self.clock.fetch_add(1, Relaxed), Relaxed);
        Some(value)
    }

    pub fn contains_key<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where K: Borrow<Q> {
        self.get(key).is_some()
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let (value, touched) = self.entries.get_mut(key)?;
        touched.store(self.clock.fetch_add(1, Relaxed), Relaxed);
        Some(value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if !self.entries.contains_key(&key) && self.entries.len() >= LIMIT {
            if let Some(oldest) = self.entries.iter()
                .min_by_key(|(_, (_, touched))| touched.load(Relaxed))
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(key, (value, AtomicU64::new(self.clock.fetch_add(1, Relaxed))));
    }

    pub fn get_or_default(&mut self, key: K) -> &mut V
    where V: Default {
        if !self.entries.contains_key(&key) {
            self.insert(key.clone(), V::default());
        }
        self.get_mut(&key).expect("cache entry inserted")
    }

    pub fn retain(&mut self, mut f: impl FnMut(&K, &mut V) -> bool) {
        self.entries.retain(|key, (value, _)| f(key, value));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, (value, _))| (key, value))
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values().map(|(value, _)| value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.values_mut().map(|(value, _)| value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_replacements_keep_entries_recent() {
        let mut cache = ReadCache::<_, _, 2>::default();
        cache.insert(1, "one");
        cache.insert(2, "two");
        assert_eq!(cache.get(&1), Some(&"one"));
        cache.insert(3, "three");
        assert_eq!(cache.get(&2), None);
        cache.insert(1, "ONE");
        cache.insert(4, "four");
        assert_eq!(cache.get(&3), None);
        assert_eq!(cache.get(&1), Some(&"ONE"));
        assert_eq!(cache.get(&4), Some(&"four"));
    }
}
