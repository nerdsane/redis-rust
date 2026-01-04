use redis_sim::redis::{Command, SDS, RespValue};
use redis_sim::replication::{ReplicaId, ReplicationConfig, ConsistencyLevel};
use redis_sim::replication::state::{ReplicationDelta, ReplicatedValue};
use redis_sim::replication::lattice::LamportClock;
use redis_sim::production::ReplicatedShardedState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Write};

#[derive(Debug, Deserialize)]
struct Message {
    src: String,
    dest: String,
    body: Body,
}

#[derive(Debug, Deserialize)]
struct Body {
    #[serde(rename = "type")]
    msg_type: String,
    msg_id: Option<u64>,
    key: Option<Value>,
    value: Option<Value>,
    from: Option<Value>,
    to: Option<Value>,
    node_id: Option<String>,
    node_ids: Option<Vec<String>>,
    #[serde(default)]
    deltas: Option<Vec<DeltaJson>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct DeltaJson {
    key: String,
    value: Option<String>,
    timestamp: u64,
    replica_id: u64,
    expiry_ms: Option<u64>,
    is_tombstone: bool,
}

#[derive(Debug, Serialize)]
struct Response {
    src: String,
    dest: String,
    body: ResponseBody,
}

#[derive(Debug, Serialize)]
struct ResponseBody {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    in_reply_to: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deltas: Option<Vec<DeltaJson>>,
}

struct NodeState {
    node_id: String,
    #[allow(dead_code)]
    replica_id: u64,
    peers: Vec<String>,
    state: ReplicatedShardedState,
    #[allow(dead_code)]
    pending_deltas: Vec<ReplicationDelta>,
}

impl NodeState {
    fn new(node_id: String, node_ids: Vec<String>) -> Self {
        let replica_id = node_id_to_replica_id(&node_id);
        let peers: Vec<String> = node_ids.iter()
            .filter(|id| *id != &node_id)
            .cloned()
            .collect();
        
        let config = ReplicationConfig {
            enabled: true,
            replica_id,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 100,
            peers: vec![],
            replication_factor: 3,
            partitioned_mode: false,
            selective_gossip: false,
            virtual_nodes_per_physical: 150,
        };
        
        NodeState {
            node_id,
            replica_id,
            peers,
            state: ReplicatedShardedState::new(config),
            pending_deltas: Vec::new(),
        }
    }

    fn execute(&mut self, cmd: Command) -> RespValue {
        self.state.execute(cmd)
    }

    fn drain_pending_deltas(&mut self) -> Vec<ReplicationDelta> {
        self.state.collect_pending_deltas()
    }

    fn apply_remote_deltas(&mut self, deltas: Vec<ReplicationDelta>) {
        self.state.apply_remote_deltas(deltas);
    }
}

fn node_id_to_replica_id(node_id: &str) -> u64 {
    node_id.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn delta_to_json(delta: &ReplicationDelta) -> DeltaJson {
    DeltaJson {
        key: delta.key.clone(),
        value: delta.value.get().map(|sds| String::from_utf8_lossy(sds.as_bytes()).to_string()),
        timestamp: delta.value.timestamp.time,
        replica_id: delta.source_replica.0,
        expiry_ms: delta.value.expiry_ms,
        is_tombstone: delta.value.is_tombstone(),
    }
}

fn json_to_delta(json: &DeltaJson) -> ReplicationDelta {
    use redis_sim::replication::lattice::LwwRegister;
    use redis_sim::replication::state::CrdtValue;

    let replica_id = ReplicaId::new(json.replica_id);
    let timestamp = LamportClock { time: json.timestamp, replica_id };

    let mut replicated_value = if json.is_tombstone {
        // Create a tombstoned LWW value
        let mut lww = LwwRegister::new(replica_id);
        lww.tombstone = true;
        lww.timestamp = timestamp;
        let mut rv = ReplicatedValue::new(replica_id);
        rv.crdt = CrdtValue::Lww(lww);
        rv.timestamp = timestamp;
        rv
    } else if let Some(ref v) = json.value {
        ReplicatedValue::with_value(SDS::from_str(v), timestamp)
    } else {
        // No value and not tombstone - create tombstone
        let mut lww = LwwRegister::new(replica_id);
        lww.tombstone = true;
        lww.timestamp = timestamp;
        let mut rv = ReplicatedValue::new(replica_id);
        rv.crdt = CrdtValue::Lww(lww);
        rv.timestamp = timestamp;
        rv
    };

    replicated_value.expiry_ms = json.expiry_ms;

    ReplicationDelta::new(json.key.clone(), replicated_value, replica_id)
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    
    let mut node_state: Option<NodeState> = None;
    let mut msg_counter: u64 = 0;
    
    for line in stdin.lock().lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        
        let msg: Message = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Failed to parse message: {} - {}", line, e);
                continue;
            }
        };
        
