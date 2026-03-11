//! Ethereum chain backend — Helios light client (consensus + execution verification).
//!
//! Connects to the Ethereum beacon chain via public consensus RPC and verifies
//! execution state via public execution RPC. Checkpoint data persisted locally
//! for fast restarts after first sync.

use crate::client::{ChainExtra, ChainId, ChainState, ChainStatus};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Public beacon chain API (a16z-operated, no key needed — Helios's own default).
const CONSENSUS_RPC: &str = "https://ethereum.operationsolarstorm.org";
/// Public execution RPC aggregator (no key needed).
const EXECUTION_RPC: &str = "https://eth.llamarpc.com";

/// Public Sepolia consensus RPC.
const SEPOLIA_CONSENSUS_RPC: &str = "https://ethereum-sepolia-beacon-api.publicnode.com";
/// Public Sepolia execution RPC.
const SEPOLIA_EXECUTION_RPC: &str = "https://ethereum-sepolia-rpc.publicnode.com";

/// Run the Helios Ethereum light client on a dedicated thread.
/// Creates a tokio current-thread runtime internally.
pub fn run_helios_connection(
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
            log::error!("Failed to create tokio runtime for ETH: {e}");
            set_state(
                &statuses,
                chain,
                ChainState::Error(format!("Runtime error: {e}")),
                ChainExtra::None,
            );
            return;
        }
    };

    let result = rt.block_on(run_helios_async(chain, statuses.clone(), stop));

    match result {
        Ok(()) => {
            set_state(&statuses, chain, ChainState::Disconnected, ChainExtra::None);
        }
        Err(e) => {
            log::error!("Helios ETH light client error: {e}");
            set_state(
                &statuses,
                chain,
                ChainState::Error(e.to_string()),
                ChainExtra::None,
            );
        }
    }
}

async fn run_helios_async(
    chain: ChainId,
    statuses: Arc<Mutex<HashMap<ChainId, ChainStatus>>>,
    stop: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use helios_ethereum::{database::FileDB, EthereumClientBuilder};

    let is_sepolia = chain == ChainId::EthereumSepolia;

    let dir_suffix = if is_sepolia { "ethereum-sepolia" } else { "ethereum" };
    let data_dir = chain_data_dir(dir_suffix);
    let _ = std::fs::create_dir_all(&data_dir);

    let (consensus_rpc, execution_rpc) = if is_sepolia {
        (SEPOLIA_CONSENSUS_RPC, SEPOLIA_EXECUTION_RPC)
    } else {
        (CONSENSUS_RPC, EXECUTION_RPC)
    };

    let mut builder = EthereumClientBuilder::<FileDB>::new()
        .consensus_rpc(consensus_rpc)?
        .execution_rpc(execution_rpc)?
        .data_dir(data_dir)
        .load_external_fallback();

    if is_sepolia {
        builder = builder.network(helios_ethereum::config::networks::Network::Sepolia);
    }

    let client = builder
        .build()
        .map_err(|e| format!("Helios build error: {e}"))?;

    set_state(
        &statuses,
        chain,
        ChainState::Syncing {
            best_block: 0,
            peers: 0,
        },
        ChainExtra::None,
    );
    log::info!("Helios: waiting for beacon chain sync...");

    // wait_synced() blocks until beacon head is verified.
    // Warm FileDB restart: <5s. First run: 30–90s.
    let synced = tokio::select! {
        result = client.wait_synced() => {
            result.map_err(|e| format!("Helios sync error: {e}"))?;
            true
        }
        _ = poll_stop_flag(stop.clone()) => false,
    };

    if !synced || stop.load(Ordering::Relaxed) {
        client.shutdown().await;
        return Ok(());
    }

    log::info!("Helios: beacon chain synced, entering poll loop");

    // Poll block number + gas price every 12s (one Ethereum slot).
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        match fetch_chain_data(&client).await {
            Ok((block_number, gas_price_gwei)) => {
                set_state(
                    &statuses,
                    chain,
                    ChainState::Live {
                        best_block: block_number,
                        peers: 0,
                    },
                    ChainExtra::Eth {
                        finalized_block: block_number,
                        gas_price_gwei,
                    },
                );
            }
            Err(e) => {
                log::warn!("Helios poll error (will retry): {e}");
            }
        }

        // Sleep 12s but check stop flag every second.
        for _ in 0..12 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    client.shutdown().await;
    Ok(())
}

async fn fetch_chain_data(
    client: &helios_ethereum::EthereumClient,
) -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    let block_number: u64 = client
        .get_block_number()
        .await
        .map_err(|e| format!("get_block_number: {e}"))?
        .to();

    let gas_price_wei: u128 = client
        .get_gas_price()
        .await
        .map_err(|e| format!("get_gas_price: {e}"))?
        .to();

    let gas_price_gwei = (gas_price_wei / 1_000_000_000).min(u64::MAX as u128) as u64;

    Ok((block_number, gas_price_gwei))
}

/// Async helper that resolves when the stop flag goes high.
async fn poll_stop_flag(stop: Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

fn chain_data_dir(suffix: &str) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home)
            .join(format!("Library/Application Support/Epoca/chain-db/{suffix}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        std::path::PathBuf::from(home).join(format!(".epoca/chain-db/{suffix}"))
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
