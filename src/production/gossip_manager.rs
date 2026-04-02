use super::gossip_actor::GossipActorHandle;
use crate::replication::gossip::{GossipMessage, GossipState, RoutedMessage};
use crate::replication::state::ReplicationDelta;
use crate::replication::{ReplicaId, ReplicationConfig};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Maximum number of active inbound peer connections per unique IP.
/// When a new connection arrives from an IP that already has an active handler,
/// the old handler is aborted before spawning a new one.
const MAX_CONNECTIONS_PER_PEER: usize = 1;

pub type DeltaCallback = Arc<dyn Fn(Vec<ReplicationDelta>) + Send + Sync>;

#[allow(dead_code)]
pub struct GossipManager {
    config: ReplicationConfig,
    gossip_state: Arc<RwLock<GossipState>>,
    delta_tx: mpsc::UnboundedSender<Vec<ReplicationDelta>>,
    delta_rx: Option<mpsc::UnboundedReceiver<Vec<ReplicationDelta>>>,
    outbound_tx: mpsc::UnboundedSender<GossipMessage>,
    outbound_rx: Option<mpsc::UnboundedReceiver<GossipMessage>>,
}

impl GossipManager {
    pub fn new(config: ReplicationConfig, gossip_state: Arc<RwLock<GossipState>>) -> Self {
        let (delta_tx, delta_rx) = mpsc::unbounded_channel();
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

        GossipManager {
            config,
            gossip_state,
            delta_tx,
            delta_rx: Some(delta_rx),
            outbound_tx,
            outbound_rx: Some(outbound_rx),
        }
    }

    pub fn get_delta_sender(&self) -> mpsc::UnboundedSender<Vec<ReplicationDelta>> {
        self.delta_tx.clone()
    }

    pub fn queue_outbound(&self, msg: GossipMessage) {
        let _ = self.outbound_tx.send(msg);
    }

    /// Start the gossip TCP server with per-peer connection tracking.
    ///
    /// ## Invariants
    /// - At most `MAX_CONNECTIONS_PER_PEER` (1) active handler task per peer IP.
    /// - When a peer reconnects, the old handler is aborted before spawning a new one.
    /// - This prevents unbounded task accumulation from peers that reconnect every gossip round.
    pub async fn start_server(
        config: ReplicationConfig,
        delta_callback: DeltaCallback,
    ) -> std::io::Result<()> {
        let port = 3001u16
            .checked_add(config.replica_id as u16)
            .expect("Precondition: replica_id must not overflow gossip port range");
        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        info!("Gossip server listening on port {}", port);

        // Track active connections per peer IP. When a peer reconnects,
        // we abort the old handler task to prevent unbounded task growth.
        let mut active_peers: HashMap<IpAddr, JoinHandle<()>> = HashMap::new();

        loop {
            let (stream, addr) = listener.accept().await?;
            let peer_ip = addr.ip();

            // If this peer already has an active connection, abort it first.
            // This is the fix for the unbounded connection accept loop:
            // each gossip round from a peer opens a new TCP connection, and
            // without this cleanup, tasks accumulate (~20/sec with 2 peers).
            if let Some(old_handle) = active_peers.remove(&peer_ip) {
                if !old_handle.is_finished() {
                    debug!(
                        "Aborting stale gossip handler for peer {} (replaced by new connection)",
                        peer_ip
                    );
                    old_handle.abort();
                }
            }

            // Garbage-collect finished tasks to prevent the HashMap from growing
            // unboundedly with IPs of peers that have since disconnected.
            active_peers.retain(|_ip, handle| !handle.is_finished());

            debug_assert!(
                !active_peers.contains_key(&peer_ip),
                "Postcondition: old peer handle must be removed before inserting new one"
            );

            info!("Gossip connection from {} (active peers: {})", addr, active_peers.len().checked_add(1).unwrap_or(usize::MAX));
            let callback = delta_callback.clone();

            let handle = tokio::spawn(async move {
                if let Err(e) = Self::handle_peer_connection(stream, callback).await {
                    warn!("Gossip peer {} error: {}", addr, e);
                }
            });

            active_peers.insert(peer_ip, handle);

            debug_assert!(
                active_peers.len() <= MAX_CONNECTIONS_PER_PEER * 100,
                "Invariant: active_peers should not grow unboundedly (current: {})",
                active_peers.len()
            );
        }
    }

