# Security policy

## Reporting a vulnerability

Report security and safety problems through the repository's public GitHub issue tracker.

Include the affected version, platform, configuration, reproduction steps, and whether live data was changed. Never post credentials, real snapshot stores, private source code, or sensitive database contents; create a minimal sanitized reproduction instead.

## Security model

Savestate checkpoints may contain ignored files, environment files, credentials, database rows, and other secrets. Stores are not encrypted. Unix stores are forced to owner-only permissions; Windows stores inherit directory ACLs. Back up or transmit a store only through a channel appropriate for its most sensitive captured content.

The store lock is process-local filesystem coordination. Network filesystems such as NFS and SMB may not propagate its guarantees between hosts. Shared or multi-host stores are unsupported and can corrupt catalogs or transactions.

PostgreSQL restore is destructive and experimental. Remote restore requires explicit configuration and confirmation, but tunnels and proxies can affect address classification. Verify the target independently. Do not point two Savestate projects at the same live database.

Savestate validates checkpoint objects, paths, root identities, journal ownership, and database OIDs before mutation. A validation refusal is a safety boundary; do not bypass it by manually editing manifests or journals. Preserve an interrupted transaction directory until recovery or forensic inspection is complete.

Supported releases are the current tagged release and the current default branch. Security fixes may require users to upgrade rather than backporting to older pre-1.0 versions.
