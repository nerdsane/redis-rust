//! Redis Sorted Set using Skip List for O(log n) operations

use super::{SkipList, SDS};
use ahash::AHashMap;

/// Redis Sorted Set using Skip List for O(log n) operations
#[derive(Clone, Debug)]
pub struct RedisSortedSet {
    /// HashMap for O(1) score lookup by member
    members: AHashMap<String, f64>,
    /// Skip list for O(log n) sorted operations
    skiplist: SkipList,
}

impl RedisSortedSet {
    pub fn new() -> Self {
        RedisSortedSet {
            members: AHashMap::new(),
            skiplist: SkipList::new(),
        }
    }

    /// VOPR: Verify all invariants hold for this sorted set
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: members and skiplist must have same length
        debug_assert_eq!(
            self.members.len(),
            self.skiplist.len(),
            "Invariant violated: members.len() ({}) != skiplist.len() ({})",
            self.members.len(),
            self.skiplist.len()
        );

        // Invariant 2: Every member in HashMap must be in skiplist with matching score
        for (member, score) in &self.members {
            let rank = self.skiplist.rank(member, *score);
            debug_assert!(
                rank.is_some(),
                "Invariant violated: member '{}' with score {} in HashMap but not found in skiplist",
                member,
                score
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    /// Add member with score. Returns true if new member, false if updated.
    pub fn add(&mut self, member: SDS, score: f64) -> bool {
        let key = member.to_string();

        // TigerStyle: Preconditions
        #[cfg(debug_assertions)]
        let pre_len = self.members.len();

        // Check if member already exists - use entry API to avoid double lookup
        use std::collections::hash_map::Entry;
        match self.members.entry(key) {
            Entry::Occupied(mut entry) => {
                let old_score = *entry.get();
                if (old_score - score).abs() < f64::EPSILON {
                    return false; // Score unchanged
                }
                // Update score
                entry.insert(score);
                // Update skiplist: remove old entry, insert new
                let key_ref = entry.key();
                self.skiplist.remove_with_score(key_ref, old_score);
                self.skiplist.insert(key_ref.clone(), score);

                #[cfg(debug_assertions)]
                self.verify_invariants();
                false
            }
            Entry::Vacant(entry) => {
                // New member - insert into skiplist first with cloned key,
                // then insert key into hashmap
                let key_for_skiplist = entry.key().clone();
                entry.insert(score);
                self.skiplist.insert(key_for_skiplist, score);

                // TigerStyle: Postconditions
                #[cfg(debug_assertions)]
                {
                    debug_assert_eq!(
                        self.members.len(),
                        pre_len + 1,
                        "Postcondition violated: len must increase by 1"
                    );
                    self.verify_invariants();
                }

                true
            }
        }
    }

    /// Remove member. Returns true if removed.
    pub fn remove(&mut self, member: &SDS) -> bool {
        let key = member.to_string();

        #[cfg(debug_assertions)]
        let pre_len = self.members.len();
        #[cfg(debug_assertions)]
        let existed = self.members.contains_key(&key);

        // Get score before removing from members (needed for skiplist removal)
        let score = self.members.get(&key).copied();
        let removed = self.members.remove(&key).is_some();
        if removed {
            // Use remove_with_score since skiplist is ordered by (score, member)
            // and we need the score to find the correct entry
            if let Some(score) = score {
                self.skiplist.remove_with_score(&key, score);
            }
        }

        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(removed, existed);
            if existed {
                debug_assert_eq!(self.members.len(), pre_len - 1);
            }
            self.verify_invariants();
        }

        removed
    }

    /// Get score of member. O(1)
    pub fn score(&self, member: &SDS) -> Option<f64> {
        self.members.get(&member.to_string()).copied()
    }

    /// Get rank of member (0-indexed). O(log n)
    pub fn rank(&self, member: &SDS) -> Option<usize> {
        let key = member.to_string();
        let score = self.members.get(&key)?;
        self.skiplist.rank(&key, *score)
    }

    /// Get range by rank [start, stop] (inclusive). O(log n + k)
    pub fn range(&self, start: isize, stop: isize) -> Vec<(SDS, f64)> {
        let len = self.skiplist.len() as isize;
        if len == 0 {
            return Vec::new();
        }

        let start = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let stop = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

        if start > stop || start >= len {
            return Vec::new();
        }

        self.skiplist
            .range(start as usize, stop as usize)
            .into_iter()
            .map(|(m, s)| (SDS::from_str(m), s))
            .collect()
    }

    /// Get range in reverse by rank. O(log n + k)
    pub fn rev_range(&self, start: isize, stop: isize) -> Vec<(SDS, f64)> {
        let len = self.skiplist.len() as isize;
        if len == 0 {
            return Vec::new();
        }

        let start = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let stop = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

        if start > stop || start >= len {
            return Vec::new();
        }

        self.skiplist
            .rev_range(start as usize, stop as usize)
            .into_iter()
            .map(|(m, s)| (SDS::from_str(m), s))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Get the skiplist length (for DST invariant checking)
    pub fn skiplist_len(&self) -> usize {
        self.skiplist.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Check if the set is sorted. Always true for a correctly functioning skiplist.
    pub fn is_sorted(&self) -> bool {
        let mut prev_score = f64::NEG_INFINITY;
        let mut prev_member = String::new();
        for (member, score) in self.skiplist.iter() {
            if score < prev_score || (score == prev_score && member < prev_member.as_str()) {
                return false;
            }
            prev_score = score;
            prev_member = member.to_string();
        }
        true
    }

    /// Parse score bound (handles -inf, +inf, exclusive with parenthesis)
    fn parse_score_bound(s: &str, _is_min: bool) -> Result<(f64, bool), String> {
        let s = s.trim();
        if s == "-inf" {
            return Ok((f64::NEG_INFINITY, false));
        }
        if s == "+inf" || s == "inf" {
            return Ok((f64::INFINITY, false));
        }

        let (exclusive, num_str) = if s.starts_with('(') {
            (true, &s[1..])
        } else {
            (false, s)
        };

        let score = num_str
            .parse::<f64>()
            .map_err(|_| "ERR min or max is not a float".to_string())?;

        Ok((score, exclusive))
    }

    /// ZCOUNT - count elements in score range. O(log n + k)
    pub fn count_in_range(&self, min: &str, max: &str) -> Result<usize, String> {
        let (min_score, min_exclusive) = Self::parse_score_bound(min, true)?;
        let (max_score, max_exclusive) = Self::parse_score_bound(max, false)?;

        let count = self
            .skiplist
            .iter()
            .filter(|(_, score)| {
                let above_min = if min_exclusive {
                    *score > min_score
                } else {
                    *score >= min_score
                };
                let below_max = if max_exclusive {
                    *score < max_score
                } else {
                    *score <= max_score
                };
                above_min && below_max
            })
            .count();

        Ok(count)
    }

    /// ZRANGEBYSCORE - get elements by score range. O(log n + k)
    pub fn range_by_score(
        &self,
        min: &str,
        max: &str,
        with_scores: bool,
        limit: Option<(isize, usize)>,
    ) -> Result<Vec<(String, Option<f64>)>, String> {
        let (min_score, min_exclusive) = Self::parse_score_bound(min, true)?;
        let (max_score, max_exclusive) = Self::parse_score_bound(max, false)?;

        let mut results: Vec<_> = self
            .skiplist
            .iter()
            .filter(|(_, score)| {
                let above_min = if min_exclusive {
                    *score > min_score
                } else {
                    *score >= min_score
                };
                let below_max = if max_exclusive {
                    *score < max_score
                } else {
                    *score <= max_score
                };
                above_min && below_max
            })
            .map(|(member, score)| {
                (
                    member.to_string(),
                    if with_scores { Some(score) } else { None },
                )
            })
            .collect();

        // Apply LIMIT
        if let Some((offset, count)) = limit {
            let start = offset.max(0) as usize;
            results = results.into_iter().skip(start).take(count).collect();
        }

        Ok(results)
    }

    /// Iterate over member-score pairs in sorted order (for ZSCAN). O(n)
    pub fn iter(&self) -> impl Iterator<Item = (&str, f64)> {
        self.skiplist.iter()
    }
}

impl PartialEq for RedisSortedSet {
    fn eq(&self, other: &Self) -> bool {
        self.members == other.members
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skiplist_remove_with_score() {
        let mut sl = SkipList::new();

        // Insert and check length
        sl.insert("a".to_string(), 1.0);
        assert_eq!(sl.len(), 1, "After insert(a, 1.0)");

        // Remove with correct score
        let removed = sl.remove_with_score("a", 1.0);
        assert!(removed, "remove_with_score should return true");
        assert_eq!(sl.len(), 0, "After remove_with_score(a, 1.0)");

        // Insert at different score
        sl.insert("a".to_string(), 5.0);
        assert_eq!(sl.len(), 1, "After insert(a, 5.0)");

        // Verify we can find it at new score
        assert!(sl.rank("a", 5.0).is_some(), "Should find a at score 5.0");
        assert!(
            sl.rank("a", 1.0).is_none(),
            "Should NOT find a at score 1.0"
        );
    }

    fn create_test_set() -> RedisSortedSet {
        let mut zset = RedisSortedSet::new();
        zset.add(SDS::from_str("alice"), 100.0);
        zset.add(SDS::from_str("bob"), 200.0);
        zset.add(SDS::from_str("charlie"), 150.0);
        zset.add(SDS::from_str("dave"), 50.0);
        zset
    }

    #[test]
    fn test_sorted_set_ordering() {
        let zset = create_test_set();

        // Should be sorted by score: dave(50), alice(100), charlie(150), bob(200)
        let range = zset.range(0, -1);
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].0.to_string(), "dave");
        assert_eq!(range[0].1, 50.0);
        assert_eq!(range[1].0.to_string(), "alice");
        assert_eq!(range[2].0.to_string(), "charlie");
        assert_eq!(range[3].0.to_string(), "bob");
    }

    #[test]
    fn test_rev_range_full() {
        let zset = create_test_set();

        // rev_range should return highest scores first: bob(200), charlie(150), alice(100), dave(50)
        let range = zset.rev_range(0, -1);
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].0.to_string(), "bob");
        assert_eq!(range[0].1, 200.0);
        assert_eq!(range[1].0.to_string(), "charlie");
        assert_eq!(range[2].0.to_string(), "alice");
        assert_eq!(range[3].0.to_string(), "dave");
    }

