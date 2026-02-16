//! Simulated Runtime for Deterministic Simulation Testing
//!
//! This module provides a simulated I/O runtime that allows:
//! - Deterministic virtual time control
//! - Network fault injection (drops, corruption, reordering)
//! - Clock skew simulation per-node
//! - Full replay capability given the same seed
//!
//! Inspired by FoundationDB's simulation framework and TigerBeetle's IO abstraction.

use super::{
    Clock, Duration, Network, NetworkListener, NetworkStream, Rng, Runtime, Ticker, TimeSource,
    Timestamp,
};
use crate::buggify::{self, faults, FaultConfig};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

// Helper function to check buggify - works around macro import issues
#[inline]
fn check_buggify<R: Rng>(rng: &mut R, fault_id: &str) -> bool {
    buggify::should_buggify(rng, fault_id)
}

/// Node identifier in the simulation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

/// Simulated runtime providing deterministic I/O
pub struct SimulatedRuntime {
    ctx: Arc<SimulationContext>,
    node_id: NodeId,
}

impl SimulatedRuntime {
    pub fn new(ctx: Arc<SimulationContext>, node_id: NodeId) -> Self {
        SimulatedRuntime { ctx, node_id }
    }

    /// Get mutable reference to RNG
    pub fn rng(&self) -> impl std::ops::DerefMut<Target = SimulatedRng> + '_ {
        self.ctx.rng.lock().expect("mutex poisoned")
    }
}

impl std::fmt::Debug for SimulatedRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatedRuntime")
            .field("node_id", &self.node_id)
            .finish()
    }
}

impl Runtime for SimulatedRuntime {
    type Clock = SimulatedClock;
    type Network = SimulatedNetwork;

    fn clock(&self) -> &Self::Clock {
        // Return a reference that captures our context
        // This is a bit of a hack - we store clocks per-node
        unsafe {
            // Safety: We're returning a reference to data that lives as long as ctx
            &*(&SimulatedClock {
                ctx: self.ctx.clone(),
                node_id: self.node_id,
            } as *const SimulatedClock)
        }
    }

    fn network(&self) -> &Self::Network {
        unsafe {
            &*(&SimulatedNetwork {
                ctx: self.ctx.clone(),
                node_id: self.node_id,
            } as *const SimulatedNetwork)
        }
    }

    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        // In simulation, we queue tasks for later execution
        let mut tasks = self.ctx.pending_tasks.lock().expect("mutex poisoned");
        tasks.push(Box::pin(future));
    }
}

/// Shared simulation context - contains all global state
pub struct SimulationContext {
    /// Current global time
    time: Mutex<Timestamp>,
    /// Per-node clock offsets for skew simulation
    clock_offsets: Mutex<HashMap<NodeId, ClockOffset>>,
    /// Network state
    network_state: Mutex<NetworkState>,
    /// Deterministic RNG
    rng: Mutex<SimulatedRng>,
    /// Fault configuration
    #[allow(dead_code)]
    fault_config: FaultConfig,
    /// Pending timers (min-heap by wake time)
    timers: Mutex<BinaryHeap<TimerEntry>>,
    /// Pending tasks to execute
    pending_tasks: Mutex<Vec<Pin<Box<dyn Future<Output = ()> + Send>>>>,
    /// Next unique ID for various purposes
    next_id: Mutex<u64>,
}

impl SimulationContext {
    pub fn new(seed: u64, fault_config: FaultConfig) -> Self {
        buggify::set_config(fault_config.clone());

        SimulationContext {
            time: Mutex::new(Timestamp::from_millis(0)),
            clock_offsets: Mutex::new(HashMap::new()),
            network_state: Mutex::new(NetworkState::new()),
            rng: Mutex::new(SimulatedRng::new(seed)),
            fault_config,
            timers: Mutex::new(BinaryHeap::new()),
            pending_tasks: Mutex::new(Vec::new()),
            next_id: Mutex::new(0),
        }
    }

    /// Get current global time
    pub fn now(&self) -> Timestamp {
        *self.time.lock().expect("mutex poisoned")
    }

