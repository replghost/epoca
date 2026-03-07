use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ChainId {
    PolkadotAssetHub,
    PaseoAssetHub,
    Previewnet,
    Ethereum,
    Bitcoin,
}

/// Which backend to use for a given chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionBackend {
    /// Embedded smoldot light client (trustless, peer-to-peer).
    Smoldot,
    /// Direct WebSocket RPC to a public endpoint (centralized).
    Rpc,
    /// Kyoto BIP-157/158 compact block filter light client (Bitcoin P2P).
    Kyoto,
    /// Helios Ethereum light client (consensus + execution verification).
    Helios,
}

impl ChainId {
    pub fn endpoint(self) -> &'static str {
        match self {
            ChainId::PolkadotAssetHub => "wss://polkadot-asset-hub-rpc.polkadot.io",
            ChainId::PaseoAssetHub => "wss://asset-hub-paseo.dotters.network",
            ChainId::Previewnet => "wss://previewnet.dotsamalabs.com/asset-hub",
            ChainId::Ethereum => "", // Helios uses consensus/execution RPC, not a single endpoint
            ChainId::Bitcoin => "", // Kyoto connects directly to Bitcoin P2P network
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ChainId::PolkadotAssetHub => "Polkadot Asset Hub",
            ChainId::PaseoAssetHub => "Paseo Asset Hub",
            ChainId::Previewnet => "Previewnet",
            ChainId::Ethereum => "Ethereum",
            ChainId::Bitcoin => "Bitcoin",
        }
    }

    /// All chain IDs including ETH/BTC. UI should gate display on settings flags.
    pub fn all() -> &'static [ChainId] {
        &[
            ChainId::PolkadotAssetHub,
            ChainId::PaseoAssetHub,
            ChainId::Previewnet,
            ChainId::Ethereum,
            ChainId::Bitcoin,
        ]
    }

    /// Substrate/Polkadot chains only (for existing chain settings UI).
    pub fn substrate_chains() -> &'static [ChainId] {
        &[
            ChainId::PolkadotAssetHub,
            ChainId::PaseoAssetHub,
            ChainId::Previewnet,
        ]
    }

    /// Determine the connection backend for this chain.
    pub fn backend(self) -> ConnectionBackend {
        match self {
            ChainId::PolkadotAssetHub | ChainId::PaseoAssetHub => ConnectionBackend::Smoldot,
            ChainId::Previewnet => ConnectionBackend::Rpc,
            ChainId::Ethereum => ConnectionBackend::Helios,
            ChainId::Bitcoin => ConnectionBackend::Kyoto,
        }
    }

    /// Return (relay_chain_spec, parachain_spec) for smoldot-backed chains.
    pub fn chain_specs(self) -> Option<(&'static str, &'static str)> {
        match self {
            ChainId::PolkadotAssetHub => Some((
                include_str!("../chain-specs/polkadot.json"),
                include_str!("../chain-specs/polkadot_asset_hub.json"),
            )),
            ChainId::PaseoAssetHub => Some((
                include_str!("../chain-specs/paseo.json"),
                include_str!("../chain-specs/paseo_asset_hub.json"),
            )),
            ChainId::Previewnet | ChainId::Ethereum | ChainId::Bitcoin => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChainState {
    Disconnected,
    Connecting,
    Syncing { best_block: u64, peers: u32 },
    Live { best_block: u64, peers: u32 },
    Error(String),
}

/// Chain-specific extra data surfaced to the UI.
#[derive(Debug, Clone, Default)]
pub enum ChainExtra {
    #[default]
    None,
    Eth {
        finalized_block: u64,
        gas_price_gwei: u64,
    },
    Btc {
        tip_height: u64,
        fee_rate_sat_vb: u32,
    },
}

#[derive(Debug, Clone)]
pub struct ChainStatus {
    pub id: ChainId,
    pub name: &'static str,
    pub state: ChainState,
    pub extra: ChainExtra,
}

pub struct ChainClient {
    statuses: Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop_flags: Mutex<HashMap<ChainId, Arc<AtomicBool>>>,
}

impl ChainClient {
    pub fn new() -> Self {
        let mut statuses = HashMap::new();
        for &id in ChainId::all() {
            statuses.insert(
                id,
                ChainStatus {
                    id,
                    name: id.display_name(),
                    state: ChainState::Disconnected,
                    extra: ChainExtra::None,
                },
            );
        }
        Self {
            statuses: Arc::new(Mutex::new(statuses)),
            stop_flags: Mutex::new(HashMap::new()),
        }
    }

    pub fn connect(&self, chain: ChainId) {
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut flags = self.stop_flags.lock().unwrap();
            // Signal any existing thread to stop
            if let Some(old) = flags.get(&chain) {
                old.store(true, Ordering::Relaxed);
            }
            flags.insert(chain, stop.clone());
        }
        let statuses = self.statuses.clone();
        thread::spawn(move || match chain.backend() {
            ConnectionBackend::Smoldot => run_smoldot_connection(chain, statuses, stop),
            ConnectionBackend::Rpc => run_rpc_connection(chain, statuses, stop),
            ConnectionBackend::Kyoto => super::btc::run_kyoto_connection(chain, statuses, stop),
            ConnectionBackend::Helios => super::eth::run_helios_connection(chain, statuses, stop),
        });
    }

    pub fn disconnect(&self, chain: ChainId) {
        let flags = self.stop_flags.lock().unwrap();
        if let Some(stop) = flags.get(&chain) {
            stop.store(true, Ordering::Relaxed);
        }
        drop(flags);
        set_state(&self.statuses, chain, ChainState::Disconnected);
    }

    pub fn status(&self, chain: ChainId) -> ChainStatus {
        self.statuses
            .lock()
            .unwrap()
            .get(&chain)
            .cloned()
            .unwrap_or(ChainStatus {
                id: chain,
                name: chain.display_name(),
                state: ChainState::Disconnected,
                extra: ChainExtra::None,
            })
    }

    pub fn all_statuses(&self) -> Vec<ChainStatus> {
        let map = self.statuses.lock().unwrap();
        ChainId::all()
            .iter()
            .filter_map(|id| map.get(id).cloned())
            .collect()
    }
}

impl Default for ChainClient {
    fn default() -> Self {
        Self::new()
    }
}

fn set_state(
    statuses: &Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    chain: ChainId,
    state: ChainState,
) {
    if let Ok(mut map) = statuses.lock() {
        if let Some(s) = map.get_mut(&chain) {
            s.state = state;
        }
    }
}

// ---------------------------------------------------------------------------
// Chain database persistence (fast restarts)
// ---------------------------------------------------------------------------

fn chain_db_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home).join("Library/Application Support/Epoca/chain-db")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        std::path::PathBuf::from(home).join(".epoca/chain-db")
    }
}