    #[test]
    fn test_rev_range_subset() {
        let zset = create_test_set();

        // Get top 2 scores
        let range = zset.rev_range(0, 1);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].0.to_string(), "bob");
        assert_eq!(range[1].0.to_string(), "charlie");
    }

    #[test]
    fn test_rev_range_negative_indices() {
        let zset = create_test_set();

        // Last 2 elements in reverse order (lowest scores)
        let range = zset.rev_range(-2, -1);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].0.to_string(), "alice");
        assert_eq!(range[1].0.to_string(), "dave");
    }

    #[test]
    fn test_rev_range_empty_set() {
        let zset = RedisSortedSet::new();
        let range = zset.rev_range(0, -1);
        assert!(range.is_empty());
    }

    #[test]
    fn test_rev_range_out_of_bounds() {
        let zset = create_test_set();

        // Start beyond length
        let range = zset.rev_range(10, 20);
        assert!(range.is_empty());

        // Invalid range (start > stop after normalization)
        let range = zset.rev_range(3, 1);
        assert!(range.is_empty());
    }

    #[test]
    fn test_range_vs_rev_range_symmetry() {
        let zset = create_test_set();

        let forward = zset.range(0, -1);
        let reverse = zset.rev_range(0, -1);

        assert_eq!(forward.len(), reverse.len());

        // First element of forward should equal last element of reverse
        assert_eq!(forward[0].0.to_string(), reverse[3].0.to_string());
        assert_eq!(forward[3].0.to_string(), reverse[0].0.to_string());
    }

    #[test]
    fn test_sorted_set_with_equal_scores() {
        let mut zset = RedisSortedSet::new();
        zset.add(SDS::from_str("zebra"), 100.0);
        zset.add(SDS::from_str("apple"), 100.0);
        zset.add(SDS::from_str("mango"), 100.0);

        // Same score: should be sorted lexicographically
        let range = zset.range(0, -1);
        assert_eq!(range[0].0.to_string(), "apple");
        assert_eq!(range[1].0.to_string(), "mango");
        assert_eq!(range[2].0.to_string(), "zebra");

        // rev_range with equal scores: reverse lexicographic within same score
        let rev = zset.rev_range(0, -1);
        assert_eq!(rev[0].0.to_string(), "zebra");
        assert_eq!(rev[1].0.to_string(), "mango");
        assert_eq!(rev[2].0.to_string(), "apple");
    }

    #[test]
    fn test_sorted_set_invariants_maintained() {
        let mut zset = RedisSortedSet::new();

        // Add elements
        zset.add(SDS::from_str("a"), 1.0);
        assert_eq!(zset.len(), 1);
        assert!(zset.is_sorted());

        // Update score
        zset.add(SDS::from_str("a"), 5.0);
        assert_eq!(zset.len(), 1); // Should not add duplicate
        assert_eq!(zset.score(&SDS::from_str("a")), Some(5.0));
        assert!(zset.is_sorted());

        // Add more and remove
        zset.add(SDS::from_str("b"), 3.0);
        zset.add(SDS::from_str("c"), 7.0);
        assert_eq!(zset.len(), 3);
        assert!(zset.is_sorted());

        zset.remove(&SDS::from_str("b"));
        assert_eq!(zset.len(), 2);
        assert!(zset.is_sorted());
    }
}
