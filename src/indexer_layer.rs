use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tonic::transport::ClientTlsConfig;
use tonic::Status;

pub mod sui {
    pub mod rpc {
        pub mod v2 {
            tonic::include_proto!("sui.rpc.v2");
        }
    }
}

use prost_types::FieldMask;
use sui::rpc::v2::subscription_service_client::SubscriptionServiceClient;
use sui::rpc::v2::SubscribeCheckpointsRequest;
use tonic::transport::Endpoint;

#[derive(Debug, Clone)]
pub struct InternalPaymentEvent {
    pub address: String,
    pub amount: String,
    pub tx_digest: String,
    pub timestamp: i64,
}

// --- JSON RPC STRUCTS ---
#[derive(Deserialize, Debug)]
struct JsonRpcResponse<T> {
    result: Option<T>,
}

// We use this for 'suix_queryTransactionBlocks'
#[derive(Deserialize, Debug)]
struct QueryResponse {
    data: Vec<TransactionBlockData>,
}

#[derive(Deserialize, Debug)]
struct TransactionBlockData {
    digest: String,
    #[serde(rename = "timestampMs")]
    timestamp_ms: Option<String>,
    #[serde(rename = "balanceChanges")]
    balance_changes: Option<Vec<BalanceChange>>,
}

#[derive(Deserialize, Debug)]
struct BalanceChange {
    #[serde(rename = "coinType")]
    coin_type: String,
    amount: String,
    owner: Option<Owner>,
}

#[derive(Deserialize, Debug)]
struct Owner {
    #[serde(rename = "AddressOwner")]
    address_owner: Option<String>,
}

// --- MONITOR MANAGER ---
type Listeners =
    Arc<RwLock<HashMap<String, Vec<mpsc::Sender<Result<InternalPaymentEvent, Status>>>>>>;

pub struct MonitorManager {
    listeners: Listeners,
    is_running: Arc<RwLock<bool>>,
}

impl MonitorManager {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(RwLock::new(HashMap::new())),
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    pub fn add_listener(
        &self,
        address: String,
        tx: mpsc::Sender<Result<InternalPaymentEvent, Status>>,
    ) {
        let address_lower = address.to_lowercase();
        {
            let mut map = self.listeners.write().unwrap();
            map.entry(address_lower.clone())
                .or_insert_with(Vec::new)
                .push(tx);
            println!(
                "📝 Watching: {} (Total Listeners: {})",
                address_lower,
                map.get(&address_lower).unwrap().len()
            );
        }

        let running = *self.is_running.read().unwrap();
        if !running {
            self.start_monitor_loop();
        }
    }

    pub fn remove_listener(&self, address: &str) {
        let address_lower = address.to_lowercase();
        let mut map = self.listeners.write().unwrap();
        map.remove(&address_lower);

        if map.is_empty() {
            println!("💤 No listeners left. Stopping poller.");
            *self.is_running.write().unwrap() = false;
        }
    }