fn load_chain_db(key: &str) -> String {
    let path = chain_db_dir().join(format!("{key}.bin"));
    std::fs::read_to_string(path).unwrap_or_default()
}

fn save_chain_db(key: &str, data: &str) {
    let dir = chain_db_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{key}.bin"));
    let _ = std::fs::write(path, data);
}

// ---------------------------------------------------------------------------
// Smoldot light client backend
// ---------------------------------------------------------------------------

fn run_smoldot_connection(
    chain: ChainId,
    statuses: Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop: Arc<AtomicBool>,
) {
    set_state(&statuses, chain, ChainState::Connecting);

    let (relay_spec, para_spec) = match chain.chain_specs() {
        Some(specs) => specs,
        None => {
            set_state(
                &statuses,
                chain,
                ChainState::Error("no chain specs for smoldot".into()),
            );
            return;
        }
    };

    let relay_db_key = match chain {
        ChainId::PolkadotAssetHub => "polkadot-relay",
        ChainId::PaseoAssetHub => "paseo-relay",
        _ => "unknown-relay",
    };
    let para_db_key = format!("{chain:?}");

    // Run smoldot on a single thread via smol::block_on.
    // smoldot's Client is !Send — it must stay on this thread.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        smol::block_on(async {
            run_smoldot_async(
                chain,
                relay_spec,
                para_spec,
                &relay_db_key,
                &para_db_key,
                &statuses,
                &stop,
            )
            .await
        })
    }));

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let msg = e.to_string();
            log::error!("smoldot error for {chain:?}: {msg}");
            if !stop.load(Ordering::Relaxed) {
                set_state(&statuses, chain, ChainState::Error(msg));
            }
        }
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                format!("panic: {s}")
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                format!("panic: {s}")
            } else {
                "internal panic".to_string()
            };
            log::error!("smoldot panic for {chain:?}: {msg}");
            set_state(&statuses, chain, ChainState::Error(msg));
        }
    }

    if stop.load(Ordering::Relaxed) {
        set_state(&statuses, chain, ChainState::Disconnected);
    } else {
        // Retry after delay
        thread::sleep(Duration::from_secs(5));
    }
}

