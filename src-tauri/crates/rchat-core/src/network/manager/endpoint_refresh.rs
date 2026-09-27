use super::*;

impl NetworkManager {
    pub(super) fn start_endpoint_refresh(&mut self) {
        if self.endpoint_refresh_task.is_none()
            && self.network_state.public_endpoint.needs_refresh()
        {
            let observer = self.network_state.public_endpoint.clone();
            self.endpoint_refresh_task =
                Some(tokio::spawn(async move { observer.refresh().await }));
        }
    }

    pub async fn dispatch_command(&mut self, command: NetworkCommand) {
        let needs_endpoint = matches!(
            command,
            NetworkCommand::StartPunch { .. } | NetworkCommand::RequestConnection { .. }
        );
        // Session registration is local lifecycle state, not a dial. Keep it
        // ordered with freeze/commit; punch_active_targets gates its later I/O.
        if needs_endpoint {
            self.start_endpoint_refresh();
            self.sync_public_endpoint().await;
            if self.endpoint_refresh_task.is_some() || self.endpoint_publication_task.is_some() {
                if self.endpoint_pending_commands.len() < 32 {
                    self.endpoint_pending_commands.push_back(command);
                } else {
                    eprintln!(
                        "[STUN] Pending connection limit reached; retry the connection request"
                    );
                }
                return;
            }
        }
        self.dispatch_ready_command(command).await;
    }

    pub(super) async fn finish_endpoint_refresh(&mut self) {
        // Discovery publication precedes any newly coordinated punch attempt.
        self.sync_public_endpoint().await;
        self.release_endpoint_commands().await;
    }

    pub(super) async fn finish_endpoint_publication(&mut self) {
        if let Some(addresses) = self.endpoint_publication_pending.take() {
            self.start_endpoint_publication(addresses);
        }
        self.release_endpoint_commands().await;
    }

    async fn release_endpoint_commands(&mut self) {
        if self.endpoint_refresh_task.is_some() || self.endpoint_publication_task.is_some() {
            return;
        }
        while let Some(command) = self.endpoint_pending_commands.pop_front() {
            self.dispatch_ready_command(command).await;
        }
    }

    pub(super) fn start_endpoint_publication(&mut self, addresses: Vec<String>) {
        if !self.is_github_sync_enabled() {
            return;
        }
        if self.endpoint_publication_task.is_some() {
            // At most one writer and one replacement snapshot. Intermediate
            // interface changes need not each produce a separate HTTP request.
            self.endpoint_publication_pending = Some(addresses);
            return;
        }
        let state = self.app_state.clone();
        self.endpoint_publication_task = Some(tokio::spawn(async move {
            let publish = async {
                let token = {
                    let mgr = state.config_manager.lock().await;
                    let config = mgr.load().await?;
                    if !config.user.connectivity.github_sync_enabled {
                        return Ok::<(), anyhow::Error>(());
                    }
                    config.system.github_token
                };
                if let Some(token) = token {
                    crate::network::discovery::publish_peer_info(&token, addresses, &state).await?;
                }
                Ok(())
            };
            match tokio::time::timeout(std::time::Duration::from_secs(15), publish).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("[STUN] Endpoint publication failed: {error}"),
                Err(_) => eprintln!(
                    "[STUN] Endpoint publication timed out; continuing direct/LAN attempts"
                ),
            }
        }));
    }

    pub(super) async fn sync_public_endpoint(&mut self) {
        let current = self
            .network_state
            .public_endpoint
            .observation()
            .fresh(std::time::Instant::now())
            .and_then(|addr| {
                crate::network::endpoint::multiaddr(addr)
                    .parse::<Multiaddr>()
                    .ok()
            });
        if current == self.advertised_public_endpoint {
            return;
        }
        if let Some(previous) = self.advertised_public_endpoint.take() {
            self.swarm.remove_external_address(&previous);
        }
        if let Some(address) = &current {
            self.swarm.add_external_address(address.clone());
            println!("[STUN] Verified active QUIC endpoint: {address}");
        } else {
            eprintln!("[STUN] No fresh public endpoint; retaining LAN/direct listener candidates");
        }
        self.advertised_public_endpoint = current;
        self.publish_listeners().await;
    }
}
