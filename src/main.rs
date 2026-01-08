use std::env;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

// 1. Import the Engine
mod indexer_layer;
use indexer_layer::MonitorManager;

// 2. Import generated proto code
pub mod payment {
    tonic::include_proto!("payment");
}
use payment::payment_service_server::{PaymentService, PaymentServiceServer};
use payment::{PaymentEvent, WatchRequest};

pub struct MyPaymentService {
    manager: Arc<MonitorManager>,
}

#[tonic::async_trait]
impl PaymentService for MyPaymentService {
    type WatchAddressStream = ReceiverStream<Result<PaymentEvent, Status>>;

    async fn watch_address(
        &self,
        request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchAddressStream>, Status> {
        let req = request.into_inner();
        let address = req.address;

        println!("🆕 New Client Watching: {}", address);

        // Channel to send data to the Client (Tonic)
        let (tx, rx) = mpsc::channel(4);
        // Channel to receive data from the Monitor
        let (internal_tx, mut internal_rx) = mpsc::channel(4);

        // 1. Register with Engine
        self.manager.add_listener(address.clone(), internal_tx);

        let manager_clone = self.manager.clone();
        let address_clone = address.clone();

        // 2. Spawn task with "Active Watch" logic
        tokio::spawn(async move {
            loop {
                // 👇 THIS IS THE MAGIC FIX: "SELECT!"
                // It waits for EITHER a payment OR a disconnect signal.
                tokio::select! {
                    // OPTION A: We received a payment from the blockchain
                    maybe_event = internal_rx.recv() => {
                        match maybe_event {
                            Some(res) => {
                                match res {
                                    Ok(event) => {
                                        let proto_event = PaymentEvent {
                                            tx_digest: event.tx_digest,
                                            amount: event.amount,
                                            token: "USDC".to_string(),
                                            timestamp: event.timestamp,
                                        };
                                        // If sending fails, client is gone
                                        if tx.send(Ok(proto_event)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Err(e)).await;
                                    }
                                }
                            }
                            None => break, // Monitor channel closed
                        }
                    }

                    // OPTION B: The Client disconnected (Ctrl+C detected!)
                    _ = tx.closed() => {
                        println!("🔌 Client disconnected from {}", address_clone);
                        break;
                    }
                }
            }

            // --- CLEANUP SECTION ---
            // This runs immediately when the loop breaks
            println!("🧹 Removing listener for: {}", address_clone);
            manager_clone.remove_listener(&address_clone);
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rustls Crypto Fix
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Port Configuration
    let port = env::var("PORT").unwrap_or_else(|_| "50051".to_string());
    let addr = format!("0.0.0.0:{}", port).parse()?;

    // Initialize Engine
    let manager = Arc::new(MonitorManager::new());

    let service = MyPaymentService {
        manager: manager.clone(),
    };

    println!("🚀 Payment gRPC Server running on {}", addr);

    // Build Server (with HTTP/1 support for future web use)
    Server::builder()
        .accept_http1(true)
        .add_service(PaymentServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
