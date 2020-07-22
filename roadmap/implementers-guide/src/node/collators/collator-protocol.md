# Collator Protocol

The Collator Protocol implements the network protocol by which collators and validators communicate. It is used by collators to distribute collations to validators and used by validators to accept collations by collators.

Collator-to-Validator networking is more difficult than Validator-to-Validator networking because the set of possible collators for any given para is unbounded, unlike the validator set. Validator-to-Validator networking protocols can easily be implemented as gossip because the data can be bounded, and validators can authenticate each other by their `PeerId`s for the purposes of instantiating and accepting connections.

Since, at least at the level of the para abstraction, the collator-set for any given para is unbounded, validators need to make sure that they are receiving connections from capable and honest collators and that their bandwidth and time are not being wasted by attackers. Communicating across this trust-boundary is the most difficult part of this subsystem.

Validation of candidates is a heavy task, and furthermore, the [`PoV`][PoV] itself is a large piece of data. Empirically, `PoV`s are on the order of 10MB.

> TODO: note the incremental validation function Ximin proposes at https://github.com/paritytech/polkadot/issues/1348

As this network protocol serves as a bridge between collators and validators, it communicates primarily with one subsystem on behalf of each. As a collator, this will receive messages from the [`CollationGeneration`][CG] subsystem. As a validator, this will communicate with the [`CandidateBacking`][CB] subsystem.

## Protocol

Input: [`CollatorProtocolMessage`][CPM]

Output:
  - [`RuntimeApiMessage`][RAM]
  - [`CandidateBackingMessage`][CBM]`::Second`
  - [`NetworkBridgeMessage`][NBM]

## Functionality

[PoV]: ../../types/availability.md#proofofvalidity
[CPM]: ../../types/overseer-protocol.md#collatorprotocolmessage
[CG]: collation-generation.md
[CB]: ../backing/candidate-backing.md
[CBM]: ../../types/overseer-protocol.md#candidatebackingmesage
[RAM]: ../../types/overseer-protocol.md#runtimeapimessage
[NBM]: ../../types/overseer-protocol.md#networkbridgemessage

