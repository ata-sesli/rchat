# RChat socket-runtime hook

Source: crates.io `libp2p-quic` 0.13.0, MIT license (copyright and license notices
are retained in the source headers). Original crate checksum:
`8dc448b2de9f4745784e3751fe8bc6c473d01b8317edd5ababcb0dec803d843f`.

Only `src/config.rs` and `src/transport.rs` differ from that published source:
`Config::runtime` optionally supplies a Quinn runtime; endpoint construction
uses it when present and otherwise retains the original provider behavior.
RChat wraps Quinn's Tokio UDP socket to demultiplex its own STUN replies. No
second UDP bind, QUIC parser, TLS changes, or connection migration is introduced.

The upstream public Provider enum hard-codes the Quinn runtime, so a custom
Provider alone cannot supply this socket wrapper. Remove this patch when an
upstream transport provides an equivalent runtime/socket hook. Upgrade the
vendored source along with libp2p; do not leave it pinned independently during
security/dependency updates. Upstream tests are retained.