        msg_counter += 1;
        
        let response = match msg.body.msg_type.as_str() {
            "init" => {
                let node_id = msg.body.node_id.clone().unwrap_or_default();
                let node_ids = msg.body.node_ids.clone().unwrap_or_default();
                node_state = Some(NodeState::new(node_id, node_ids));
                
                Response {
                    src: msg.dest.clone(),
                    dest: msg.src.clone(),
                    body: ResponseBody {
                        msg_type: "init_ok".to_string(),
                        msg_id: Some(msg_counter),
                        in_reply_to: msg.body.msg_id,
                        value: None,
                        code: None,
                        text: None,
                        deltas: None,
                    },
                }
            }
            "read" => {
                let state = node_state.as_mut().expect("Node not initialized");
                let key = value_to_string(&msg.body.key);
                let cmd = Command::Get(key);
                let result = state.execute(cmd);
                
                match result {
                    RespValue::BulkString(Some(data)) => {
                        let value_str = String::from_utf8_lossy(&data);
                        let value: Value = serde_json::from_str(&value_str)
                            .unwrap_or(Value::String(value_str.to_string()));
                        Response {
                            src: msg.dest.clone(),
                            dest: msg.src.clone(),
                            body: ResponseBody {
                                msg_type: "read_ok".to_string(),
                                msg_id: Some(msg_counter),
                                in_reply_to: msg.body.msg_id,
                                value: Some(value),
                                code: None,
                                text: None,
                                deltas: None,
                            },
                        }
                    }
                    RespValue::BulkString(None) => {
                        Response {
                            src: msg.dest.clone(),
                            dest: msg.src.clone(),
                            body: ResponseBody {
                                msg_type: "error".to_string(),
                                msg_id: Some(msg_counter),
                                in_reply_to: msg.body.msg_id,
                                value: None,
                                code: Some(20),
                                text: Some("key does not exist".to_string()),
                                deltas: None,
                            },
                        }
                    }
                    _ => {
                        Response {
                            src: msg.dest.clone(),
                            dest: msg.src.clone(),
                            body: ResponseBody {
                                msg_type: "error".to_string(),
                                msg_id: Some(msg_counter),
                                in_reply_to: msg.body.msg_id,
                                value: None,
                                code: Some(13),
                                text: Some("internal error".to_string()),
                                deltas: None,
                            },
                        }
                    }
                }
            }
            "write" => {
                let state = node_state.as_mut().expect("Node not initialized");
                let key = value_to_string(&msg.body.key);
                let value = value_to_string(&msg.body.value);
                let sds = SDS::from_str(&value);
                let cmd = Command::Set(key, sds);
                let _ = state.execute(cmd);
                
                let pending = state.drain_pending_deltas();
                let peers = state.peers.clone();
                let my_id = state.node_id.clone();
                
                for peer in &peers {
                    let gossip_msg = Response {
                        src: my_id.clone(),
                        dest: peer.clone(),
                        body: ResponseBody {
                            msg_type: "replicate".to_string(),
                            msg_id: Some(msg_counter),
                            in_reply_to: None,
                            value: None,
                            code: None,
                            text: None,
                            deltas: Some(pending.iter().map(delta_to_json).collect()),
                        },
                    };
                    let gossip_str = serde_json::to_string(&gossip_msg)?;
                    writeln!(stdout, "{}", gossip_str)?;
                }
                
                Response {
                    src: msg.dest.clone(),
                    dest: msg.src.clone(),
                    body: ResponseBody {
                        msg_type: "write_ok".to_string(),
                        msg_id: Some(msg_counter),
                        in_reply_to: msg.body.msg_id,
                        value: None,
                        code: None,
                        text: None,
                        deltas: None,
                    },
                }
            }
            "replicate" => {
                let state = node_state.as_mut().expect("Node not initialized");
                
                if let Some(ref delta_jsons) = msg.body.deltas {
                    let deltas: Vec<ReplicationDelta> = delta_jsons.iter()
                        .map(json_to_delta)
                        .collect();
                    state.apply_remote_deltas(deltas);
                }
                
                continue;
            }
            "cas" => {
                let state = node_state.as_mut().expect("Node not initialized");
                let key = value_to_string(&msg.body.key);
                let from_value = value_to_string(&msg.body.from);
                let to_value = value_to_string(&msg.body.to);
                
                let get_cmd = Command::Get(key.clone());
                let current = state.execute(get_cmd);
                
                match current {
                    RespValue::BulkString(Some(data)) => {
                        let current_str = String::from_utf8_lossy(&data);
                        if current_str == from_value {
                            let sds = SDS::from_str(&to_value);
                            let set_cmd = Command::Set(key, sds);
                            let _ = state.execute(set_cmd);
                            
                            let pending = state.drain_pending_deltas();
                            let peers = state.peers.clone();
                            let my_id = state.node_id.clone();
                            
                            for peer in &peers {
                                let gossip_msg = Response {
                                    src: my_id.clone(),
                                    dest: peer.clone(),
                                    body: ResponseBody {
                                        msg_type: "replicate".to_string(),
                                        msg_id: Some(msg_counter),
                                        in_reply_to: None,
                                        value: None,
                                        code: None,
                                        text: None,
                                        deltas: Some(pending.iter().map(delta_to_json).collect()),
                                    },
                                };
                                let gossip_str = serde_json::to_string(&gossip_msg)?;
                                writeln!(stdout, "{}", gossip_str)?;
                            }
                            
                            Response {
                                src: msg.dest.clone(),
                                dest: msg.src.clone(),
                                body: ResponseBody {
                                    msg_type: "cas_ok".to_string(),
                                    msg_id: Some(msg_counter),
                                    in_reply_to: msg.body.msg_id,
                                    value: None,
                                    code: None,
                                    text: None,
                                    deltas: None,
                                },
                            }
                        } else {
                            Response {
                                src: msg.dest.clone(),
                                dest: msg.src.clone(),
                                body: ResponseBody {
                                    msg_type: "error".to_string(),
                                    msg_id: Some(msg_counter),
                                    in_reply_to: msg.body.msg_id,
                                    value: None,
                                    code: Some(22),
                                    text: Some(format!(
                                        "expected {}, but had {}",
                                        from_value, current_str
                                    )),
                                    deltas: None,
                                },
                            }
                        }
                    }
                    RespValue::BulkString(None) => {
                        Response {
                            src: msg.dest.clone(),
                            dest: msg.src.clone(),
                            body: ResponseBody {
                                msg_type: "error".to_string(),
                                msg_id: Some(msg_counter),
                                in_reply_to: msg.body.msg_id,
                                value: None,
                                code: Some(20),
                                text: Some("key does not exist".to_string()),
                                deltas: None,
                            },
                        }
                    }
                    _ => {
                        Response {
                            src: msg.dest.clone(),
                            dest: msg.src.clone(),
                            body: ResponseBody {
                                msg_type: "error".to_string(),
                                msg_id: Some(msg_counter),
                                in_reply_to: msg.body.msg_id,
                                value: None,
                                code: Some(13),
                                text: Some("internal error".to_string()),
                                deltas: None,
                            },
                        }
                    }
                }
            }
            _ => {
                Response {
                    src: msg.dest.clone(),
                    dest: msg.src.clone(),
                    body: ResponseBody {
                        msg_type: "error".to_string(),
                        msg_id: Some(msg_counter),
                        in_reply_to: msg.body.msg_id,
                        value: None,
                        code: Some(10),
                        text: Some(format!("unsupported message type: {}", msg.body.msg_type)),
                        deltas: None,
                    },
                }
            }
        };
        
        let response_str = serde_json::to_string(&response)?;
        writeln!(stdout, "{}", response_str)?;
        stdout.flush()?;
    }
    
    Ok(())
}

fn value_to_string(value: &Option<Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None => String::new(),
    }
}
