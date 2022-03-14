# tracing-gum

"gum" to make `tracing::{warn,info,..}` and `mick-jaeger` stick together, to be cross referenced in grafana with zero additional loc in the source code.

## Architecture Decision Record (ADR)

### Context

For the cross referencing in grafana to work, an explicit `traceID` of type `u128` has to be provided, both in the log line and the `jaeger::Span`.
Before this PR, no such `traceID` annotations could be made easily accross the codebase.

Related issues:

* <https://github.com/paritytech/polkadot/issues/5045>

### Decision

The various options are to add ~2 lines per tracing line including a `candidate_hash` reference, to derive the `TraceIdentifier` from that.
This would have avoided the complexity introduced by adding another proc-macro to the codebase.
The visual overhead and friction and required diligence to keep the 100s of `tracing::{warn!,info!,debug!,..}` up is unreasonably high in the mid/long run especially with the context of more people joining the team.

### Consequences

Minimal training/impact is required to name `CandidateHash` as `candidate_hash` when providing to any of the log macros (`warn!`, `info!`, etc.).

The crate has to be used throughout the entire codebase to work consistently, to disambiguate, the prefix `gum::` is used.

Feature parity with `tracing::{warn!,..}` is not desired. We want consistency more than anything else. All currently used features _are_ supported with _gum_ as well.
