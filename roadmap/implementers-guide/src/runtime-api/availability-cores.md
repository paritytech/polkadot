# Availability Cores

Yields information on all availability cores. Cores are either free or occupied. Free cores can have paras assigned to them. Occupied cores don't, but they can become available part-way through a block due to bitfields and then have something scheduled on them. To allow optimistic validation of candidates, the occupied cores are accompanied by information on what is upcoming. This information can be leveraged when validators perceive that there is a high likelihood of a core becoming available based on bitfields seen, and then optimistically validate something that would become scheduled based on that, although there is no guarantee on what the block producer will actually include in the block. 

See also the [Scheduler Module](../runtime/scheduler.md) for a high-level description of what an availability core is and why it exists.

```rust
fn availability_cores(at: Block) -> Vec<CoreState>;
```

This is all the information that a validator needs about scheduling for the current block. It includes all information on [Scheduler](../runtime/scheduler.md) core-assignments and [Inclusion](../runtime/inclusion.md) state of blocks occupying availability cores. It includes data necessary to determine not only which paras are assigned now, but which cores are likely to become freed after processing bitfields, and exactly which bitfields would be necessary to make them so.  The implementation of this runtime API should invoke `Scheduler::clear` and `Scheduler::schedule(Vec::new(), current_block_number + 1)` to ensure that scheduling is accurate.

```rust
struct OccupiedCore {
    // NOTE: this has no ParaId as it can be deduced from the candidate descriptor.
    /// If this core is freed by availability, this is the assignment that is next up on this
    /// core, if any. None if there is nothing queued for this core.
    next_up_on_available: Option<ScheduledCore>,
    /// The relay-chain block number this began occupying the core at.
    occupied_since: BlockNumber,
    /// The relay-chain block this will time-out at, if any.
    time_out_at: BlockNumber,
    /// If this core is freed by being timed-out, this is the assignment that is next up on this
    /// core. None if there is nothing queued for this core or there is no possibility of timing
    /// out.
    next_up_on_time_out: Option<ScheduledCore>,
    /// A bitfield with 1 bit for each validator in the set. `1` bits mean that the corresponding
    /// validators has attested to availability on-chain. A 2/3+ majority of `1` bits means that
    /// this will be available.
    availability: Bitfield,
    /// The group assigned to distribute availability pieces of this candidate.
    group_responsible: GroupIndex,
    /// The hash of the candidate occupying the core.
    candidate_hash: CandidateHash,
    /// The descriptor of the candidate occupying the core.
    candidate_descriptor: CandidateDescriptor,
}

struct ScheduledCore {
    /// The ID of a para scheduled.
    para_id: ParaId,
    /// The collator required to author the block, if any.
    collator: Option<CollatorId>,
}

enum CoreState {
    /// The core is currently occupied.
    Occupied(OccupiedCore),
    /// The core is currently free, with a para scheduled and given the opportunity
    /// to occupy.
    ///
    /// If a particular Collator is required to author this block, that is also present in this
    /// variant.
    Scheduled(ScheduledCore),
    /// The core is currently free and there is nothing scheduled. This can be the case for parathread
    /// cores when there are no parathread blocks queued. Parachain cores will never be left idle.
    Free,
}
```
