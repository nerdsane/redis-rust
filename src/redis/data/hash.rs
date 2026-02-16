//! Redis Hash data structure

use super::SDS;
use ahash::AHashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct RedisHash {
    fields: AHashMap<String, SDS>,
}

impl RedisHash {
    pub fn new() -> Self {
        RedisHash {
            fields: AHashMap::new(),
        }
    }

    /// Verify all invariants hold for this hash
    /// Called in debug builds after every mutation
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual AHashMap size
        debug_assert_eq!(
            self.len(),
            self.fields.len(),
            "Invariant violated: len() must equal fields.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.fields.is_empty(),
            "Invariant violated: is_empty() must equal fields.is_empty()"
        );

        // Invariant 3: All keys should be retrievable
        for (key, value) in &self.fields {
            let key_sds = SDS::from_str(key);
            debug_assert!(
                self.fields.get(key).is_some(),
                "Invariant violated: key '{}' must be retrievable",
                key
            );
            debug_assert_eq!(
                self.fields.get(key),
                Some(value),
                "Invariant violated: value for key '{}' must be consistent",
                key
            );
            // Invariant 4: Key converted to SDS and back should match
            debug_assert_eq!(
                key_sds.to_string(),
                *key,
                "Invariant violated: key roundtrip must be stable"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn set(&mut self, field: SDS, value: SDS) {
        let field_str = field.to_string();

        // TigerStyle: Precondition - capture state for postcondition check
        #[cfg(debug_assertions)]
        let expected_len = if self.fields.contains_key(&field_str) {
            self.fields.len()
        } else {
            self.fields.len() + 1
        };

        self.fields.insert(field_str.clone(), value.clone());

        // TigerStyle: Postcondition - verify the set succeeded
        debug_assert!(
            self.fields.contains_key(&field_str),
            "Postcondition violated: field must exist after set"
        );
        debug_assert_eq!(
            self.fields.get(&field_str),
            Some(&value),
            "Postcondition violated: value must match after set"
        );
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.fields.len(),
            expected_len,
            "Postcondition violated: len must be correct after set"
        );

        self.verify_invariants();
    }

    pub fn get(&self, field: &SDS) -> Option<&SDS> {
        self.fields.get(&field.to_string())
    }

    pub fn delete(&mut self, field: &SDS) -> bool {
        let field_str = field.to_string();

        // TigerStyle: Precondition - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.fields.len();
        #[cfg(debug_assertions)]
        let existed = self.fields.contains_key(&field_str);

        let removed = self.fields.remove(&field_str).is_some();

        // TigerStyle: Postconditions
        debug_assert!(
            !self.fields.contains_key(&field_str),
            "Postcondition violated: field must not exist after delete"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                removed, existed,
                "Postcondition violated: remove result must match existence"
            );
            let expected_len = if existed { pre_len - 1 } else { pre_len };
            debug_assert_eq!(
                self.fields.len(),
                expected_len,
                "Postcondition violated: len must be correct after delete"
            );
        }

        self.verify_invariants();
        removed
    }

    pub fn exists(&self, field: &SDS) -> bool {
        self.fields.contains_key(&field.to_string())
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn keys(&self) -> Vec<SDS> {
        self.fields.keys().map(|k| SDS::from_str(k)).collect()
    }

    pub fn values(&self) -> Vec<SDS> {
        self.fields.values().cloned().collect()
    }

    pub fn get_all(&self) -> Vec<(SDS, SDS)> {
        self.fields
            .iter()
            .map(|(k, v)| (SDS::from_str(k), v.clone()))
            .collect()
    }

    /// Iterate over field-value pairs (for HSCAN)
    pub fn iter(&self) -> impl Iterator<Item = (&String, &SDS)> {
        self.fields.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_set_and_get() {
        let mut hash = RedisHash::new();

        // Set a field
        hash.set(SDS::from_str("name"), SDS::from_str("Alice"));
        assert_eq!(hash.len(), 1);
        assert!(!hash.is_empty());

        // Get the field
        let value = hash.get(&SDS::from_str("name"));
        assert!(value.is_some());
        assert_eq!(value.unwrap().to_string(), "Alice");

        // Update the field
        hash.set(SDS::from_str("name"), SDS::from_str("Bob"));
        assert_eq!(hash.len(), 1); // Should not increase length
        assert_eq!(hash.get(&SDS::from_str("name")).unwrap().to_string(), "Bob");
    }

    #[test]
    fn test_hash_delete() {
        let mut hash = RedisHash::new();

        hash.set(SDS::from_str("a"), SDS::from_str("1"));
        hash.set(SDS::from_str("b"), SDS::from_str("2"));
        assert_eq!(hash.len(), 2);

        // Delete existing
        let removed = hash.delete(&SDS::from_str("a"));
        assert!(removed);
        assert_eq!(hash.len(), 1);
        assert!(!hash.exists(&SDS::from_str("a")));

        // Delete non-existing
        let removed = hash.delete(&SDS::from_str("nonexistent"));
        assert!(!removed);
        assert_eq!(hash.len(), 1);
    }

    #[test]
    fn test_hash_keys_and_values() {
        let mut hash = RedisHash::new();

        hash.set(SDS::from_str("k1"), SDS::from_str("v1"));
        hash.set(SDS::from_str("k2"), SDS::from_str("v2"));
        hash.set(SDS::from_str("k3"), SDS::from_str("v3"));

        let keys = hash.keys();
        assert_eq!(keys.len(), 3);

        let values = hash.values();
        assert_eq!(values.len(), 3);

        let all = hash.get_all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_hash_invariants_maintained() {
        let mut hash = RedisHash::new();

        // Empty hash
        assert!(hash.is_empty());
        assert_eq!(hash.len(), 0);

        // Add elements
        hash.set(SDS::from_str("counter"), SDS::from_str("0"));
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 1);
        assert!(hash.exists(&SDS::from_str("counter")));

        // Update (invariants checked by verify_invariants in debug)
        hash.set(SDS::from_str("counter"), SDS::from_str("10"));
        assert_eq!(hash.len(), 1);
        assert_eq!(
            hash.get(&SDS::from_str("counter")).unwrap().to_string(),
            "10"
        );

        // Delete (invariants checked by verify_invariants in debug)
        hash.delete(&SDS::from_str("counter"));
        assert!(hash.is_empty());
        assert_eq!(hash.len(), 0);
        assert!(!hash.exists(&SDS::from_str("counter")));
    }
}
