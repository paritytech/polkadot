# tracing-gum

"gum" to make `tracing::{warn,info,..}` and `mick-jaeger` stick together, to be cross referenced in grafana with zero additional loc in the source code.

## Architecture Decision Record (ADR)

### Context

For the cross referencing in grafana to work, an explicit `traceID` of type `u128` has to be provided, both in the log line and the `jaeger::Span`.

Related:

* <https://github.com/paritytech/polkadot/issues/5045>

### Decision

The various options are to add ~3 lines per tracing line including a `candidate_hash` reference, to derive the `TraceIdentifier` from that.
This would have avoided the complexity introduced with this crate.
The visual overhead and friction and required diligence to keep this up is unreasonably high in the mid/long run especially given a certain growth of the company.

### Consequences

Minimal training is required to name `CandidateHash` as `candidate_hash` when providing to any of the log macros (`warn!`, `info!`, etc.).

The crate has to be used throughout the entire codebase to work consistently.