    fn start_monitor_loop(&self) {
        let listeners_clone = self.listeners.clone();
        let is_running_clone = self.is_running.clone();

        // Mark as running
        *is_running_clone.write().unwrap() = true;

        tokio::spawn(async move {
            println!("🚀 LOW LATENCY gRPC Stream Started.");
            let rpc_endpoint = "https://fullnode.mainnet.sui.io:443";

            loop {
                // Check if we should stop (Kill Switch)
                if listeners_clone.read().unwrap().is_empty() {
                    println!("zzz No listeners. Stopping stream.");
                    *is_running_clone.write().unwrap() = false;
                    break;
                }

                println!("⏳ Connecting to Sui RPC...");

                // Configure the Connection
                let channel_result = Endpoint::from_static(rpc_endpoint)
                    .tls_config(ClientTlsConfig::new().with_native_roots())
                    .unwrap()
                    .connect_timeout(Duration::from_secs(5))
                    .keep_alive_timeout(Duration::from_secs(10))
                    .keep_alive_while_idle(true)
                    .http2_keep_alive_interval(Duration::from_secs(15))
                    .connect()
                    .await;

                match channel_result {
                    Ok(channel) => {
                        let mut client = SubscriptionServiceClient::new(channel)
                            .max_decoding_message_size(usize::MAX);

                        let request = tonic::Request::new(SubscribeCheckpointsRequest {
                            read_mask: Some(FieldMask {
                                paths: vec![
                                    "sequence_number".to_string(),
                                    "digest".to_string(),
                                    "summary.timestamp_ms".to_string(),
                                    "transactions.balance_changes".to_string(),
                                    "transactions.digest".to_string(),
                                ],
                            }),
                            ..Default::default()
                        });

                        match client.subscribe_checkpoints(request).await {
                            Ok(response) => {
                                let mut stream = response.into_inner();
                                println!("✅ Stream Connected! Waiting for payments...");

                                while let Ok(Some(msg)) = stream.message().await {
                                    if let Some(checkpoint) = msg.checkpoint {
                                        for tx in checkpoint.transactions {
                                            for change in tx.balance_changes {
                                                if let Some(owner_address) = change.address.as_ref()
                                                {
                                                    let owner_lower = owner_address.to_lowercase();

                                                    // Optional: Check if token is USDC
                                                    let is_usdc = change
                                                        .coin_type
                                                        .as_deref()
                                                        .map(|t| t.to_lowercase().contains("usdc"))
                                                        .unwrap_or(false);

                                                    if is_usdc {
                                                        // ---------------------------------------------------------
                                                        // STEP 1: OPEN LOCK, CLONE CHANNELS, CLOSE LOCK IMMEDIATELY
                                                        // ---------------------------------------------------------
                                                        // We use a block { ... } to ensure the lock is dropped before await
                                                        let targets = {
                                                            let listeners_map =
                                                                listeners_clone.read().unwrap();
                                                            listeners_map.get(&owner_lower).cloned()
                                                        };

                                                        // ---------------------------------------------------------
                                                        // STEP 2: SEND ASYNC (Safe now because lock is gone)
                                                        // ---------------------------------------------------------
                                                        if let Some(channels) = targets {
                                                            println!(
                                                                "🚨 FAST PAYMENT DETECTED FOR: {}",
                                                                owner_lower
                                                            );

                                                            let event = InternalPaymentEvent {
                                                                address: owner_lower.clone(),
                                                                amount: change
                                                                    .amount
                                                                    .unwrap_or_default()
                                                                    .to_string(),
                                                                tx_digest: tx
                                                                    .digest
                                                                    .clone()
                                                                    .unwrap_or_default(),
                                                                timestamp: checkpoint
                                                                    .summary
                                                                    .as_ref()
                                                                    .and_then(|s| {
                                                                        s.timestamp.as_ref()
                                                                    })
                                                                    .map(|t| {
                                                                        t.seconds * 1000
                                                                            + (t.nanos as i64
                                                                                / 1_000_000)
                                                                    })
                                                                    .unwrap_or(0),
                                                            };

                                                            for tx_channel in channels {
                                                                // Now we can await safely!
                                                                let _ = tx_channel
                                                                    .send(Ok(event.clone()))
                                                                    .await;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                println!("❌ Stream ended by server. Reconnecting...");
                            }
                            Err(e) => println!("❌ Subscription failed: {:?}. Retrying...", e),
                        }
                    }
                    Err(e) => println!("❌ Failed to connect: {:?}. Retrying...", e),
                }

                sleep(Duration::from_secs(1)).await;
            }
        });
    }
}

// --- HELPER FUNCTIONS ---

async fn fetch_latest_checkpoint(client: &Client, url: &str) -> Result<u64, String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sui_getLatestCheckpointSequenceNumber",
        "params": []
    });

    let resp = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|e: reqwest::Error| e.to_string())?;

    let body = resp
        .json::<JsonRpcResponse<String>>()
        .await
        .map_err(|e: reqwest::Error| e.to_string())?;

    body.result
        .ok_or("No result".to_string())?
        .parse::<u64>()
        .map_err(|e| e.to_string())
}

// UPDATED: Using 'suix_queryTransactionBlocks'
async fn fetch_txs_for_checkpoint(
    client: &Client,
    url: &str,
    seq: u64,
) -> Result<Option<QueryResponse>, String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "suix_queryTransactionBlocks",
        "params": [
            {
                "filter": {
                    "Checkpoint": seq.to_string()
                },
                "options": {
                    "showBalanceChanges": true,
                    "showInput": false,
                    "showEffects": false,
                    "showEvents": false
                }
            },
            null, // nextCursor
            null, // limit
            false // descending
        ]
    });

    let resp_result: Result<reqwest::Response, reqwest::Error> =
        client.post(url).json(&payload).send().await;

    match resp_result {
        Ok(r) => {
            if !r.status().is_success() {
                // Checkpoint probably doesn't exist yet or bad request
                return Ok(None);
            }

            // Map to our new QueryResponse struct
            let body_result: Result<JsonRpcResponse<QueryResponse>, reqwest::Error> =
                r.json().await;

            match body_result {
                Ok(body) => Ok(body.result),
                Err(e) => Err(format!("Parsing Error: {}", e)),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}
