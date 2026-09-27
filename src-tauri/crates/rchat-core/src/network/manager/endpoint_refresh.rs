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
            NetworkCommand::StartPunch { .. }
                | NetworkCommand::RegisterTemporarySession { .. }
                | NetworkCommand::RequestConnection { .. }
        );
        if needs_endpoint {
            self.start_endpoint_refresh();
            if self.endpoint_refresh_task.is_some() {
                if self.endpoint_pending_commands.len() < 32 {
                    self.endpoint_pending_commands.push_back(command);
                } else {
                    eprintln!(
                        "[STUN] Pending connection limit reached; retry the connection request"
                    );
                }
                return;
            }
            self.sync_public_endpoint().await;
        }
        self.dispatch_ready_command(command).await;
    }

    pub(super) async fn finish_endpoint_refresh(&mut self) {
        // Discovery publication precedes any newly coordinated punch attempt.
        self.sync_public_endpoint().await;
        while let Some(command) = self.endpoint_pending_commands.pop_front() {
            self.dispatch_ready_command(command).await;
        }
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
