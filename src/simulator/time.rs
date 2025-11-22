use std::ops::{Add, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualTime(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(pub u64);

impl VirtualTime {
    pub const ZERO: VirtualTime = VirtualTime(0);

    pub fn as_millis(&self) -> u64 {
        self.0
    }

    pub fn from_millis(millis: u64) -> Self {
        VirtualTime(millis)
    }

    pub fn from_secs(secs: u64) -> Self {
        VirtualTime(secs * 1000)
    }
}

impl Add<Duration> for VirtualTime {
    type Output = VirtualTime;

    fn add(self, rhs: Duration) -> Self::Output {
        VirtualTime(self.0 + rhs.0)
    }
}

impl Sub<VirtualTime> for VirtualTime {
    type Output = Duration;

    fn sub(self, rhs: VirtualTime) -> Self::Output {
        Duration(self.0 - rhs.0)
    }
}

impl Duration {
    pub const ZERO: Duration = Duration(0);

    pub fn from_millis(millis: u64) -> Self {
        Duration(millis)
    }

    pub fn from_secs(secs: u64) -> Self {
        Duration(secs * 1000)
    }

    pub fn as_millis(&self) -> u64 {
        self.0
    }
}
