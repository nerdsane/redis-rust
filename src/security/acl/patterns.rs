//! Key pattern matching for ACL

/// A key pattern (glob-style)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPattern {
    /// The pattern string (e.g., "user:*", "cache:*")
    pub pattern: String,
    /// Whether this is a read pattern (vs write)
    pub read: bool,
    /// Whether this is a write pattern
    pub write: bool,
}

impl KeyPattern {
    /// Create a new pattern allowing both read and write
    pub fn new(pattern: String) -> Self {
        Self {
            pattern,
            read: true,
            write: true,
        }
    }

    /// Create a read-only pattern
    pub fn read_only(pattern: String) -> Self {
        Self {
            pattern,
            read: true,
            write: false,
        }
    }

    /// Create a write-only pattern
    pub fn write_only(pattern: String) -> Self {
        Self {
            pattern,
            read: false,
            write: true,
        }
    }

    /// Check if a key matches this pattern
    pub fn matches(&self, key: &str) -> bool {
        glob_match(&self.pattern, key)
    }
}

/// Key patterns for a user
#[derive(Debug, Clone)]
pub struct KeyPatterns {
    /// Allow all keys
    pub allow_all: bool,
    /// Specific patterns
    pub patterns: Vec<KeyPattern>,
}

impl KeyPatterns {
    /// Create patterns that allow all keys
    pub fn allow_all() -> Self {
        Self {
            allow_all: true,
            patterns: Vec::new(),
        }
    }

    /// Create patterns that deny all keys
    pub fn deny_all() -> Self {
        Self {
            allow_all: false,
            patterns: Vec::new(),
        }
    }

    /// Add a pattern
    pub fn add_pattern(&mut self, pattern: KeyPattern) {
        self.patterns.push(pattern);
    }

    /// Add a pattern from string (e.g., "user:*")
    pub fn add(&mut self, pattern: &str) {
        self.patterns.push(KeyPattern::new(pattern.to_string()));
    }

    /// Check if a key is permitted
    pub fn is_key_permitted(&self, key: &str) -> bool {
        if self.allow_all {
            return true;
        }

        // Check if any pattern matches
        for pattern in &self.patterns {
            if pattern.matches(key) {
                return true;
            }
        }

        false
    }

    /// Reset to deny all
    pub fn reset(&mut self) {
        self.allow_all = false;
        self.patterns.clear();
    }

    /// Reset to allow all
    pub fn reset_all(&mut self) {
        self.allow_all = true;
        self.patterns.clear();
    }
}

impl Default for KeyPatterns {
    fn default() -> Self {
        Self::allow_all()
    }
}

/// Simple glob pattern matching (supports * and ?)
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    glob_match_impl(&pattern_chars, &text_chars)
}

fn glob_match_impl(pattern: &[char], text: &[char]) -> bool {
    let mut p = 0;
    let mut t = 0;
    let mut star_p = None;
    let mut star_t = None;

    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                '?' => {
                    // ? matches any single character
                    p += 1;
                    t += 1;
                    continue;
                }
                '*' => {
                    // * matches zero or more characters
                    star_p = Some(p);
                    star_t = Some(t);
                    p += 1;
                    continue;
                }
                c if c == text[t] => {
                    p += 1;
                    t += 1;
                    continue;
                }
                _ => {}
            }
        }

        // No match - try to backtrack to last *
        if let (Some(sp), Some(st)) = (star_p, star_t) {
            p = sp + 1;
            star_t = Some(st + 1);
            t = st + 1;
        } else {
            return false;
        }
    }

    // Check remaining pattern characters (should all be *)
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }

    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("foo", "foo"));
        assert!(!glob_match("foo", "bar"));
        assert!(!glob_match("foo", "foobar"));
        assert!(!glob_match("foobar", "foo"));
    }

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(glob_match("foo*", "foo"));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("*bar", "bar"));
        assert!(glob_match("foo*bar", "foobar"));
        assert!(glob_match("foo*bar", "foo123bar"));
        assert!(!glob_match("foo*bar", "foobarbaz"));
    }

    #[test]
    fn test_glob_match_question() {
        assert!(glob_match("fo?", "foo"));
        assert!(glob_match("fo?", "for"));
        assert!(!glob_match("fo?", "fo"));
        assert!(!glob_match("fo?", "fooo"));
        assert!(glob_match("???", "abc"));
        assert!(!glob_match("???", "ab"));
    }

    #[test]
    fn test_glob_match_complex() {
        assert!(glob_match("user:*", "user:123"));
        assert!(glob_match("user:*", "user:"));
        assert!(!glob_match("user:*", "admin:123"));
        assert!(glob_match("cache:*:data", "cache:foo:data"));
        assert!(glob_match("*:*:*", "a:b:c"));
        assert!(glob_match("user:???:*", "user:abc:data"));
        assert!(!glob_match("user:???:*", "user:ab:data"));
    }

    #[test]
    fn test_key_patterns() {
        let mut patterns = KeyPatterns::deny_all();
        patterns.add("user:*");
        patterns.add("cache:*");

        assert!(patterns.is_key_permitted("user:123"));
        assert!(patterns.is_key_permitted("cache:foo"));
        assert!(!patterns.is_key_permitted("admin:secret"));
        assert!(!patterns.is_key_permitted("other"));
    }

    #[test]
    fn test_key_patterns_allow_all() {
        let patterns = KeyPatterns::allow_all();
        assert!(patterns.is_key_permitted("anything"));
        assert!(patterns.is_key_permitted(""));
    }
}
