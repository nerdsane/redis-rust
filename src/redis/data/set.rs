//! Redis Set data structure

use super::SDS;
use ahash::AHashSet;

#[derive(Clone, Debug, PartialEq)]
pub struct RedisSet {
    members: AHashSet<String>,
}

impl RedisSet {
    pub fn new() -> Self {
        RedisSet {
            members: AHashSet::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this set
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual HashSet size
        debug_assert_eq!(
            self.len(),
            self.members.len(),
            "Invariant violated: len() must equal members.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.members.is_empty(),
            "Invariant violated: is_empty() must equal members.is_empty()"
        );

        // Invariant 3: All members must be retrievable via contains()
        for member in &self.members {
            debug_assert!(
                self.members.contains(member),
                "Invariant violated: member '{}' must be in set",
                member
            );
        }

        // Invariant 4: members() count must equal len()
        debug_assert_eq!(
            self.members().len(),
            self.len(),
            "Invariant violated: members().len() must equal len()"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn add(&mut self, member: SDS) -> bool {
        let member_str = member.to_string();

        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        #[cfg(debug_assertions)]
        let already_exists = self.members.contains(&member_str);

        let inserted = self.members.insert(member_str.clone());

        // TigerStyle: Postconditions
        debug_assert!(
            self.members.contains(&member_str),
            "Postcondition violated: member must exist after add"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                inserted, !already_exists,
                "Postcondition violated: insert result must match prior non-existence"
            );
            let expected_len = if already_exists { pre_len } else { pre_len + 1 };
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after add"
            );
        }

        self.verify_invariants();
        inserted
    }

    pub fn remove(&mut self, member: &SDS) -> bool {
        let member_str = member.to_string();

        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        #[cfg(debug_assertions)]
        let existed = self.members.contains(&member_str);

        let removed = self.members.remove(&member_str);

        // TigerStyle: Postconditions
        debug_assert!(
            !self.members.contains(&member_str),
            "Postcondition violated: member must not exist after remove"
        );
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                removed, existed,
                "Postcondition violated: remove result must match prior existence"
            );
            let expected_len = if existed { pre_len - 1 } else { pre_len };
            debug_assert_eq!(
                self.members.len(),
                expected_len,
                "Postcondition violated: len must be correct after remove"
            );
        }

        self.verify_invariants();
        removed
    }

    pub fn contains(&self, member: &SDS) -> bool {
        self.members.contains(&member.to_string())
    }

    pub fn members(&self) -> Vec<SDS> {
        self.members.iter().map(|s| SDS::from_str(s)).collect()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// SPOP: Remove and return a random member from the set
    /// Returns None if the set is empty
    pub fn pop(&mut self) -> Option<SDS> {
        #[cfg(debug_assertions)]
        let pre_len = self.members.len();

        // Get a "random" member by taking the first from iterator
        // HashSet iteration order is arbitrary, which provides pseudo-randomness
        let member = self.members.iter().next().cloned();

        if let Some(ref m) = member {
            let removed = self.members.remove(m);

            // TigerStyle: Postconditions
            debug_assert!(
                removed,
                "Postcondition violated: member must have been removed"
            );
            debug_assert!(
                !self.members.contains(m),
                "Postcondition violated: popped member must not exist in set"
            );
            #[cfg(debug_assertions)]
            {
                debug_assert_eq!(
                    self.members.len(),
                    pre_len - 1,
                    "Postcondition violated: len must decrease by 1 after pop"
                );
            }
        }

        self.verify_invariants();
        member.map(|s| SDS::from_str(&s))
    }

    /// SPOP with count: Remove and return up to `count` random members
    pub fn pop_count(&mut self, count: usize) -> Vec<SDS> {
        #[cfg(debug_assertions)]
        let pre_len = self.members.len();

        let to_remove = count.min(self.members.len());
        let mut result = Vec::with_capacity(to_remove);

        for _ in 0..to_remove {
            if let Some(member) = self.members.iter().next().cloned() {
                self.members.remove(&member);
                result.push(SDS::from_str(&member));
            }
        }

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                result.len(),
                to_remove,
                "Postcondition violated: must return exactly to_remove members"
            );
            debug_assert_eq!(
                self.members.len(),
                pre_len - to_remove,
                "Postcondition violated: len must decrease by to_remove after pop_count"
            );
        }

        self.verify_invariants();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_add_and_contains() {
        let mut set = RedisSet::new();

        // Add new member
        let added = set.add(SDS::from_str("apple"));
        assert!(added);
        assert_eq!(set.len(), 1);
        assert!(set.contains(&SDS::from_str("apple")));

        // Add duplicate
        let added = set.add(SDS::from_str("apple"));
        assert!(!added);
        assert_eq!(set.len(), 1);

        // Add another
        let added = set.add(SDS::from_str("banana"));
        assert!(added);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_set_remove() {
        let mut set = RedisSet::new();

        set.add(SDS::from_str("a"));
        set.add(SDS::from_str("b"));
        assert_eq!(set.len(), 2);

        // Remove existing
        let removed = set.remove(&SDS::from_str("a"));
        assert!(removed);
        assert_eq!(set.len(), 1);
        assert!(!set.contains(&SDS::from_str("a")));

        // Remove non-existing
        let removed = set.remove(&SDS::from_str("nonexistent"));
        assert!(!removed);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_set_members() {
        let mut set = RedisSet::new();

        set.add(SDS::from_str("x"));
        set.add(SDS::from_str("y"));
        set.add(SDS::from_str("z"));

        let members = set.members();
        assert_eq!(members.len(), 3);

        // Convert to strings for easier checking
        let member_strs: Vec<String> = members.iter().map(|m| m.to_string()).collect();
        assert!(member_strs.contains(&"x".to_string()));
        assert!(member_strs.contains(&"y".to_string()));
        assert!(member_strs.contains(&"z".to_string()));
    }

    #[test]
    fn test_set_invariants_maintained() {
        let mut set = RedisSet::new();

        // Empty set
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);

        // After add
        set.add(SDS::from_str("member1"));
        assert!(!set.is_empty());
        assert_eq!(set.len(), 1);

        // After duplicate add (no change)
        set.add(SDS::from_str("member1"));
        assert_eq!(set.len(), 1);

        // After second add
        set.add(SDS::from_str("member2"));
        assert_eq!(set.len(), 2);

        // After remove
        set.remove(&SDS::from_str("member1"));
        assert_eq!(set.len(), 1);
        assert!(!set.contains(&SDS::from_str("member1")));
        assert!(set.contains(&SDS::from_str("member2")));

        // After remove to empty
        set.remove(&SDS::from_str("member2"));
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }
}
