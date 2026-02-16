//! Skip List Implementation for Sorted Sets
//!
//! A probabilistic data structure providing O(log n) insert, delete, and search.
//! Used by Redis for sorted sets due to its simplicity and cache efficiency.

use std::cmp::Ordering;

const SKIPLIST_MAXLEVEL: usize = 32;
const SKIPLIST_P: f64 = 0.25; // Probability for level promotion

/// A node in the skip list
#[derive(Clone, Debug)]
struct SkipListNode {
    member: String,
    score: f64,
    /// Forward pointers and span at each level
    /// span[i] = number of nodes skipped at level i
    levels: Vec<SkipListLevel>,
    /// Backward pointer for reverse traversal
    backward: Option<usize>,
}

#[derive(Clone, Debug)]
struct SkipListLevel {
    forward: Option<usize>, // Index of next node at this level
    span: usize,            // Number of nodes between this and forward
}

/// Skip list data structure for sorted set
#[derive(Clone, Debug)]
pub struct SkipList {
    /// All nodes stored in a Vec (index 0 is header)
    nodes: Vec<Option<SkipListNode>>,
    /// Free list for reusing slots
    free_slots: Vec<usize>,
    /// Index of tail node
    tail: Option<usize>,
    /// Current max level in use
    level: usize,
    /// Number of elements
    length: usize,
    /// RNG state for level generation (simple xorshift)
    rng_state: u64,
}

impl SkipList {
    pub fn new() -> Self {
        // Create header node with max levels
        let header = SkipListNode {
            member: String::new(),
            score: 0.0,
            levels: (0..SKIPLIST_MAXLEVEL)
                .map(|_| SkipListLevel {
                    forward: None,
                    span: 0,
                })
                .collect(),
            backward: None,
        };

        SkipList {
            nodes: vec![Some(header)],
            free_slots: Vec::new(),
            tail: None,
            level: 1,
            length: 0,
            rng_state: 0x853c49e6748fea9b, // Initial seed
        }
    }

    /// Generate random level using geometric distribution
    fn random_level(&mut self) -> usize {
        let mut level = 1;
        // Xorshift64 for fast random numbers
        let mut x = self.rng_state;
        while level < SKIPLIST_MAXLEVEL {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.rng_state = x;
            // Check if random < SKIPLIST_P (using fixed point)
            if (x & 0xFFFF) as f64 / 65536.0 >= SKIPLIST_P {
                break;
            }
            level += 1;
        }
        level
    }

    /// Allocate a new node slot
    fn alloc_node(&mut self, member: String, score: f64, level: usize) -> usize {
        let node = SkipListNode {
            member,
            score,
            levels: (0..level)
                .map(|_| SkipListLevel {
                    forward: None,
                    span: 0,
                })
                .collect(),
            backward: None,
        };

        if let Some(idx) = self.free_slots.pop() {
            self.nodes[idx] = Some(node);
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(Some(node));
            idx
        }
    }

    /// Free a node slot
    fn free_node(&mut self, idx: usize) {
        self.nodes[idx] = None;
        self.free_slots.push(idx);
    }

    /// Compare (score, member) tuples
    #[inline]
    fn compare(score1: f64, member1: &str, score2: f64, member2: &str) -> Ordering {
        score1
            .partial_cmp(&score2)
            .unwrap_or(Ordering::Equal)
            .then_with(|| member1.cmp(member2))
    }

