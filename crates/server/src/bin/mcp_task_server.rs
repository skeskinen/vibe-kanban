use std::str::FromStr;

use rmcp::{ServiceExt, transport::stdio};
use server::mcp::task_server::TaskServer;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use tracing_subscriber::{EnvFilter, prelude::*};
use utils::assets::asset_dir;

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(std::io::stderr)
                        .with_filter(EnvFilter::new("debug")),
                )
                .init();

            let version = env!("CARGO_PKG_VERSION");
            tracing::debug!("[MCP] Starting MCP task server version {version}...");

            // Database connection
            let database_url = format!(
                "sqlite://{}",
                asset_dir().join("db.sqlite").to_string_lossy()
            );

            let options = SqliteConnectOptions::from_str(&database_url)?.create_if_missing(false);
            let pool = SqlitePool::connect_with(options).await?;

            let service = TaskServer::new(pool)
                .serve(stdio())
                .await
                .inspect_err(|e| {
                    tracing::error!("serving error: {:?}", e);
                })?;

            service.waiting().await?;
            Ok(())
        })
}
