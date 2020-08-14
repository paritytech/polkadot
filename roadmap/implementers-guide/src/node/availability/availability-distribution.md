# Availability Distribution

Distribute availability erasure-coded chunks to validators.

After a candidate is backed, the availability of the PoV block must be confirmed by 2/3+ of all validators. Validating a candidate successfully and contributing it to being backable leads to the PoV and erasure-coding being stored in the [Availability Store](../utility/availability-store.md).

## Protocol

`PeerSet`: `Validation`

Input:

- NetworkBridgeUpdateV1(update)

Output:

- NetworkBridge::SendValidationMessage(`[PeerId]`, message)
- NetworkBridge::ReportPeer(PeerId, cost_or_benefit)
- AvailabilityStore::QueryPoV(candidate_hash, response_channel)
- AvailabilityStore::StoreChunk(candidate_hash, chunk_index, inclusion_proof, chunk_data)

## Functionality

For each relay-parent in our local view update, look at all backed candidates pending availability. Distribute via gossip all erasure chunks for all candidates that we have to peers.

We define an operation `live_candidates(relay_heads) -> Set<CommittedCandidateReceipt>` which returns a set of [`CommittedCandidateReceipt`s](../../types/candidate.md#committed-candidate-receipt).
This is defined as all candidates pending availability in any of those relay-chain heads or any of their last `K` ancestors in the same session. We assume that state is not pruned within `K` blocks of the chain-head. `K` commonly is small and is currently fixed to `K=3`.

We will send any erasure-chunks that correspond to candidates in `live_candidates(peer_most_recent_view_update)`.
Likewise, we only accept and forward messages pertaining to a candidate in `live_candidates(current_heads)`.
Each erasure chunk should be accompanied by a merkle proof that it is committed to by the erasure trie root in the candidate receipt, and this gossip system is responsible for checking such proof.

We re-attempt to send anything live to a peer upon any view update from that peer.

On our view change, for all live candidates, we will check if we have the PoV by issuing a `QueryAvailabileData` message and waiting for the response. If the query returns `Some`, we will perform the erasure-coding and distribute all messages to peers that will accept them.

If we are operating as a validator, we note our index `i` in the validator set and keep the `i`th availability chunk for any live candidate, as we receive it. We keep the chunk and its merkle proof in the [Availability Store](../utility/availability-store.md) by sending a `StoreChunk` command. This includes chunks and proofs generated as the result of a successful `QueryPoV`.

The back-and-forth seems suboptimal at first glance, but drastically simplifies the pruning in the availability store, as it creates an invariant that chunks are only stored if the candidate was actually backed.
