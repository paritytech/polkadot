# Validator Groups

Yields the validator groups used during the current session. The validators in the groups are referred to by their index into the validator-set.

```rust
/// A helper data-type for tracking validator-group rotations.
struct GroupRotationInfo {
    session_start_block: BlockNumber,
    group_rotation_frequency: BlockNumber,
    now: BlockNumber,
}

impl GroupRotationInfo {
    /// Returns the index of the group needed to validate the core at the given index,
    /// assuming the given amount of cores/groups.
    fn group_for_core(&self, core_index, cores) -> GroupIndex;

    /// Returns the block number of the next rotation after the current block. If the current block
    /// is 10 and the rotation frequency is 5, this should return 15.
    ///
    /// If the group rotation frequency is 0, returns 0.
    fn next_rotation_at(&self) -> BlockNumber;

    /// Returns the block number of the last rotation before or including the current block. If the
    /// current block is 10 and the rotation frequency is 5, this should return 10.
    ///
    /// If the group rotation frequency is 0, returns 0.
    fn last_rotation_at(&self) -> BlockNumber;
}

/// Returns the validator groups and rotation info localized based on the block whose state
/// this is invoked on. Note that `now` in the `GroupRotationInfo` should be the successor of
/// the number of the block.
fn validator_groups(at: Block) -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo);
```
