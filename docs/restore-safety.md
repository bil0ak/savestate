# Restore safety and recovery

Every restore is a planned filesystem and database transaction. A preview alone never authorizes mutation: Savestate captures current state again under the exclusive store lock and compares it with the preview. If anything changed, the operation stops or presents a refreshed preview.

## Transaction contract

Savestate performs these stages in order:

1. Resolve and verify the checkpoint, filesystem roots, stable root identities, object hashes, database targets, and every journal-derived path.
2. Create and verify a recovery checkpoint covering exactly the state the restore may replace.
3. Materialize filesystem trees and database staging resources without changing the live targets.
4. Durably journal each preservation, swap, verification, and cleanup transition.
5. Commit staged resources into place.
6. Verify live filesystem metadata/content and configured databases.
7. Write the `complete` barrier.
8. Remove rollback material and finish idempotent cleanup.

Rollback sources are retained until the complete barrier. Before that barrier, `savestate recover --rollback` compensates to the recovery checkpoint and `savestate recover --resume` can safely retry the target. After the target has been verified and marked complete, rollback is intentionally refused; resume only finishes cleanup.

## Filesystem behavior

Ignored paths omitted from a checkpoint are preservation state, not deletions. Savestate detaches them at the smallest safe boundary, swaps selected content, and reattaches them through the durable journal. State created after planning is classified again from the detached tree. Explicitly selected content wins an exact conflict.

Every transaction path must be a validated, non-empty relative path. Symlink ancestors, traversal, duplicate journal entries, stale transaction directories, and paths outside the transaction root fail closed. New manifests bind roots to stable filesystem identities where the platform supports them; a deleted and recreated external root is refused.

Regular-file hardlinks are preserved only when the complete inode group lies inside one selected root. Metadata includes file type, mode, ownership, signed modification time, hardlink topology, and extended attributes. Unsupported special files fail capture.

## Crash behavior

Journal phases are runtime persisted state with validated transitions. A crash at any pre-complete transition leaves enough durable ownership information to roll back or resume idempotently. Ambiguous or unowned resources are never guessed: new mutations remain blocked until the operator resolves them.

Savestate provides resource-consistent, transactionally recoverable checkpoints. It does not claim a globally simultaneous snapshot across a filesystem and multiple databases. Stop unrelated writers before restore.
