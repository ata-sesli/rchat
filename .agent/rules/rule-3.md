---
trigger: model_decision
description: Cargo verification commands for this RChat workspace
---

When you need to run Cargo verification for this RChat workspace, especially `cargo test`, `cargo build`, or similar full Rust/Tauri checks, run it outside the sandbox. The sandboxed environment may ignore or fail to reuse the actual workspace dependency/build cache and can rebuild too much from scratch, wasting disk space and time.

Prefer the existing workspace setup and target cache. If sandboxing blocks Swift/Tauri/macOS build scripts, Cargo registry access, or shared target locks, request escalation instead of trying to force a fresh sandbox-local rebuild.