    /// Advance time to the specified timestamp
    pub fn advance_to(&self, time: Timestamp) {
        let mut t = self.time.lock().expect("mutex poisoned");
        if time > *t {
            *t = time;
        }
    }

    /// Advance time by duration
    pub fn advance_by(&self, duration: Duration) {
        let mut t = self.time.lock().expect("mutex poisoned");
        *t = *t + duration;
    }

    /// Get next unique ID
    pub fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().expect("mutex poisoned");
        let result = *id;
        *id += 1;
        result
    }

    /// Set clock offset for a node (for skew simulation)
    pub fn set_clock_offset(&self, node: NodeId, offset: ClockOffset) {
        self.clock_offsets.lock().expect("mutex poisoned").insert(node, offset);
    }

    /// Get local time for a node (applies clock offset)
    pub fn local_time(&self, node: NodeId) -> Timestamp {
        let global = self.now();
        let offsets = self.clock_offsets.lock().expect("mutex poisoned");
        if let Some(offset) = offsets.get(&node) {
            offset.apply(global)
        } else {
            global
        }
    }

    /// Add a timer
    pub fn add_timer(&self, wake_time: Timestamp, waker: Waker) -> u64 {
        let id = self.next_id();
        let mut timers = self.timers.lock().expect("mutex poisoned");
        timers.push(TimerEntry {
            wake_time,
            id,
            waker,
        });
        id
    }

    /// Process timers that should fire
    pub fn process_timers(&self) {
        let now = self.now();
        let mut timers = self.timers.lock().expect("mutex poisoned");

        while let Some(entry) = timers.peek() {
            if entry.wake_time <= now {
                let entry = timers.pop().expect("heap non-empty after peek");
                entry.waker.wake();
            } else {
                break;
            }
        }
    }

    /// Get next timer wake time (if any)
    pub fn next_timer_time(&self) -> Option<Timestamp> {
        self.timers.lock().expect("mutex poisoned").peek().map(|e| e.wake_time)
    }
}

impl std::fmt::Debug for SimulationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulationContext")
            .field("time", &self.now())
            .finish()
    }
}

/// Simulated time source for DST
///
/// This implements `TimeSource` by reading from a shared `SimulationContext`.
/// The time can be advanced externally for time-travel testing.
#[derive(Clone)]
pub struct SimulatedTimeSource {
    ctx: Arc<SimulationContext>,
    node_id: NodeId,
}

impl SimulatedTimeSource {
    /// Create a new simulated time source
    pub fn new(ctx: Arc<SimulationContext>, node_id: NodeId) -> Self {
        SimulatedTimeSource { ctx, node_id }
    }

    /// Create a time source with default node ID (for simple tests)
    pub fn new_default(ctx: Arc<SimulationContext>) -> Self {
        SimulatedTimeSource {
            ctx,
            node_id: NodeId(0),
        }
    }

    /// Get the underlying context (for time manipulation)
    pub fn context(&self) -> &Arc<SimulationContext> {
        &self.ctx
    }
}

impl TimeSource for SimulatedTimeSource {
    #[inline]
    fn now_millis(&self) -> u64 {
        self.ctx.local_time(self.node_id).as_millis()
    }
}

impl std::fmt::Debug for SimulatedTimeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatedTimeSource")
            .field("node_id", &self.node_id)
            .field("time_ms", &self.now_millis())
            .finish()
    }
}

/// Clock offset for simulating clock skew
#[derive(Debug, Clone, Copy)]
pub struct ClockOffset {
    /// Fixed offset in milliseconds (positive = ahead, negative = behind)
    pub fixed_offset_ms: i64,
    /// Drift rate in parts per million (+1000 = 0.1% faster)
    pub drift_ppm: i64,
    /// Timestamp when drift started
    pub drift_anchor: Timestamp,
}

impl Default for ClockOffset {
    fn default() -> Self {
        ClockOffset {
            fixed_offset_ms: 0,
            drift_ppm: 0,
            drift_anchor: Timestamp::ZERO,
        }
    }
}

impl ClockOffset {
    /// Apply offset to global time to get local time
    pub fn apply(&self, global_time: Timestamp) -> Timestamp {
        let base = global_time.0 as i64;
        let elapsed = base - self.drift_anchor.0 as i64;
        let drift = (elapsed * self.drift_ppm) / 1_000_000;
        let local = base + self.fixed_offset_ms + drift;
        Timestamp(local.max(0) as u64)
    }
}

