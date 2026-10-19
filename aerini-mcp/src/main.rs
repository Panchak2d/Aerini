use std::process::ExitCode;

use aerini_mcp::{AerinMcp, Config};
use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("aerini-mcp: {message}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), String> {
    let server = AerinMcp::new(Config::from_env_and_args()?)?;
    let service = server
        .serve(stdio())
        .await
        .map_err(|e| format!("cannot start the MCP server: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| format!("the MCP session ended with an error: {e}"))?;
    Ok(())
}