    /// Insert a new element. Returns true if new element, false if updated.
    pub fn insert(&mut self, member: String, score: f64) -> bool {
        let mut update = [0usize; SKIPLIST_MAXLEVEL];
        let mut rank = [0usize; SKIPLIST_MAXLEVEL];

        // Find position at each level
        let mut x = 0; // Start at header
        for i in (0..self.level).rev() {
            rank[i] = if i == self.level - 1 { 0 } else { rank[i + 1] };

            loop {
                let node = self.nodes[x].as_ref().expect("node must exist at valid index");
                if let Some(fwd) = node.levels[i].forward {
                    let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
                    if Self::compare(fwd_node.score, &fwd_node.member, score, &member)
                        == Ordering::Less
                    {
                        rank[i] += node.levels[i].span;
                        x = fwd;
                        continue;
                    }
                }
                break;
            }
            update[i] = x;
        }

        // Check if element already exists (would be right after x at level 0)
        let node = self.nodes[x].as_ref().expect("node must exist at valid index");
        if let Some(fwd) = node.levels[0].forward {
            let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
            if fwd_node.member == member {
                // Update score - need to reposition if score changed
                if (fwd_node.score - score).abs() > f64::EPSILON {
                    // Remove and re-insert with new score
                    self.delete_node(fwd, &update);
                    self.insert_internal(member, score, &mut update, &mut rank);
                }
                return false;
            }
        }

        // Insert new node
        self.insert_internal(member, score, &mut update, &mut rank);
        true
    }

    fn insert_internal(
        &mut self,
        member: String,
        score: f64,
        update: &mut [usize; SKIPLIST_MAXLEVEL],
        rank: &mut [usize; SKIPLIST_MAXLEVEL],
    ) {
        let level = self.random_level();
        let new_idx = self.alloc_node(member, score, level);

        // Initialize update/rank for new levels
        if level > self.level {
            for i in self.level..level {
                rank[i] = 0;
                update[i] = 0; // Header
                let header = self.nodes[0].as_mut().expect("node must exist at valid index");
                header.levels[i].span = self.length;
            }
            self.level = level;
        }

        // Update forward pointers and spans
        for i in 0..level {
            // Read values first to avoid borrow conflicts
            let old_forward = self.nodes[update[i]].as_ref().expect("node must exist at valid index").levels[i].forward;
            let old_span = self.nodes[update[i]].as_ref().expect("node must exist at valid index").levels[i].span;

            // Update new node
            let new_node = self.nodes[new_idx].as_mut().expect("node must exist at valid index");
            new_node.levels[i].forward = old_forward;
            new_node.levels[i].span = old_span - (rank[0] - rank[i]);

            // Update the predecessor node
            let update_node = self.nodes[update[i]].as_mut().expect("node must exist at valid index");
            update_node.levels[i].forward = Some(new_idx);
            update_node.levels[i].span = (rank[0] - rank[i]) + 1;
        }

        // Increment span for levels above the new node's level
        for i in level..self.level {
            let update_node = self.nodes[update[i]].as_mut().expect("node must exist at valid index");
            update_node.levels[i].span += 1;
        }

        // Set backward pointer
        let backward = if update[0] == 0 {
            None
        } else {
            Some(update[0])
        };
        self.nodes[new_idx].as_mut().expect("node must exist at valid index").backward = backward;

        // Update backward of next node or tail
        let new_fwd = self.nodes[new_idx].as_ref().expect("node must exist at valid index").levels[0].forward;
        if let Some(fwd) = new_fwd {
            self.nodes[fwd].as_mut().expect("node must exist at valid index").backward = Some(new_idx);
        } else {
            self.tail = Some(new_idx);
        }

        self.length += 1;
    }