/// Timer entry for the heap
struct TimerEntry {
    wake_time: Timestamp,
    id: u64,
    waker: Waker,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.wake_time == other.wake_time && self.id == other.id
    }
}

impl Eq for TimerEntry {}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap
        other
            .wake_time
            .cmp(&self.wake_time)
            .then_with(|| other.id.cmp(&self.id))
    }
}

/// Simulated clock for a specific node
pub struct SimulatedClock {
    ctx: Arc<SimulationContext>,
    node_id: NodeId,
}

impl Clock for SimulatedClock {
    fn now(&self) -> Timestamp {
        self.ctx.local_time(self.node_id)
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let wake_time = self.ctx.now() + duration;
        let ctx = self.ctx.clone();

        Box::pin(SimulatedSleep {
            ctx,
            wake_time,
            timer_id: None,
        })
    }

    fn interval(&self, period: Duration) -> Box<dyn Ticker + Send> {
        Box::new(SimulatedTicker {
            ctx: self.ctx.clone(),
            period,
            next_tick: self.ctx.now() + period,
        })
    }
}

/// Future for simulated sleep
struct SimulatedSleep {
    ctx: Arc<SimulationContext>,
    wake_time: Timestamp,
    timer_id: Option<u64>,
}

impl Future for SimulatedSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let now = self.ctx.now();
        if now >= self.wake_time {
            Poll::Ready(())
        } else {
            // Register timer if not already
            if self.timer_id.is_none() {
                let id = self.ctx.add_timer(self.wake_time, cx.waker().clone());
                self.timer_id = Some(id);
            }
            Poll::Pending
        }
    }
}

unsafe impl Send for SimulatedSleep {}

/// Simulated ticker
struct SimulatedTicker {
    ctx: Arc<SimulationContext>,
    period: Duration,
    next_tick: Timestamp,
}

impl Ticker for SimulatedTicker {
    fn tick(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let wake_time = self.next_tick;
        self.next_tick = self.next_tick + self.period;
        let ctx = self.ctx.clone();

        Box::pin(SimulatedSleep {
            ctx,
            wake_time,
            timer_id: None,
        })
    }
}

/// Network state tracking all connections and in-flight packets
struct NetworkState {
    /// Listeners by address
    listeners: HashMap<String, ListenerState>,
    /// Pending connections waiting for accept
    pending_connections: HashMap<String, VecDeque<PendingConnection>>,
    /// In-flight packets
    packets: VecDeque<InFlightPacket>,
    /// Active partitions (node pairs that can't communicate)
    partitions: std::collections::HashSet<(NodeId, NodeId)>,
}

impl NetworkState {
    fn new() -> Self {
        NetworkState {
            listeners: HashMap::new(),
            pending_connections: HashMap::new(),
            packets: VecDeque::new(),
            partitions: std::collections::HashSet::new(),
        }
    }
}

struct ListenerState {
    node_id: NodeId,
    wakers: Vec<Waker>,
}

struct PendingConnection {
    from_node: NodeId,
    from_addr: String,
    stream_id: u64,
}

#[allow(dead_code)]
struct InFlightPacket {
    from: NodeId,
    to: NodeId,
    stream_id: u64,
    data: Vec<u8>,
    delivery_time: Timestamp,
}

/// Simulated network for a specific node
pub struct SimulatedNetwork {
    ctx: Arc<SimulationContext>,
    node_id: NodeId,
}

impl Network for SimulatedNetwork {
    type Listener = SimulatedListener;
    type Stream = SimulatedStream;

    fn bind<'a>(
        &'a self,
        addr: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<Self::Listener>> + Send + 'a>> {
        let ctx = self.ctx.clone();
        let node_id = self.node_id;
        let addr = addr.to_string();

