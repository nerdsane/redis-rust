use crate::replication::{ReplicaId, ReplicationConfig};
use crate::replication::gossip::{GossipMessage, GossipState};
use crate::replication::state::ReplicationDelta;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{info, warn, error};

pub type DeltaCallback = Arc<dyn Fn(Vec<ReplicationDelta>) + Send + Sync>;

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

    pub async fn start_server(
        config: ReplicationConfig,
        delta_callback: DeltaCallback,
    ) -> std::io::Result<()> {
        let port = 3001 + config.replica_id as u16;
        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        info!("Gossip server listening on port {}", port);

        loop {
            let (stream, addr) = listener.accept().await?;
            info!("Gossip connection from {}", addr);
            let callback = delta_callback.clone();
            
            tokio::spawn(async move {
                if let Err(e) = Self::handle_peer_connection(stream, callback).await {
                    warn!("Gossip peer error: {}", e);
                }
            });
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
                        GossipMessage::Heartbeat { source_replica, epoch } => {
                            info!("Heartbeat from replica {} epoch {}", source_replica.0, epoch);
                        }
                        GossipMessage::SyncRequest { .. } => {
                        }
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
        let replica_id = ReplicaId::new(config.replica_id);
        let peers = config.peers.clone();

        info!(
            "Starting gossip loop with {} peers, interval {:?}",
            peers.len(),
            gossip_interval
        );

        loop {
            ticker.tick().await;

            let deltas = collect_deltas();
            
            {
                let mut state = gossip_state.write();
                state.advance_epoch();
            }

            if deltas.is_empty() {
                continue;
            }

            let epoch = gossip_state.read().epoch;
            let msg = GossipMessage::new_delta_batch(replica_id, deltas, epoch);
            let data = match msg.serialize() {
                Ok(d) => d,
                Err(e) => {
                    error!("Failed to serialize gossip message: {}", e);
                    continue;
                }
            };

            let framed_data = Self::frame_message(&data);
            
            for peer in &peers {
                match TcpStream::connect(peer).await {
                    Ok(mut stream) => {
                        if let Err(e) = stream.write_all(&framed_data).await {
                            warn!("Failed to send to peer {}: {}", peer, e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to connect to peer {}: {}", peer, e);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerState {
    pub replica_id: ReplicaId,
    pub address: String,
    pub last_seen_epoch: u64,
    pub connected: bool,
}

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