    async fn handle_peer_connection(
        mut stream: TcpStream,
        delta_callback: DeltaCallback,
    ) -> std::io::Result<()> {
        loop {
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let msg_len = u32::from_be_bytes(len_buf) as usize;

            if msg_len > 1024 * 1024 {
                warn!("Message too large: {} bytes", msg_len);
                break;
            }

            let mut msg_buf = vec![0u8; msg_len];
            if stream.read_exact(&mut msg_buf).await.is_err() {
                break;
            }

            match GossipMessage::deserialize(&msg_buf) {
                Ok(msg) => {
                    match msg {
                        GossipMessage::DeltaBatch { deltas, .. } => {
                            delta_callback(deltas);
                        }
                        GossipMessage::TargetedDelta {
                            deltas,
                            target_replica,
                            source_replica,
                            ..
                        } => {
                            // Targeted deltas are sent directly to the intended recipient
                            // In selective gossip mode, we only receive deltas for keys we're responsible for
                            info!(
                                "Received targeted delta from replica {} (target: {}): {} deltas",
                                source_replica.0,
                                target_replica.0,
                                deltas.len()
                            );
                            delta_callback(deltas);
                        }
                        GossipMessage::Heartbeat {
                            source_replica,
                            epoch,
                        } => {
                            info!(
                                "Heartbeat from replica {} epoch {}",
                                source_replica.0, epoch
                            );
                        }
                        GossipMessage::SyncRequest { .. } => {}
                        GossipMessage::SyncResponse { deltas, .. } => {
                            delta_callback(deltas);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize gossip message: {}", e);
                }
            }
        }

        Ok(())
    }

    fn frame_message(data: &[u8]) -> Vec<u8> {
        let len = data.len() as u32;
        let mut framed = Vec::with_capacity(4 + data.len());
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(data);
        framed
    }

    pub async fn start_gossip_loop(
        config: ReplicationConfig,
        gossip_state: Arc<RwLock<GossipState>>,
        collect_deltas: impl Fn() -> Vec<ReplicationDelta> + Send + Sync + 'static,
    ) {
        let gossip_interval = config.gossip_interval();
        let mut ticker = interval(gossip_interval);
        let peers = config.peers.clone();
        let selective_mode = config.uses_selective_gossip();

        // Build peer address map for selective routing
        let peer_map: HashMap<ReplicaId, String> = peers
            .iter()
            .enumerate()
            .map(|(i, addr)| {
                let peer_id = if (i as u64) >= config.replica_id {
                    (i as u64) + 2 // Skip our own ID
                } else {
                    (i as u64) + 1
                };
                (ReplicaId::new(peer_id), addr.clone())
            })
            .collect();

        info!(
            "Starting gossip loop with {} peers, interval {:?}, selective: {}",
            peers.len(),
            gossip_interval,
            selective_mode
        );

        // Persistent connection pool: reuse TCP connections across gossip rounds.
        // This is the primary fix for the connection storm — previously each
        // send_to_peer() opened a new TCP connection and dropped it after one message,
        // causing ~20 new connections/sec with 2 peers at 100ms intervals.
        let mut peer_connections: HashMap<String, TcpStream> = HashMap::new();

        loop {
            ticker.tick().await;

            let deltas = collect_deltas();

            // Queue deltas and get routed messages
            let routed_messages: Vec<RoutedMessage>;
            {
                let mut state = gossip_state.write();
                state.advance_epoch();
                state.queue_deltas(deltas);
                routed_messages = state.drain_outbound();
            }

            if routed_messages.is_empty() {
                continue;
            }

            // Send each routed message using persistent connections
            for routed in routed_messages {
                let data = match routed.message.serialize() {
                    Ok(d) => d,
                    Err(e) => {
                        error!("Failed to serialize gossip message: {}", e);
                        continue;
                    }
                };
                let framed_data = Self::frame_message(&data);

                match routed.target {
                    Some(target_replica) => {
                        // Targeted message: send to specific replica
                        if let Some(addr) = peer_map.get(&target_replica) {
                            Self::send_to_peer_persistent(
                                &mut peer_connections,
                                addr,
                                &framed_data,
                            )
                            .await;
                        } else {
                            debug!("No address for target replica {}", target_replica.0);
                        }
                    }
                    None => {
                        // Broadcast message: send to all peers
                        for peer_addr in &peers {
                            Self::send_to_peer_persistent(
                                &mut peer_connections,
                                peer_addr,
                                &framed_data,
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    /// Send framed data to a peer using a persistent connection pool.
    ///
    /// Reuses existing TCP connections across gossip rounds. If the connection
    /// is broken (write fails), it is removed and a fresh connection is established.
    /// This eliminates the ~20 new TCP connections/sec that caused OOM on node-0.
    ///
    /// ## Invariants
    /// - `peer_connections` maps peer address strings to live TCP streams.
    /// - A broken connection is always removed before attempting reconnection.
    /// - At most one connection per peer address exists in the map at any time.
    async fn send_to_peer_persistent(
        peer_connections: &mut HashMap<String, TcpStream>,
        addr: &str,
        framed_data: &[u8],
    ) {
        debug_assert!(!addr.is_empty(), "Precondition: peer address must not be empty");
        debug_assert!(!framed_data.is_empty(), "Precondition: framed data must not be empty");

        // Try to reuse existing connection
        if let Some(stream) = peer_connections.get_mut(addr) {
            match stream.write_all(framed_data).await {
                Ok(()) => return, // Success — connection reused
                Err(e) => {
                    debug!("Persistent connection to {} broken, reconnecting: {}", addr, e);
                    peer_connections.remove(addr);
                    // Fall through to reconnect below
                }
            }
        }

        // No existing connection or it was broken — establish a new one
        match TcpStream::connect(addr).await {
            Ok(mut stream) => {
                match stream.write_all(framed_data).await {
                    Ok(()) => {
                        peer_connections.insert(addr.to_string(), stream);
                    }
                    Err(e) => {
                        warn!("Failed to send to peer {} on fresh connection: {}", addr, e);
                        // Don't insert a broken connection
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to peer {}: {}", addr, e);
            }
        }
    }

    /// Actor-based gossip loop - uses GossipActorHandle instead of Arc<RwLock<>>
    ///
    /// This is the preferred way to run the gossip loop as it eliminates
    /// lock contention and follows the actor model.
    pub async fn start_gossip_loop_with_actor(
        config: ReplicationConfig,
        gossip_handle: GossipActorHandle,
        collect_deltas: impl Fn() -> Vec<ReplicationDelta> + Send + Sync + 'static,
    ) {
        let gossip_interval = config.gossip_interval();
        let mut ticker = interval(gossip_interval);
        let peers = config.peers.clone();
        let selective_mode = config.uses_selective_gossip();

        // Build peer address map for selective routing
        let peer_map: HashMap<ReplicaId, String> = peers
            .iter()
            .enumerate()
            .map(|(i, addr)| {
                let peer_id = if (i as u64) >= config.replica_id {
                    (i as u64) + 2 // Skip our own ID
                } else {
                    (i as u64) + 1
                };
                (ReplicaId::new(peer_id), addr.clone())
            })
            .collect();

        info!(
            "Starting actor-based gossip loop with {} peers, interval {:?}, selective: {}",
            peers.len(),
            gossip_interval,
            selective_mode
        );

        // Persistent connection pool: reuse TCP connections across gossip rounds.
        let mut peer_connections: HashMap<String, TcpStream> = HashMap::new();

        loop {
            ticker.tick().await;

            let deltas = collect_deltas();

            // Use actor handle - no locks!
            gossip_handle.advance_epoch();
            gossip_handle.queue_deltas(deltas);
            let routed_messages = gossip_handle.drain_outbound().await;

            if routed_messages.is_empty() {
                continue;
            }

            // Send each routed message using persistent connections
            for routed in routed_messages {
                let data = match routed.message.serialize() {
                    Ok(d) => d,
                    Err(e) => {
                        error!("Failed to serialize gossip message: {}", e);
                        continue;
                    }
                };
                let framed_data = Self::frame_message(&data);

                match routed.target {
                    Some(target_replica) => {
                        // Targeted message: send to specific replica
                        if let Some(addr) = peer_map.get(&target_replica) {
                            Self::send_to_peer_persistent(
                                &mut peer_connections,
                                addr,
                                &framed_data,
                            )
                            .await;
                        } else {
                            debug!("No address for target replica {}", target_replica.0);
                        }
                    }
                    None => {
                        // Broadcast message: send to all peers
                        for peer_addr in &peers {
                            Self::send_to_peer_persistent(
                                &mut peer_connections,
                                peer_addr,
                                &framed_data,
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PeerState {
    pub replica_id: ReplicaId,
    pub address: String,
    pub last_seen_epoch: u64,
    pub connected: bool,
}

#[allow(dead_code)]
impl PeerState {
    pub fn new(replica_id: ReplicaId, address: String) -> Self {
        PeerState {
            replica_id,
            address,
            last_seen_epoch: 0,
            connected: false,
        }
    }
}
