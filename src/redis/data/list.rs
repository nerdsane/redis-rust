//! Redis List data structure

use super::SDS;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq)]
pub struct RedisList {
    items: VecDeque<SDS>,
}

impl RedisList {
    pub fn new() -> Self {
        RedisList {
            items: VecDeque::new(),
        }
    }

    /// Verify all invariants hold for this list
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant 1: len() must match actual VecDeque size
        debug_assert_eq!(
            self.len(),
            self.items.len(),
            "Invariant violated: len() must equal items.len()"
        );

        // Invariant 2: is_empty() must be consistent with len()
        debug_assert_eq!(
            self.is_empty(),
            self.items.is_empty(),
            "Invariant violated: is_empty() must equal items.is_empty()"
        );

        // Invariant 3: is_empty() iff len() == 0
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() must equal len() == 0"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    pub fn lpush(&mut self, value: SDS) {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();

        self.items.push_front(value.clone());

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                pre_len + 1,
                "Postcondition violated: len must increase by 1 after lpush"
            );
            debug_assert_eq!(
                self.items.front().map(|v| v.to_string()),
                Some(value.to_string()),
                "Postcondition violated: pushed value must be at front"
            );
        }

        self.verify_invariants();
    }

    pub fn rpush(&mut self, value: SDS) {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();

        self.items.push_back(value.clone());

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                pre_len + 1,
                "Postcondition violated: len must increase by 1 after rpush"
            );
            debug_assert_eq!(
                self.items.back().map(|v| v.to_string()),
                Some(value.to_string()),
                "Postcondition violated: pushed value must be at back"
            );
        }

        self.verify_invariants();
    }

    pub fn lpop(&mut self) -> Option<SDS> {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();
        #[cfg(debug_assertions)]
        let was_empty = self.items.is_empty();

        let result = self.items.pop_front();

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            if was_empty {
                debug_assert!(
                    result.is_none(),
                    "Postcondition violated: pop from empty must return None"
                );
                debug_assert_eq!(
                    self.items.len(),
                    0,
                    "Postcondition violated: empty list must stay empty"
                );
            } else {
                debug_assert!(
                    result.is_some(),
                    "Postcondition violated: pop from non-empty must return Some"
                );
                debug_assert_eq!(
                    self.items.len(),
                    pre_len - 1,
                    "Postcondition violated: len must decrease by 1"
                );
            }
        }

        self.verify_invariants();
        result
    }

    pub fn rpop(&mut self) -> Option<SDS> {
        #[cfg(debug_assertions)]
        let pre_len = self.items.len();
        #[cfg(debug_assertions)]
        let was_empty = self.items.is_empty();

        let result = self.items.pop_back();

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            if was_empty {
                debug_assert!(
                    result.is_none(),
                    "Postcondition violated: pop from empty must return None"
                );
                debug_assert_eq!(
                    self.items.len(),
                    0,
                    "Postcondition violated: empty list must stay empty"
                );
            } else {
                debug_assert!(
                    result.is_some(),
                    "Postcondition violated: pop from non-empty must return Some"
                );
                debug_assert_eq!(
                    self.items.len(),
                    pre_len - 1,
                    "Postcondition violated: len must decrease by 1"
                );
            }
        }

        self.verify_invariants();
        result
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn range(&self, start: isize, stop: isize) -> Vec<SDS> {
        let len = self.items.len() as isize;
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

        self.items
            .iter()
            .skip(start as usize)
            .take((stop - start + 1) as usize)
            .cloned()
            .collect()
    }

    /// LINDEX - get element at index
    pub fn get(&self, index: isize) -> Option<&SDS> {
        let len = self.items.len() as isize;
        let idx = if index < 0 { len + index } else { index };

        if idx < 0 || idx >= len {
            return None;
        }

        self.items.get(idx as usize)
    }

    /// LSET - set element at index (for LSET command)
    pub fn set(&mut self, index: isize, value: SDS) -> Result<(), String> {
        let len = self.items.len() as isize;

        // TigerStyle: Preconditions
        debug_assert!(len > 0, "Precondition: list must not be empty for LSET");

        let idx = if index < 0 { len + index } else { index };

        if idx < 0 || idx >= len {
            return Err("ERR index out of range".to_string());
        }

        #[cfg(debug_assertions)]
        let pre_len = self.items.len();

        self.items[idx as usize] = value.clone();

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                pre_len,
                "Postcondition violated: length must not change after LSET"
            );
            debug_assert_eq!(
                self.items[idx as usize].to_string(),
                value.to_string(),
                "Postcondition violated: value must be set at index"
            );
        }

        self.verify_invariants();
        Ok(())
    }

    /// LTRIM - trim list to specified range
    pub fn trim(&mut self, start: isize, stop: isize) {
        let len = self.items.len() as isize;
        if len == 0 {
            return;
        }

        // Normalize indices
        let s = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let e = if stop < 0 {
            (len + stop).max(-1)
        } else {
            stop.min(len - 1)
        };

        if s > e || s >= len {
            self.items.clear();
            self.verify_invariants();
            return;
        }

        // Keep only elements in range
        let new_items: VecDeque<SDS> = self
            .items
            .iter()
            .skip(s as usize)
            .take((e - s + 1) as usize)
            .cloned()
            .collect();

        #[cfg(debug_assertions)]
        let expected_len = (e - s + 1) as usize;

        self.items = new_items;

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.items.len(),
                expected_len,
                "Postcondition violated: length must equal trimmed range size"
            );
        }

        self.verify_invariants();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_lpush_and_lpop() {
        let mut list = RedisList::new();

        // lpush adds to front
        list.lpush(SDS::from_str("first"));
        assert_eq!(list.len(), 1);
        assert!(!list.is_empty());

        list.lpush(SDS::from_str("second"));
        assert_eq!(list.len(), 2);

        // lpop removes from front (LIFO for lpush/lpop)
        let val = list.lpop();
        assert_eq!(val.unwrap().to_string(), "second");
        assert_eq!(list.len(), 1);

        let val = list.lpop();
        assert_eq!(val.unwrap().to_string(), "first");
        assert!(list.is_empty());

        // lpop from empty
        let val = list.lpop();
        assert!(val.is_none());
    }

    #[test]
    fn test_list_rpush_and_rpop() {
        let mut list = RedisList::new();

        // rpush adds to back
        list.rpush(SDS::from_str("first"));
        list.rpush(SDS::from_str("second"));
        assert_eq!(list.len(), 2);

        // rpop removes from back (LIFO for rpush/rpop)
        let val = list.rpop();
        assert_eq!(val.unwrap().to_string(), "second");

        let val = list.rpop();
        assert_eq!(val.unwrap().to_string(), "first");
        assert!(list.is_empty());

        // rpop from empty
        let val = list.rpop();
        assert!(val.is_none());
    }

    #[test]
    fn test_list_queue_behavior() {
        let mut list = RedisList::new();

        // rpush + lpop = FIFO queue
        list.rpush(SDS::from_str("a"));
        list.rpush(SDS::from_str("b"));
        list.rpush(SDS::from_str("c"));

        assert_eq!(list.lpop().unwrap().to_string(), "a");
        assert_eq!(list.lpop().unwrap().to_string(), "b");
        assert_eq!(list.lpop().unwrap().to_string(), "c");
    }

    #[test]
    fn test_list_range() {
        let mut list = RedisList::new();

        list.rpush(SDS::from_str("a"));
        list.rpush(SDS::from_str("b"));
        list.rpush(SDS::from_str("c"));
        list.rpush(SDS::from_str("d"));

        // Full range
        let range = list.range(0, -1);
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].to_string(), "a");
        assert_eq!(range[3].to_string(), "d");

        // Subset
        let range = list.range(1, 2);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].to_string(), "b");
        assert_eq!(range[1].to_string(), "c");

        // Negative indices
        let range = list.range(-2, -1);
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].to_string(), "c");
        assert_eq!(range[1].to_string(), "d");
    }

    #[test]
    fn test_list_invariants_maintained() {
        let mut list = RedisList::new();

        // Empty list invariants
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);

        // After lpush
        list.lpush(SDS::from_str("x"));
        assert!(!list.is_empty());
        assert_eq!(list.len(), 1);

        // After rpush
        list.rpush(SDS::from_str("y"));
        assert_eq!(list.len(), 2);

        // After lpop
        list.lpop();
        assert_eq!(list.len(), 1);

        // After rpop to empty
        list.rpop();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }
}