        Box::pin(async move {
            // Scope the network lock so it's dropped before we move ctx
            {
                let mut network = ctx.network_state.lock().expect("mutex poisoned");

                if network.listeners.contains_key(&addr) {
                    return Err(IoError::new(ErrorKind::AddrInUse, "Address already in use"));
                }

                network.listeners.insert(
                    addr.clone(),
                    ListenerState {
                        node_id,
                        wakers: Vec::new(),
                    },
                );
                network
                    .pending_connections
                    .insert(addr.clone(), VecDeque::new());
            }

            Ok(SimulatedListener { ctx, addr, node_id })
        })
    }

    fn connect<'a>(
        &'a self,
        addr: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<Self::Stream>> + Send + 'a>> {
        let ctx = self.ctx.clone();
        let node_id = self.node_id;
        let addr = addr.to_string();

        Box::pin(async move {
            // BUGGIFY: connection timeout
            {
                let mut rng = ctx.rng.lock().expect("mutex poisoned");
                if check_buggify(&mut *rng, faults::network::CONNECT_TIMEOUT) {
                    return Err(IoError::new(ErrorKind::TimedOut, "Connection timed out"));
                }
            }

            let stream_id = ctx.next_id();

            // Extract what we need in a scoped block so network lock is dropped before we move ctx
            let remote_node = {
                let mut network = ctx.network_state.lock().expect("mutex poisoned");

                // Check if listener exists
                let listener = network.listeners.get(&addr).ok_or_else(|| {
                    IoError::new(ErrorKind::ConnectionRefused, "Connection refused")
                })?;

                let remote_node = listener.node_id;
                let wakers: Vec<Waker> = listener.wakers.clone();

                // Check for partition
                if network.partitions.contains(&(node_id, remote_node))
                    || network.partitions.contains(&(remote_node, node_id))
                {
                    return Err(IoError::new(
                        ErrorKind::ConnectionRefused,
                        "Network partition",
                    ));
                }

                // Queue pending connection for listener
                if let Some(pending) = network.pending_connections.get_mut(&addr) {
                    pending.push_back(PendingConnection {
                        from_node: node_id,
                        from_addr: format!("{}:{}", node_id.0, stream_id),
                        stream_id,
                    });
                }

                // Wake any waiting acceptors
                for waker in wakers {
                    waker.wake();
                }

                remote_node
            };

            Ok(SimulatedStream {
                ctx,
                node_id,
                remote_node,
                stream_id,
                read_buffer: Arc::new(Mutex::new(VecDeque::new())),
                closed: Arc::new(Mutex::new(false)),
            })
        })
    }
}

/// Simulated TCP listener
pub struct SimulatedListener {
    ctx: Arc<SimulationContext>,
    addr: String,
    node_id: NodeId,
}

impl std::fmt::Debug for SimulatedListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatedListener")
            .field("addr", &self.addr)
            .field("node_id", &self.node_id)
            .finish()
    }
}

impl NetworkListener for SimulatedListener {
    type Stream = SimulatedStream;

    fn accept(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = IoResult<(Self::Stream, String)>> + Send + '_>> {
        let ctx = self.ctx.clone();
        let addr = self.addr.clone();
        let node_id = self.node_id;

        Box::pin(async move {
            // Check for pending connection
            let mut network = ctx.network_state.lock().expect("mutex poisoned");
            let pending = network.pending_connections.get_mut(&addr);

            if let Some(pending) = pending {
                if let Some(conn) = pending.pop_front() {
                    let stream = SimulatedStream {
                        ctx: ctx.clone(),
                        node_id,
                        remote_node: conn.from_node,
                        stream_id: conn.stream_id,
                        read_buffer: Arc::new(Mutex::new(VecDeque::new())),
                        closed: Arc::new(Mutex::new(false)),
                    };
                    return Ok((stream, conn.from_addr));
                }
            }

            // No pending connection - would need to wait
            // For now, return WouldBlock
            Err(IoError::new(ErrorKind::WouldBlock, "No pending connection"))
        })
    }

    fn local_addr(&self) -> IoResult<String> {
        Ok(self.addr.clone())
    }
}

/// Simulated TCP stream
pub struct SimulatedStream {
    ctx: Arc<SimulationContext>,
    node_id: NodeId,
    remote_node: NodeId,
    stream_id: u64,
    read_buffer: Arc<Mutex<VecDeque<u8>>>,
    closed: Arc<Mutex<bool>>,
}

