# RChat bad-pattern checks

Requires the `badbox` CLI on PATH (rules and fixtures verified with 0.3.0).
From the repository root:

```sh
bun run check:patterns
bun run test:patterns
```

The check covers RChat's core, TUI, native capture/audio processing, and Tauri
command source. It excludes vendored crates, historical notes, build output,
and the intentionally bad fixtures in this hidden directory. Inline Rust tests
are still scanned. No Rust build is required.

## Rules

- `rchat/unbounded-queue`: reports `unbounded` / `unbounded_channel` constructors,
  including turbofish type arguments. Review whether slow playback, encoding,
  or network consumers can accumulate frames indefinitely. Prefer bounded queues
  with an explicit dropping/backpressure policy where appropriate; do not block
  a real-time audio callback to silence the finding.
- `rchat/std-unbounded-channel`: reports zero-argument `std::sync::mpsc::channel`
  and `mpsc::channel`, including turbofish arguments. The no-argument restriction
  excludes bounded Tokio/futures channel constructors; `sync_channel`, bare
  `channel`, and unrelated qualified channel APIs are not matched. Import aliases
  are not resolved, so renamed standard-library imports can escape detection.
- `rchat/blocking-in-awaiting-callable`: reports `std::thread::sleep` or
  `thread::sleep` in a callable that also contains an await. Use async timers
  for async work. Dedicated synchronous workers and `spawn_blocking` closures
  without awaits are accepted by the fixtures.
- `rchat/unfinished-code`: reports `todo!` and `unimplemented!` macros in
  callables. Unsupported chat/media operations should return actionable errors,
  not crash at runtime. Strings and comments are not matches.

## Limits and interpretation

These are syntax-based review signals, not proof of bugs. Badbox does not resolve
types or imports: renamed imports may escape detection, and a custom function
with a matching constructor name may be reported. The sleep rule requires an
await site, not merely an `async` declaration; it does not cover every blocking
API or prove which runtime executes the callable. Macro expansion and conditional
compilation are not evaluated. This pack does not check Svelte or prove group
authorization, transaction safety, or synchronization correctness.

Badbox exits successfully when it finds patterns; errors/diagnostics fail the
command. Therefore `check:patterns` is advisory, not a zero-findings CI gate.
The fixture runner explicitly checks expected findings and rejects diagnostics,
because Badbox 0.3.0 has no CLI runner for its DSL test declarations.

Initial scan at commit `85c1ca7`: 110 files, four unbounded-queue findings in
`live/voice/manager.rs`, `live/voice/voice.rs` (two callables), and
`network/voice_stream.rs`. These are not suppressions or an allowed-count budget;
review future output on its merits.
