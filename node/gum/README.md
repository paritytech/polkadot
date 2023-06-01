# tracing-gum

"gum" to make `tracing::{warn,info,..}` and `mick-jaeger` stick together, to be
cross referenced in grafana with zero additional loc in the source code.

## Architecture Decision Record (ADR)

### Context

For cross referencing spans and logs in grafana loki and tempo, a shared
`traceID` or `TraceIdentifier` is required. All logs must be annotated with such
meta information.

In most cases `CandidateHash` is the primary identifier of the `jaeger::Span`
and hence the source from which the `traceID` is derived. For cases where it is
_not_ the primary identifier, a helper tag named `traceID` is added to those
spans (out of scope, this is already present as a convenience measure).

Log lines on the other hand side, use `warn!,info!,debug!,trace!,..` API
provided by the `tracing` crate. Many of these, contain a `candidate_hash`,
which is _not_ equivalent to the `traceID` (256bits vs 128bits), and hence must
be derived.

To achieve the cross ref, either all instances of `candidate_hash` could be
added or this could be approached more systematically by providing a `macro` to
automatically do so.

Related issues:

* <https://github.com/paritytech/polkadot/issues/5045>

### Decision

Adding approx. 2 lines per tracing line including a `candidate_hash` reference,
to derive the `TraceIdentifier` from that, and printing that as part of the
key-value section in the `tracing::*` macros. The visual overhead and friction
and required diligence to keep the 100s of `tracing::{warn!,info!,debug!,..}` up
is unreasonably high in the mid/long run. This is especially true, in the
context of more people joining the team. Hence a proc-macro is introduced
which abstracts this away, and does so automagically at the cost of
one-more-proc-macro in the codebase.

### Consequences

Minimal training/impact is required to name `CandidateHash` as `candidate_hash`
when providing to any of the log macros (`warn!`, `info!`, etc.).

The crate has to be used throughout the entire codebase to work consistently, to
disambiguate, the prefix `gum::` is used.

Feature parity with `tracing::{warn!,..}` is not desired. We want consistency
more than anything. All currently used features _are_ supported with _gum_ as
well.
