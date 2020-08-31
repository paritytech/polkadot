# Collation Generation

The collation generation subsystem is executed on collator nodes and produces candidates to be distributed to validators. If configured to produce collations for a para, it produces collations and then feeds them to the [Collator Protocol][CP] subsystem, which handles the networking.

## Protocol

Input: None

Output: CollationDistributionMessage

## Functionality

The process of generating a collation for a parachain is very parachain-specific. As such, the details of how to do so are left beyond the scope of this description. The subsystem should be implemented as an abstract wrapper, which is aware of this configuration:

```rust
struct CollationGenerationConfig {
	key: CollatorPair,
	collation_producer: Fn(params) -> async (HeadData, Vec<Vec<u8>>, PoV),
}
```

The configuration should be optional, to allow for the case where the node is not run with the capability to collate.

On `ActiveLeavesUpdate`:
  * If there is no collation generation config, ignore.
  * Otherwise, for each `activated` head in the update:
    * Determine if the para is scheduled or is next up on any occupied core by fetching the `availability_cores` Runtime API.
    * Determine an occupied core assumption to make about the para. The simplest thing to do is to always assume that if the para occupies a core, that the candidate will become available. Further on, this might be determined based on bitfields seen or validator requests.
    * Use the Runtime API subsystem to fetch the global validation data and local validation data.
	* Construct validation function params based on validation data.
	* Invoke the `collation_producer`.
	* Construct a `CommittedCandidateReceipt` using the outputs of the `collation_producer` and signing with the `key`.
	* Dispatch a [`CollatorProtocolMessage`][CPM]`::DistributeCollation(receipt, pov)`.

[CP]: collator-protocol.md
[CPM]: ../../types/overseer-protocol.md#collatorprotocolmessage
