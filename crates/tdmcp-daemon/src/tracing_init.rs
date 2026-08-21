//! Tracing defaults: EnvFilter + fmt.

use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

use crate::config::Config;

/// Install the global subscriber. `RUST_LOG` / `TDMCP_LOG` control filter.
pub fn init(_cfg: &Config) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| {
            EnvFilter::try_new(
                "tdmcp_daemon=info,tdmcp_gui=info,tdmcp_core=info,tdmcp_mcp=info,tdmcp_ipc=info",
            )
        })
        .unwrap_or_else(|_| EnvFilter::new("info"));

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}
