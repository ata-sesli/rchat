# Durable Group Record Protocol v3

Version 3 is a mandatory upgrade. Version 1 and version 2 group records,
invitations, and sync envelopes are rejected rather than interpreted with
legacy ordering rules.

## Identity and source of truth

The verified record set is the source of truth. Policy queries and SQLite
materialization consume the same pure state-machine result.

A group's UUID is derived from the founder peer ID and signed genesis-record
ID. A competing `GroupCreated` record therefore cannot replace the founder of
an existing group.

For any finite signed record set, delivery order must not change the derived
administrator, settings, active and invited members, dissolution state, name,
messages, receipts, file sources, or pending dependencies. Wall-clock
timestamps are display metadata only.

## Causal order

- Every record carries a positive Lamport counter below `u64::MAX`; the maximum
  value is reserved so the next frontier can always advance safely.
- `GroupCreated` is the only parentless record and has counter 1.
- Every later record names an authorization parent at the immediately
  preceding counter. Additional parents merge concurrent branches.
- An author has one canonical record per `(group, counter)` position.
- Records at one counter are ordered by `(author_peer_id, record_id)`.
- Authorization is derived only from the signed parent ancestry. Learning an
  unrelated concurrent record cannot retroactively reject an existing
  descendant.
- The deterministic projection replays causally authorized records in canonical
  order and treats transitions invalidated by a winning concurrent branch as
  accepted no-ops.
- The next frontier contains one immediately preceding primary parent plus up
  to 63 outstanding policy branches. Larger frontiers are merged over
  subsequent records without rejecting otherwise valid policy records.

Every record is first authorized against its causal branch. The shared
projection then applies all authorized records in canonical order. If an
earlier concurrent transition invalidates a later transition's precondition,
the later record is an accepted no-op. A record never gains authority from an
unrelated concurrent or future change.

## Pending and resource bounds

- Record size is limited to 1 MiB and direct parents to 64.
- Pending rows are limited to 1,024 per group and 64 per author per group.
- Missing dependencies remain pending; authorization failures are rejected.
- Reconciliation is iterative and rebuilds the materialized projection from
  one state-machine result inside one SQLite transaction.
- Events are emitted only after the transaction commits.

## Synchronization

Sync pages are ordered by the canonical record cursor, not timestamps. A page
contains at most 256 records. Explicit dependency requests are handled as a
separate phase (up to 64 IDs), then cursor pages continue without repeating
those out-of-order records. `has_more` and `next_cursor` continue repair until
the remote history is exhausted; an empty terminal response stops retrying an
unavailable dependency.