async fn run_smoldot_async(
    chain: ChainId,
    relay_spec: &str,
    para_spec: &str,
    relay_db_key: &str,
    para_db_key: &str,
    statuses: &Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use smoldot_light::{AddChainConfig, AddChainConfigJsonRpc, Client};

    let mut client = Client::new(smoldot_light::platform::DefaultPlatform::new(
        env!("CARGO_PKG_NAME").into(),
        env!("CARGO_PKG_VERSION").into(),
    ));

    // Load persisted database content for fast restarts
    let relay_db = load_chain_db(relay_db_key);
    let para_db = load_chain_db(para_db_key);

    // Add relay chain (no JSON-RPC needed — just for networking/finality)
    log::debug!("{chain:?}: adding relay chain...");
    let relay = client.add_chain(AddChainConfig {
        specification: relay_spec,
        database_content: &relay_db,
        potential_relay_chains: std::iter::empty(),
        json_rpc: AddChainConfigJsonRpc::Disabled,
        user_data: (),
    }).map_err(|e| format!("relay add_chain failed: {e}"))?;
    let relay_id = relay.chain_id;
    log::debug!("{chain:?}: relay chain added");

    // Add parachain with JSON-RPC enabled
    log::debug!("{chain:?}: adding parachain...");
    let para = client.add_chain(AddChainConfig {
        specification: para_spec,
        database_content: &para_db,
        potential_relay_chains: std::iter::once(relay_id),
        json_rpc: AddChainConfigJsonRpc::Enabled {
            max_pending_requests: std::num::NonZeroU32::new(128).unwrap(),
            max_subscriptions: 1024,
        },
        user_data: (),
    }).map_err(|e| format!("para add_chain failed: {e}"))?;
    log::debug!("{chain:?}: parachain added");
    let para_id = para.chain_id;
    let mut responses = para
        .json_rpc_responses
        .expect("JSON-RPC was enabled, responses must be Some");

    // Subscribe to new heads
    let sub_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "chain_subscribeNewHeads",
        "params": []
    })
    .to_string();
    client
        .json_rpc_request(sub_req, para_id)
        .map_err(|e| format!("json_rpc_request failed: {e:?}"))?;

    let mut health_id: u64 = 1000;
    let mut last_db_save = std::time::Instant::now();

    // Main event loop
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Check for SPA app RPC requests and forward to smoldot.
        for req in crate::rpc_bridge::take_requests_for_chain(chain) {
            log::debug!("[smoldot] forwarding app RPC corr={}", req.corr_id);
            if let Err(e) = client.json_rpc_request(req.json_rpc, para_id) {
                log::warn!("[smoldot] json_rpc_request failed: {e:?}");
                crate::rpc_bridge::push_response(
                    req.corr_id,
                    serde_json::json!({"jsonrpc":"2.0","id":req.corr_id,"error":{"code":-32000,"message":"request failed"}}).to_string(),
                );
            }
        }

        // Race: next response vs 2-second timer (faster polling for app requests)
        let response = smol::future::or(
            async { responses.next().await },
            async {
                smol::Timer::after(Duration::from_secs(2)).await;
                None
            },
        )
        .await;

        if stop.load(Ordering::Relaxed) {
            break;
        }

        match response {
            Some(text) => {
                // Check if this is an app RPC response (route back to SPA).
                if let Some(id) = extract_json_rpc_id(&text) {
                    if crate::rpc_bridge::is_app_request(id) {
                        crate::rpc_bridge::push_response(id, text);
                        continue;
                    }
                }
                handle_smoldot_response(chain, &text, statuses);
            }
            None => {
                // Timer fired or chain removed — send a health request
                health_id += 1;
                let health_req = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": health_id,
                    "method": "system_health",
                    "params": []
                })
                .to_string();
                let _ = client.json_rpc_request(health_req, para_id);
            }
        }

        // Periodically persist parachain database (every 60s)
        // Note: relay chain has json_rpc: Disabled, so we can only request DB from the parachain.
        if last_db_save.elapsed() > Duration::from_secs(60) {
            last_db_save = std::time::Instant::now();
            let para_db_req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 9991,
                "method": "chainHead_unstable_finalizedDatabase",
                "params": []
            })
            .to_string();
            let _ = client.json_rpc_request(para_db_req, para_id);
        }
    }

    // Clean up
    let _ = client.remove_chain(para_id);
    let _ = client.remove_chain(relay_id);

    Ok(())
}

