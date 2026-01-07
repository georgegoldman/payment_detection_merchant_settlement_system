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
    tonic::include_proto!("payment"); // Must match package name in .proto
}
use payment::payment_service_server::{PaymentService, PaymentServiceServer};
use payment::{PaymentEvent, WatchRequest};

// 3. Define the Service Struct
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

        let (tx, rx) = mpsc::channel(4);
        let (internal_tx, mut internal_rx) = mpsc::channel(4);

        // 1. Register with Engine
        self.manager.add_listener(address.clone(), internal_tx);

        // 2. Clone the manager so we can call 'remove' later
        let manager_clone = self.manager.clone();
        let address_clone = address.clone();

        // 3. Spawn the task with a Drop Guard
        tokio::spawn(async move {
            // This logic runs when the client connects
            while let Some(res) = internal_rx.recv().await {
                match res {
                    Ok(event) => {
                        let proto_event = PaymentEvent {
                            tx_digest: event.tx_digest,
                            amount: event.amount,
                            token: "USDC".to_string(),
                            timestamp: event.timestamp,
                        };
                        // If sending fails, it means Client disconnected (Ctrl+C)
                        if tx.send(Ok(proto_event)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                    }
                }
            }

            // --- CLEANUP SECTION ---
            // This runs automatically when the loop breaks (Ctrl+C or error)
            println!(
                "🔌 Client disconnected from {}. Cleaning up...",
                address_clone
            );
            manager_clone.remove_listener(&address_clone);
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // This line registers the "Ring" encryption provider so rustls doesn't crash.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    let port = env::var("PORT").unwrap_or_else(|_| "50051".to_string());
    let addr = format!("0.0.0.0:{}", port).parse()?;

    // Initialize the Engine
    let manager = Arc::new(MonitorManager::new());

    let service = MyPaymentService {
        manager: manager.clone(),
    };

    println!("🚀 Payment gRPC Server running on {}", addr);

    Server::builder()
        .add_service(PaymentServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