    /// Delete a node given its index and update array
    fn delete_node(&mut self, idx: usize, update: &[usize; SKIPLIST_MAXLEVEL]) {
        // Update forward pointers and spans
        for i in 0..self.level {
            let update_fwd = self.nodes[update[i]].as_ref().expect("node must exist at valid index").levels[i].forward;
            if update_fwd == Some(idx) {
                // Read idx_node values first
                let idx_span = self.nodes[idx].as_ref().expect("node must exist at valid index").levels[i].span;
                let idx_fwd = self.nodes[idx].as_ref().expect("node must exist at valid index").levels[i].forward;

                let update_node = self.nodes[update[i]].as_mut().expect("node must exist at valid index");
                // Reorder arithmetic to avoid overflow: (span + idx_span) - 1 instead of span + (idx_span - 1)
                update_node.levels[i].span = update_node.levels[i].span + idx_span - 1;
                update_node.levels[i].forward = idx_fwd;
            } else {
                let update_node = self.nodes[update[i]].as_mut().expect("node must exist at valid index");
                update_node.levels[i].span -= 1;
            }
        }

        // Update backward pointer of next node
        let fwd = self.nodes[idx].as_ref().expect("node must exist at valid index").levels[0].forward;
        if let Some(fwd_idx) = fwd {
            self.nodes[fwd_idx].as_mut().expect("node must exist at valid index").backward =
                self.nodes[idx].as_ref().expect("node must exist at valid index").backward;
        } else {
            self.tail = self.nodes[idx].as_ref().expect("node must exist at valid index").backward;
        }

        // Update level if needed
        while self.level > 1 {
            let header = self.nodes[0].as_ref().expect("node must exist at valid index");
            if header.levels[self.level - 1].forward.is_some() {
                break;
            }
            self.level -= 1;
        }

        self.free_node(idx);
        self.length -= 1;
    }

    /// Remove an element by member name. Returns true if removed.
    pub fn remove(&mut self, member: &str) -> Option<f64> {
        let mut update = [0usize; SKIPLIST_MAXLEVEL];

        // Find the node to delete
        let mut x = 0;
        for i in (0..self.level).rev() {
            loop {
                let node = self.nodes[x].as_ref().expect("node must exist at valid index");
                if let Some(fwd) = node.levels[i].forward {
                    let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
                    // Need to find by member, so we need score first
                    if fwd_node.member.as_str() < member
                        || (fwd_node.member == member
                            && Self::compare(
                                fwd_node.score,
                                &fwd_node.member,
                                f64::INFINITY,
                                member,
                            ) == Ordering::Less)
                    {
                        x = fwd;
                        continue;
                    }
                }
                break;
            }
            update[i] = x;
        }

        // Check if we found the node
        let node = self.nodes[x].as_ref().expect("node must exist at valid index");
        if let Some(fwd) = node.levels[0].forward {
            let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
            if fwd_node.member == member {
                let score = fwd_node.score;
                self.delete_node(fwd, &update);
                return Some(score);
            }
        }

        None
    }

    /// Remove an element by member name and score. Returns true if removed.
    /// This method properly traverses the (score, member) ordered skiplist.
    pub fn remove_with_score(&mut self, member: &str, score: f64) -> bool {
        let mut update = [0usize; SKIPLIST_MAXLEVEL];

        // Find position at each level (same traversal as insert)
        let mut x = 0;
        for i in (0..self.level).rev() {
            loop {
                let node = self.nodes[x].as_ref().expect("node must exist at valid index");
                if let Some(fwd) = node.levels[i].forward {
                    let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
                    if Self::compare(fwd_node.score, &fwd_node.member, score, member)
                        == Ordering::Less
                    {
                        x = fwd;
                        continue;
                    }
                }
                break;
            }
            update[i] = x;
        }

        // Check if we found the exact element
        let node = self.nodes[x].as_ref().expect("node must exist at valid index");
        if let Some(fwd) = node.levels[0].forward {
            let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
            if fwd_node.member == member && (fwd_node.score - score).abs() < f64::EPSILON {
                self.delete_node(fwd, &update);
                return true;
            }
        }

        false
    }