impl std::fmt::Debug for SimulatedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatedStream")
            .field("node_id", &self.node_id)
            .field("remote_node", &self.remote_node)
            .field("stream_id", &self.stream_id)
            .finish()
    }
}

impl NetworkStream for SimulatedStream {
    fn read<'a>(
        &'a mut self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<usize>> + Send + 'a>> {
        let read_buffer = self.read_buffer.clone();
        let closed = self.closed.clone();

        Box::pin(async move {
            let mut buffer = read_buffer.lock().expect("mutex poisoned");

            if buffer.is_empty() {
                if *closed.lock().expect("mutex poisoned") {
                    return Ok(0); // EOF
                }
                return Err(IoError::new(ErrorKind::WouldBlock, "No data available"));
            }

            let to_read = buf.len().min(buffer.len());
            for i in 0..to_read {
                buf[i] = buffer.pop_front().expect("buffer verified non-empty");
            }

            Ok(to_read)
        })
    }

    fn read_exact<'a>(
        &'a mut self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        let read_buffer = self.read_buffer.clone();
        let closed = self.closed.clone();

        Box::pin(async move {
            let mut buffer = read_buffer.lock().expect("mutex poisoned");

            if buffer.len() < buf.len() {
                if *closed.lock().expect("mutex poisoned") {
                    return Err(IoError::new(ErrorKind::UnexpectedEof, "Unexpected EOF"));
                }
                return Err(IoError::new(ErrorKind::WouldBlock, "Insufficient data"));
            }

            for byte in buf.iter_mut() {
                *byte = buffer.pop_front().expect("buffer length checked above");
            }

            Ok(())
        })
    }

    fn write_all<'a>(
        &'a mut self,
        buf: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        let ctx = self.ctx.clone();
        let node_id = self.node_id;
        let remote_node = self.remote_node;
        let stream_id = self.stream_id;
        let mut data = buf.to_vec();

        Box::pin(async move {
            // Check if stream is closed
            // Apply BUGGIFY faults
            {
                let mut rng = ctx.rng.lock().expect("mutex poisoned");

                // BUGGIFY: packet drop
                if check_buggify(&mut *rng, faults::network::PACKET_DROP) {
                    // Silently drop the packet
                    return Ok(());
                }

                // BUGGIFY: connection reset
                if check_buggify(&mut *rng, faults::network::CONNECTION_RESET) {
                    return Err(IoError::new(ErrorKind::ConnectionReset, "Connection reset"));
                }

                // BUGGIFY: packet corruption
                if check_buggify(&mut *rng, faults::network::PACKET_CORRUPT) && !data.is_empty() {
                    let idx = rng.gen_range(0, data.len() as u64) as usize;
                    let bit = rng.gen_range(0, 8);
                    data[idx] ^= 1 << bit;
                }

                // BUGGIFY: partial write
                if check_buggify(&mut *rng, faults::network::PARTIAL_WRITE) && data.len() > 1 {
                    let new_len = rng.gen_range(1, data.len() as u64) as usize;
                    data.truncate(new_len);
                }
            }

            // Calculate delivery time (with potential delay)
            let base_delay = Duration::from_millis(1);
            let delivery_time = {
                let mut rng = ctx.rng.lock().expect("mutex poisoned");
                let delay = if check_buggify(&mut *rng, faults::network::DELAY) {
                    // Add significant delay
                    Duration::from_millis(rng.gen_range(10, 1000))
                } else {
                    base_delay
                };
                ctx.now() + delay
            };

            // Queue packet
            let mut network = ctx.network_state.lock().expect("mutex poisoned");

            // BUGGIFY: reorder - insert at random position
            let packet = InFlightPacket {
                from: node_id,
                to: remote_node,
                stream_id,
                data,
                delivery_time,
            };

            {
                let mut rng = ctx.rng.lock().expect("mutex poisoned");
                if check_buggify(&mut *rng, faults::network::REORDER) && !network.packets.is_empty()
                {
                    let pos = rng.gen_range(0, network.packets.len() as u64) as usize;
                    network.packets.insert(pos, packet);
                } else {
                    // BUGGIFY: duplicate
                    if check_buggify(&mut *rng, faults::network::DUPLICATE) {
                        let dup = InFlightPacket {
                            from: node_id,
                            to: remote_node,
                            stream_id,
                            data: network
                                .packets
                                .back()
                                .map(|p| p.data.clone())
                                .unwrap_or_default(),
                            delivery_time,
                        };
                        network.packets.push_back(dup);
                    }
                    network.packets.push_back(packet);
                }
            }

            Ok(())
        })
    }

    fn flush(&mut self) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn peer_addr(&self) -> IoResult<String> {
        Ok(format!("node{}:{}", self.remote_node.0, self.stream_id))
    }
}

