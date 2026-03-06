//! Bitcoin chain backend — BIP-157/158 compact block filter light client via Kyoto.
//!
//! Connects directly to the Bitcoin P2P network. Downloads block headers and
//! compact filters, verifies proof-of-work locally. No trusted servers.

use crate::client::{ChainExtra, ChainId, ChainState, ChainStatus};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Run the Kyoto BIP-157/158 light client on a dedicated thread.
/// Creates a tokio current-thread runtime internally.
pub fn run_kyoto_connection(
    chain: ChainId,
    statuses: Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop: Arc<AtomicBool>,
) {
    set_state(&statuses, chain, ChainState::Connecting, ChainExtra::None);

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("Failed to create tokio runtime for BTC: {e}");
            set_state(
                &statuses,
                chain,
                ChainState::Error(format!("Runtime error: {e}")),
                ChainExtra::None,
            );
            return;
        }
    };

    let result = rt.block_on(run_kyoto_async(chain, statuses.clone(), stop));

    match result {
        Ok(()) => {
            set_state(&statuses, chain, ChainState::Disconnected, ChainExtra::None);
        }
        Err(e) => {
            log::error!("Kyoto BTC light client error: {e}");
            set_state(
                &statuses,
                chain,
                ChainState::Error(e.to_string()),
                ChainExtra::None,
            );
        }
    }
}

async fn run_kyoto_async(
    chain: ChainId,
    statuses: Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use bip157::chain::BlockHeaderChanges;
    use bip157::{Builder, Client, Event, Network};

    // Data directory for persisting headers + filters across restarts
    let data_dir = chain_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);

    let builder = Builder::new(Network::Bitcoin)
        .data_dir(&data_dir)
        .required_peers(2);

    let (node, client) = builder.build();

    // Spawn the node on a background task
    let node_handle = tokio::task::spawn(async move { node.run().await });

    let Client {
        requester,
        mut info_rx,
        mut warn_rx,
        mut event_rx,
    } = client;

    // Track best known tip
    let mut best_height: u64 = 0;
    let mut synced = false;

    loop {
        // Check stop flag periodically
        if stop.load(Ordering::Relaxed) {
            let _ = requester.shutdown();
            break;
        }

        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Some(Event::ChainUpdate(BlockHeaderChanges::Connected(header))) => {
                        best_height = header.height as u64;
                        let state = if synced {
                            ChainState::Live { best_block: best_height, peers: 0 }
                        } else {
                            ChainState::Syncing { best_block: best_height, peers: 0 }
                        };
                        set_state(
                            &statuses, chain, state,
                            ChainExtra::Btc { tip_height: best_height, fee_rate_sat_vb: 0 },
                        );
                    }
                    Some(Event::FiltersSynced(update)) => {
                        synced = true;
                        best_height = update.tip().height as u64;
                        set_state(
                            &statuses, chain,
                            ChainState::Live { best_block: best_height, peers: 0 },
                            ChainExtra::Btc { tip_height: best_height, fee_rate_sat_vb: 0 },
                        );
                        log::info!("Bitcoin light client synced to height {best_height}");
                    }
                    Some(Event::IndexedFilter(_filter)) => {
                        // In a future phase, we'll check filters against watched scripts
                        // for wallet balance tracking. For now just track sync progress.
                    }
                    Some(Event::Block(_indexed_block)) => {
                        // Block fetched (if we requested one). Future use for tx scanning.
                    }
                    Some(Event::ChainUpdate(BlockHeaderChanges::Reorganized { .. })) => {
                        // Chain reorg detected. Re-sync will happen automatically.
                        log::warn!("Bitcoin chain reorganization detected");
                    }
                    None => {
                        // Channel closed — node shut down
                        break;
                    }
                    _ => {}
                }
            }
            info = info_rx.recv() => {
                if let Some(info) = info {
                    log::debug!("BTC: {info}");
                }
            }
            warn = warn_rx.recv() => {
                if let Some(warn) = warn {
                    log::warn!("BTC: {warn}");
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                // Periodic stop-flag check
                if stop.load(Ordering::Relaxed) {
                    let _ = requester.shutdown();
                    break;
                }
            }
        }
    }

    // Wait for node task to finish
    let _ = node_handle.await;
    Ok(())
}

fn chain_data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home).join("Library/Application Support/Epoca/chain-db/bitcoin")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        std::path::PathBuf::from(home).join(".epoca/chain-db/bitcoin")
    }
}

fn set_state(
    statuses: &Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    chain: ChainId,
    state: ChainState,
    extra: ChainExtra,
) {
    if let Ok(mut map) = statuses.lock() {
        if let Some(s) = map.get_mut(&chain) {
            s.state = state;
            s.extra = extra;
        }
    }
}
