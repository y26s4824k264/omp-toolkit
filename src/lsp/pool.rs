//! Persistent workspace-keyed LSP client pool.
//! Keeps language servers warm across calls and restarts crashed instances lazily.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use tokio::sync::Mutex;

use crate::lsp::{client::LspClient, config::ResolvedServer};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    server_name: String,
    workspace_root: PathBuf,
}

#[derive(Default)]
pub struct LspPool {
    clients: Mutex<HashMap<PoolKey, Arc<LspClient>>>,
}

impl LspPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a live persistent client for `(server, workspace_root)`, starting it if needed.
    pub async fn get_or_start(
        &self,
        server: &ResolvedServer,
        timeout: Duration,
    ) -> Result<Arc<LspClient>> {
        let key = PoolKey {
            server_name: server.def.name.clone(),
            workspace_root: server.workspace_root.clone(),
        };
        {
            let mut guard = self.clients.lock().await;
            if let Some(existing) = guard.get(&key) {
                if !existing.is_dead() {
                    return Ok(Arc::clone(existing));
                }
                guard.remove(&key);
            }
        }

        let started = LspClient::start(server.clone(), timeout).await?;
        let mut guard = self.clients.lock().await;
        if let Some(existing) = guard.get(&key)
            && !existing.is_dead()
        {
            let keep = Arc::clone(existing);
            drop(guard);
            started.shutdown().await;
            return Ok(keep);
        }
        guard.insert(key, Arc::clone(&started));
        Ok(started)
    }

    /// List currently warm server instances in the pool.
    pub async fn active_servers(&self) -> Vec<(String, PathBuf)> {
        let mut guard = self.clients.lock().await;
        guard.retain(|_, client| !client.is_dead());
        guard
            .keys()
            .map(|k| (k.server_name.clone(), k.workspace_root.clone()))
            .collect()
    }

    /// Gracefully shut down all persistent language servers.
    pub async fn shutdown_all(&self) {
        let clients: Vec<Arc<LspClient>> = {
            let mut guard = self.clients.lock().await;
            guard.drain().map(|(_, c)| c).collect()
        };
        for client in clients {
            client.shutdown().await;
        }
    }
}