/// Simulated RNG - deterministic based on seed
pub struct SimulatedRng {
    inner: rand_chacha::ChaCha8Rng,
}

impl SimulatedRng {
    pub fn new(seed: u64) -> Self {
        use rand::SeedableRng;
        SimulatedRng {
            inner: rand_chacha::ChaCha8Rng::seed_from_u64(seed),
        }
    }
}

impl Rng for SimulatedRng {
    fn next_u64(&mut self) -> u64 {
        use rand::RngCore;
        self.inner.next_u64()
    }

    fn gen_bool(&mut self, probability: f64) -> bool {
        use rand::Rng;
        self.inner.gen_bool(probability.clamp(0.0, 1.0))
    }

    fn gen_range(&mut self, min: u64, max: u64) -> u64 {
        use rand::Rng;
        if min >= max {
            return min;
        }
        self.inner.gen_range(min..max)
    }

    fn shuffle<T>(&mut self, slice: &mut [T]) {
        use rand::seq::SliceRandom;
        slice.shuffle(&mut self.inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_offset() {
        let offset = ClockOffset {
            fixed_offset_ms: 100,
            drift_ppm: 0,
            drift_anchor: Timestamp::ZERO,
        };

        let global = Timestamp::from_millis(1000);
        let local = offset.apply(global);
        assert_eq!(local.as_millis(), 1100);
    }

    #[test]
    fn test_clock_drift() {
        let offset = ClockOffset {
            fixed_offset_ms: 0,
            drift_ppm: 1000, // 0.1% faster
            drift_anchor: Timestamp::ZERO,
        };

        let global = Timestamp::from_millis(1_000_000);
        let local = offset.apply(global);
        // Should be 1001000 (1M + 1000 ppm drift)
        assert_eq!(local.as_millis(), 1_001_000);
    }

    #[test]
    fn test_simulated_rng_deterministic() {
        let mut rng1 = SimulatedRng::new(12345);
        let mut rng2 = SimulatedRng::new(12345);

        for _ in 0..100 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
        }
    }

    #[test]
    fn test_simulation_context() {
        let ctx = SimulationContext::new(42, FaultConfig::disabled());

        assert_eq!(ctx.now().as_millis(), 0);

        ctx.advance_by(Duration::from_millis(100));
        assert_eq!(ctx.now().as_millis(), 100);

        ctx.advance_to(Timestamp::from_millis(500));
        assert_eq!(ctx.now().as_millis(), 500);

        // Can't go backwards
        ctx.advance_to(Timestamp::from_millis(200));
        assert_eq!(ctx.now().as_millis(), 500);
    }

    #[test]
    fn test_per_node_clock_skew() {
        let ctx = Arc::new(SimulationContext::new(42, FaultConfig::disabled()));

        let node1 = NodeId(1);
        let node2 = NodeId(2);

        // Node 1 is 50ms ahead
        ctx.set_clock_offset(
            node1,
            ClockOffset {
                fixed_offset_ms: 50,
                drift_ppm: 0,
                drift_anchor: Timestamp::ZERO,
            },
        );

        // Node 2 is 30ms behind
        ctx.set_clock_offset(
            node2,
            ClockOffset {
                fixed_offset_ms: -30,
                drift_ppm: 0,
                drift_anchor: Timestamp::ZERO,
            },
        );

        ctx.advance_to(Timestamp::from_millis(1000));

        assert_eq!(ctx.local_time(node1).as_millis(), 1050);
        assert_eq!(ctx.local_time(node2).as_millis(), 970);
    }
}
