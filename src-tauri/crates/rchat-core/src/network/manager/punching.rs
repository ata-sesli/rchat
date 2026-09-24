use super::*;

fn punch_backoff(attempt: u32) -> std::time::Duration {
    let exponent = attempt.saturating_sub(1).min(3);
    std::time::Duration::from_millis(500u64 * 2u64.pow(exponent))
}

impl NetworkManager {
    /// Register a pending shadow poll (called when creating an invite).
    pub(super) fn register_shadow_poll(
        &mut self,
        invitee: &str,
        password: &str,
        my_username: &str,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.pending_shadow_polls.insert(
            invitee.to_string(),
            (password.to_string(), my_username.to_string(), now),
        );
        println!("[Shadow] 📋 Registered poll for {}", invitee);
    }

    /// Poll for shadow invites from all pending invitees
    pub(super) async fn poll_shadow_invites(&mut self) {
        use crate::network::gist;
        use crate::network::invite;

        if self.pending_shadow_polls.is_empty() {
            return;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.pending_shadow_polls
            .retain(|_, (_, _, created)| now - *created < 120);

        let invitees: Vec<String> = self.pending_shadow_polls.keys().cloned().collect();

        for invitee in invitees {
            let (password, my_username, _) = match self.pending_shadow_polls.get(&invitee) {
                Some(v) => v.clone(),
                None => continue,
            };

            match gist::get_friend_shadows(&invitee).await {
                Ok(shadows) => {
                    for shadow in shadows {
                        match invite::decrypt_shadow_invite(
                            &shadow,
                            &password,
                            &my_username,
                            &invitee,
                        ) {
                            Ok(Some(payload)) => {
                                println!(
                                    "[Shadow] 🎯 Found shadow from {}: {}",
                                    invitee, payload.invitee_address
                                );
                                if let Ok(addr) = payload.invitee_address.parse::<Multiaddr>() {
                                    self.add_punch_target_for_peer(
                                        &invitee,
                                        addr,
                                        payload.invitee_peer_id.parse().ok(),
                                    );
                                }
                                self.pending_shadow_polls.remove(&invitee);
                            }
                            Ok(None) => {}
                            Err(e) => eprintln!("[Shadow] Decrypt error: {}", e),
                        }
                    }
                }
                Err(e) => eprintln!("[Shadow] Failed to fetch shadows from {}: {:?}", invitee, e),
            }
        }
    }

    /// Punch active targets with bounded attempts and exponential backoff.
    pub(super) fn punch_active_targets(&mut self) {
        if self.active_punch_targets.is_empty() {
            return;
        }

        let now = std::time::Instant::now();
        let names = self.active_punch_targets.keys().cloned().collect::<Vec<_>>();

        for name in names {
            let Some(target) = self.active_punch_targets.get(&name).cloned() else {
                continue;
            };
            let elapsed = now.saturating_duration_since(target.started_at);
            if elapsed > PUNCH_WINDOW {
                self.active_punch_targets.remove(&name);
                self.emit_connectivity_state(&name, "failed", "punch_timeout", target.attempt);
                println!("[Punch] ⏰ Timeout for {}", name);
                continue;
            }
            if target.attempt >= MAX_PUNCH_ATTEMPTS || now < target.next_attempt_at {
                if target.attempt >= MAX_PUNCH_ATTEMPTS {
                    self.active_punch_targets.remove(&name);
                    self.emit_connectivity_state(
                        &name,
                        "failed",
                        "unreachable_peer",
                        target.attempt,
                    );
                }
                continue;
            }

            let attempt = target.attempt + 1;
            self.emit_connectivity_state(&name, "punching", "retry", attempt);
            self.record_outgoing_dial(&target.address, OutgoingDialSource::Punch);
            let _ = self.swarm.dial(target.address.clone());
            let next_attempt_at = now + punch_backoff(attempt);
            if let Some(target) = self.active_punch_targets.get_mut(&name) {
                target.attempt = attempt;
                target.next_attempt_at = next_attempt_at;
            }
            println!("[Punch] 📤 {}/{} to {}", attempt, MAX_PUNCH_ATTEMPTS, name);
        }
    }

    /// Add or replace a target and restart its bounded retry window.
    pub(super) fn add_punch_target(&mut self, name: &str, addr: Multiaddr) {
        self.add_punch_target_for_peer(name, addr, None);
    }

    pub(super) fn add_punch_target_for_peer(
        &mut self,
        name: &str,
        addr: Multiaddr,
        peer_id: Option<PeerId>,
    ) {
        println!("[Punch] 🎯 Added target: {} -> {}", name, addr);
        let now = std::time::Instant::now();
        self.active_punch_targets.insert(
            name.to_string(),
            PunchTarget {
                address: addr,
                peer_id,
                started_at: now,
                next_attempt_at: now,
                attempt: 0,
            },
        );
    }

    /// Remove a target after connection or explicit cancellation.
    pub(super) fn remove_punch_target(&mut self, name: &str) -> bool {
        if self.active_punch_targets.remove(name).is_some() {
            println!("[Punch] 🎉 {} connected, removed from targets", name);
            true
        } else {
            false
        }
    }

    pub(super) fn emit_connectivity_state(
        &self,
        peer_id: &str,
        state: &str,
        reason: &str,
        attempt: u32,
    ) {
        self.emit(crate::events::CoreEvent::ConnectivityStateUpdated(
            crate::events::ConnectivityStateUpdatedEvent {
                peer_id: peer_id.to_string(),
                state: state.to_string(),
                reason: reason.to_string(),
                attempt,
                max_attempts: MAX_PUNCH_ATTEMPTS,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::punch_backoff;

    #[test]
    fn punch_backoff_is_bounded_and_increases() {
        assert_eq!(punch_backoff(1).as_millis(), 500);
        assert_eq!(punch_backoff(2).as_millis(), 1_000);
        assert_eq!(punch_backoff(3).as_millis(), 2_000);
        assert_eq!(punch_backoff(4).as_millis(), 4_000);
        assert_eq!(punch_backoff(99).as_millis(), 4_000);
    }
}
