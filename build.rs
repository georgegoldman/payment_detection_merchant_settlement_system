fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Compile the Payment Service (Your API)
    tonic_build::compile_protos("proto/payment_service.proto")?;

    // 2. Compile the Sui RPC protos (For the Engine)
    // Ensure you have the 'proto/sui/...' folder we set up earlier
    tonic_build::configure()
        .build_server(false) // We only need the client for Sui
        .compile(&["proto/sui/rpc/v2/subscription_service.proto"], &["proto"])?;

    Ok(())
}
