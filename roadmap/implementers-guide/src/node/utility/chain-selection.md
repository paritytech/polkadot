# Chain Selection Subsystem

This subsystem implements the necessary metadata for the implementation of the [chain selection](../../protocol-chain-selection.md) portion of the protocol.

The subsystem wraps a database component which maintains a view of the unfinalized chain and records the properties of each block: whether the block is **viable**, whether it is **stagnant**, and whether it is **reverted**. It should also maintain an updated set of active leaves in accordance with this view, which should be cheap to query. Leaves are ordered descending first by weight and then by block number.

This subsystem needs to update its information on the unfinalized chain:
  * On every leaf-activated signal
  * On every block-finalized signal
  * On every `ChainSelectionMessage::Approve`
  * Periodically, to detect stagnation.

Simple implementations of these updates do `O(n_unfinalized_blocks)` disk operations. If the amount of unfinalized blocks is relatively small, the updates should not take very much time. However, in cases where there are hundreds or thousands of unfinalized blocks the naive implementations of these update algorithms would have to be replaced with more sophisticated versions.

### `OverseerSignal::ActiveLeavesUpdate`

Determine all new blocks implicitly referenced by any new active leaves and add them to the view. Update the set of viable leaves accordingly. The weights of imported blocks can be determined by the [`ChainApiMessage::BlockWeight`](../../types/overseer-protocol.md#chain-api-message).

### `OverseerSignal::BlockFinalized`

Delete data for all orphaned chains and update all metadata descending from the new finalized block accordingly, along with the set of viable leaves. Note that finalizing a **reverted** or **stagnant** block means that the descendants of those blocks may lose that status because the definitions of those properties don't include the finalized chain. Update the set of viable leaves accordingly.

### `ChainSelectionMessage::Approved`

Update the approval status of the referenced block. If the block was stagnant and thus non-viable and is now viable, then the metadata of all of its descendants needs to be updated as well, as they may no longer be stagnant either. Update the set of viable leaves accordingly.

### `ChainSelectionMessage::BestLeafContaining`

If the required block is unknown or not viable, then return `None`.
Iterate over all leaves, returning the first leaf containing the required block in its chain, and `None` otherwise.

### Periodically

Detect stagnant blocks and apply the stagnant definition to all descendants. Update the set of viable leaves accordingly.