    /// Get rank of element (0-indexed). Returns None if not found.
    pub fn rank(&self, member: &str, score: f64) -> Option<usize> {
        let mut rank = 0;
        let mut x = 0;

        for i in (0..self.level).rev() {
            loop {
                let node = self.nodes[x].as_ref().expect("node must exist at valid index");
                if let Some(fwd) = node.levels[i].forward {
                    let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
                    let cmp = Self::compare(fwd_node.score, &fwd_node.member, score, member);
                    if cmp == Ordering::Less
                        || (cmp == Ordering::Equal && fwd_node.member.as_str() < member)
                    {
                        rank += node.levels[i].span;
                        x = fwd;
                        continue;
                    }
                }
                break;
            }
        }

        // Check if element exists
        let node = self.nodes[x].as_ref().expect("node must exist at valid index");
        if let Some(fwd) = node.levels[0].forward {
            let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
            if fwd_node.member == member && (fwd_node.score - score).abs() < f64::EPSILON {
                return Some(rank);
            }
        }

        None
    }

    /// Get element by rank (0-indexed)
    pub fn get_by_rank(&self, rank: usize) -> Option<(&str, f64)> {
        if rank >= self.length {
            return None;
        }

        let mut traversed = 0;
        let mut x = 0;

        for i in (0..self.level).rev() {
            loop {
                let node = self.nodes[x].as_ref().expect("node must exist at valid index");
                if let Some(fwd) = node.levels[i].forward {
                    if traversed + node.levels[i].span <= rank {
                        traversed += node.levels[i].span;
                        x = fwd;
                        continue;
                    }
                }
                break;
            }
        }

        // Move one more step to get the element at rank
        let node = self.nodes[x].as_ref().expect("node must exist at valid index");
        if let Some(fwd) = node.levels[0].forward {
            let fwd_node = self.nodes[fwd].as_ref().expect("node must exist at valid index");
            return Some((&fwd_node.member, fwd_node.score));
        }

        None
    }

    /// Get range of elements by rank [start, end] (inclusive, 0-indexed)
    pub fn range(&self, start: usize, end: usize) -> Vec<(&str, f64)> {
        if start > end || start >= self.length {
            return Vec::new();
        }

        let end = end.min(self.length - 1);
        let mut result = Vec::with_capacity(end - start + 1);

        // Find start position
        let mut traversed = 0;
        let mut x = 0;

        for i in (0..self.level).rev() {
            loop {
                let node = self.nodes[x].as_ref().expect("node must exist at valid index");
                if let Some(fwd) = node.levels[i].forward {
                    if traversed + node.levels[i].span <= start {
                        traversed += node.levels[i].span;
                        x = fwd;
                        continue;
                    }
                }
                break;
            }
        }

        // Collect elements
        let node = self.nodes[x].as_ref().expect("node must exist at valid index");
        let mut current = node.levels[0].forward;

        for _ in start..=end {
            if let Some(idx) = current {
                let n = self.nodes[idx].as_ref().expect("node must exist at valid index");
                result.push((n.member.as_str(), n.score));
                current = n.levels[0].forward;
            } else {
                break;
            }
        }

        result
    }

    /// Get range in reverse order by rank
    pub fn rev_range(&self, start: usize, end: usize) -> Vec<(&str, f64)> {
        if start > end || start >= self.length {
            return Vec::new();
        }

        let end = end.min(self.length - 1);

        // Convert to forward indices and collect
        let fwd_start = self.length - 1 - end;
        let fwd_end = self.length - 1 - start;

        let forward = self.range(fwd_start, fwd_end);
        forward.into_iter().rev().collect()
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Iterate over all elements in order
    pub fn iter(&self) -> SkipListIter<'_> {
        let header = self.nodes[0].as_ref().expect("node must exist at valid index");
        SkipListIter {
            skiplist: self,
            current: header.levels[0].forward,
        }
    }
}

pub struct SkipListIter<'a> {
    skiplist: &'a SkipList,
    current: Option<usize>,
}

impl<'a> Iterator for SkipListIter<'a> {
    type Item = (&'a str, f64);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(idx) = self.current {
            let node = self.skiplist.nodes[idx].as_ref().expect("node must exist at valid index");
            self.current = node.levels[0].forward;
            Some((&node.member, node.score))
        } else {
            None
        }
    }
}