fn handle_smoldot_response(
    chain: ChainId,
    text: &str,
    statuses: &Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
) {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Check if this is a database response (id 9991) — persist it
    if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
        if id == 9991 {
            if let Some(db) = v.get("result").and_then(|r| r.as_str()) {
                save_chain_db(&format!("{chain:?}"), db);
            }
            return;
        }
    }

    // Check for system_health response
    if let Some(result) = v.get("result") {
        if let (Some(peers), Some(is_syncing)) = (
            result.get("peers").and_then(|p| p.as_u64()),
            result.get("isSyncing").and_then(|s| s.as_bool()),
        ) {
            // Get current best block from existing state
            let current_block = if let Ok(map) = statuses.lock() {
                match map.get(&chain).map(|s| &s.state) {
                    Some(ChainState::Live { best_block, .. })
                    | Some(ChainState::Syncing { best_block, .. }) => *best_block,
                    _ => 0,
                }
            } else {
                0
            };

            if is_syncing {
                set_state(
                    statuses,
                    chain,
                    ChainState::Syncing {
                        best_block: current_block,
                        peers: peers as u32,
                    },
                );
            } else {
                set_state(
                    statuses,
                    chain,
                    ChainState::Live {
                        best_block: current_block,
                        peers: peers as u32,
                    },
                );
            }
            return;
        }
    }

    // Check for chain_newHead subscription notification
    if let Some(block) = parse_block_number(text) {
        // Preserve current peers count
        let current_peers = if let Ok(map) = statuses.lock() {
            match map.get(&chain).map(|s| &s.state) {
                Some(ChainState::Live { peers, .. })
                | Some(ChainState::Syncing { peers, .. }) => *peers,
                _ => 0,
            }
        } else {
            0
        };

        // If we have peers info already and know we're syncing, keep Syncing state
        // Otherwise assume Live (system_health will correct if syncing)
        let current_syncing = if let Ok(map) = statuses.lock() {
            matches!(
                map.get(&chain).map(|s| &s.state),
                Some(ChainState::Syncing { .. }) | Some(ChainState::Connecting)
            )
        } else {
            false
        };

        if current_syncing && current_peers > 0 {
            set_state(
                statuses,
                chain,
                ChainState::Syncing {
                    best_block: block,
                    peers: current_peers,
                },
            );
        } else {
            set_state(
                statuses,
                chain,
                ChainState::Live {
                    best_block: block,
                    peers: current_peers,
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RPC WebSocket backend (fallback for chains without smoldot support)
// ---------------------------------------------------------------------------

fn run_rpc_connection(
    chain: ChainId,
    statuses: Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop: Arc<AtomicBool>,
) {
    set_state(&statuses, chain, ChainState::Connecting);

    let endpoint = chain.endpoint();
    let result = tungstenite::connect(endpoint);

    match result {
        Ok((mut socket, _)) => {
            let sub_req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "chain_subscribeNewHeads",
                "params": []
            })
            .to_string();

            if socket
                .send(tungstenite::Message::Text(sub_req.into()))
                .is_err()
            {
                set_state(
                    &statuses,
                    chain,
                    ChainState::Error("send failed".to_string()),
                );
                return;
            }

            loop {
                if stop.load(Ordering::Relaxed) {
                    let _ = socket.close(None);
                    break;
                }

                match socket.read() {
                    Ok(tungstenite::Message::Text(text)) => {
                        if let Some(block) = parse_block_number(&text) {
                            set_state(
                                &statuses,
                                chain,
                                ChainState::Live {
                                    best_block: block,
                                    peers: 0,
                                },
                            );
                        }
                    }
                    Ok(tungstenite::Message::Close(_)) => {
                        if !stop.load(Ordering::Relaxed) {
                            set_state(
                                &statuses,
                                chain,
                                ChainState::Error("connection closed".to_string()),
                            );
                        }
                        break;
                    }
                    Ok(tungstenite::Message::Ping(data)) => {
                        let _ = socket.send(tungstenite::Message::Pong(data));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if !stop.load(Ordering::Relaxed) {
                            set_state(&statuses, chain, ChainState::Error(e.to_string()));
                        }
                        break;
                    }
                }
            }

            if stop.load(Ordering::Relaxed) {
                set_state(&statuses, chain, ChainState::Disconnected);
            }
        }
        Err(e) => {
            if !stop.load(Ordering::Relaxed) {
                set_state(&statuses, chain, ChainState::Error(e.to_string()));
            }
        }
    }

    // Retry delay if not stopped
    if !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(5));
    }
}

/// Extract the "id" field from a JSON-RPC response (for correlation routing).
fn extract_json_rpc_id(text: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("id")?.as_u64()
}

/// Parse a Substrate JSON-RPC new head notification and extract the block number.
/// The notification looks like:
/// {"jsonrpc":"2.0","method":"chain_newHead","params":{"subscription":"...","result":{"number":"0x12345",...}}}
fn parse_block_number(text: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    // Subscription notification
    let num_str = v
        .pointer("/params/result/number")
        .and_then(|n| n.as_str())?;
    let hex = num_str.strip_prefix("0x").unwrap_or(num_str);
    u64::from_str_radix(hex, 16).ok()
}
