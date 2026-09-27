# Active QUIC public endpoint refresh

RChat sends STUN binding requests through the IPv4 UDP socket already owned by
Quinn/libp2p. The wrapper delegates normal QUIC I/O, segmentation and readiness
to Quinn. It intercepts only well-formed replies from the requested STUN server
with the matching random 96-bit transaction ID. Private/loopback/link-local
results are not accepted as public endpoints. There is one in-flight probe,
two server candidates, two-second DNS/probe deadlines, and a five-second retry
cooldown. Probes do not close/rebind the listener or interrupt its connections.

`NetworkState.public_endpoint.observation()` separates `last_known` from
`verified_at`; `fresh(now)` returns nothing after 30 seconds or invalidation.
Network-interface listener changes invalidate the observation, including any
probe already in flight. A five-second manager tick checks freshness, so route
or NAT changes that do not change interface addresses are eventually observed.
No promise of instantaneous gateway-change detection is made.

GUI/TUI invitation creation and GitHub shadow-invite redemption use the same
freshness-aware selector. Stale observations are refreshed before use. LAN and
direct public listener addresses remain fallback candidates; they are not
claimed to be STUN-verified. The manager defers up to 32 punch/reconnect
commands while a background refresh runs, updates libp2p's external address and
publishes discovery addresses before releasing them. On failure it removes the
old public advertisement and permits ordinary direct/LAN attempts. Existing
bounded punch attempts remain bounded; refreshing does not reset their budget.

Discovery publication runs off the network event loop with a 15-second deadline,
one active task, and one coalesced replacement snapshot. Only dependent punch
work waits for it; media ticks and session lifecycle commands continue. Session
registration remains ordered with archive freeze/commit, while its actual punch
is gated separately. Discovery and shadow writes serialize their complete local
Gist read/modify/write transactions and retain unexpired shadows. A failed Gist
read aborts the update instead of replacing existing data with an empty blob.

STUN observes the mapping toward its server, **not universal reachability**.
Destination-dependent NAT, blocked UDP, or two restrictive NATs may still defeat
direct connections. No relay is added. IPv6 continues using direct listener
addresses; this change does not add an IPv6 STUN probe.

Already copied temporary invitation links are immutable snapshots: regenerate
them if the inviter changes networks. An endpoint refresh cannot rewrite a link
already sent to another person.

## Automated verification

Endpoint tests run a local STUN fixture alongside a real libp2p QUIC listener.
They check the request's source port, changed mappings, wrong-source and
wrong-transaction replies, invalidation during a probe, refresh failure,
continued QUIC acceptance, expiry, and advertisement filtering. Manager tests
check refresh ordering and bounded pending work. These tests do not contact
public STUN servers or require two internet connections.

```sh
cargo nextest run --manifest-path src-tauri/Cargo.toml -p rchat-core -E 'test(endpoint)'
```

## Required manual two-network validation

Use two devices on different internet connections (for example home broadband
and a cellular hotspot), not merely two machines on one LAN. Start the GUI or
TUI on both, with GitHub discovery/punch assistance enabled.

1. Look for `[STUN] Verified active QUIC endpoint` in the logs. Confirm the port
   is the mapped UDP port, not an assumed local port. Establish a direct chat.
2. Keep the apps running, wait over 30 seconds, create/redeem an invitation, and
   confirm refresh precedes punching. Exchange messages and voice/media traffic.
3. Switch one device's internet connection without restarting RChat. Confirm
   the old observation is invalidated and discovery receives the new endpoint.
   Regenerate any copied temporary link, then reconnect and exchange messages.
4. Block STUN temporarily. Confirm no fresh public observation is claimed, the
   last-known value remains diagnostic-only, and LAN/direct-address connections
   still work. Unblock STUN and confirm recovery after the bounded retry delay.
5. Record the two network types, OS versions, refresh logs, and connection result.
   A direct-connect failure on destination-dependent NAT is a documented limit,
   not evidence that a STUN-observed port is universally reachable.

The implementation session verified automated tests only; this real two-network
test remains outstanding.
