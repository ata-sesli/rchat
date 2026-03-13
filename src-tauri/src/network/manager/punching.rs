use super::*;

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

        // Skip if no pending polls
        if self.pending_shadow_polls.is_empty() {
            return;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Remove expired polls (2 minute TTL)
        self.pending_shadow_polls
            .retain(|_, (_, _, created)| now - *created < 120);

        // Clone keys to avoid borrow issues
        let invitees: Vec<String> = self.pending_shadow_polls.keys().cloned().collect();

        for invitee in invitees {
            let (password, my_username, _) = match self.pending_shadow_polls.get(&invitee) {
                Some(v) => v.clone(),
                None => continue,
            };

            // Fetch shadow invites from invitee's Gist
            match gist::get_friend_shadows(&invitee).await {
                Ok(shadows) => {
                    for shadow in shadows {
                        // Try to decrypt with our key
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

                                // Add to active punch targets for continuous punching
                                if let Ok(addr) = payload.invitee_address.parse::<Multiaddr>() {
                                    self.add_punch_target(&invitee, addr);
                                }

                                // Remove from pending shadow polls
                                self.pending_shadow_polls.remove(&invitee);
                            }
                            Ok(None) => {
                                // Wrong key or not for us, continue
                            }
                            Err(e) => {
                                eprintln!("[Shadow] Decrypt error: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[Shadow] Failed to fetch shadows from {}: {:?}", invitee, e);
                }
            }
        }
    }

    /// Continuously punch all active targets (called every 500ms)
    pub(super) fn punch_active_targets(&mut self) {
        if self.active_punch_targets.is_empty() {
            return;
        }

        let now = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30);

        // Remove expired targets (older than 30 seconds)
        let expired: Vec<String> = self
            .active_punch_targets
            .iter()
            .filter(|(_, (_, start))| now.duration_since(*start) > timeout)
            .map(|(name, _)| name.clone())
            .collect();

        for name in expired {
            println!("[Punch] ⏰ Timeout for {}", name);
            self.active_punch_targets.remove(&name);
        }

        // Punch all remaining active targets
        let punch_targets: Vec<(String, Multiaddr, std::time::Instant)> = self
            .active_punch_targets
            .iter()
            .map(|(name, (addr, start))| (name.clone(), addr.clone(), *start))
            .collect();

        for (name, addr, start) in punch_targets {
            let attempt = (now.duration_since(start).as_millis() / 500) + 1;
            self.record_outgoing_dial(&addr, OutgoingDialSource::Punch);
            let _ = self.swarm.dial(addr.clone());
            // Only log every 10th attempt to reduce spam
            if attempt % 10 == 1 || attempt <= 3 {
                println!("[Punch] 📤 {}/60 to {}", attempt.min(60), name);
            }
        }
    }

    /// Add a target to active punch list
    pub(super) fn add_punch_target(&mut self, name: &str, addr: Multiaddr) {
        println!("[Punch] 🎯 Added target: {} -> {}", name, addr);
        self.active_punch_targets
            .insert(name.to_string(), (addr, std::time::Instant::now()));
    }

    /// Remove a target from active punch list (e.g., on connection success)
    pub(super) fn remove_punch_target(&mut self, name: &str) -> bool {
        if self.active_punch_targets.remove(name).is_some() {
            println!("[Punch] 🎉 {} connected, removed from targets", name);
            true
        } else {
            false
        }
    }
}
