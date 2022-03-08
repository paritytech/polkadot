# Bitfield Signing

Validators vote on the availability of a backed candidate by issuing signed bitfields, where each bit corresponds to a single candidate. These bitfields can be used to compactly determine which backed candidates are available or not based on a 2/3+ quorum.

## Protocol

Input:

There is no dedicated input mechanism for bitfield signing. Instead, Bitfield Signing produces a bitfield representing the current state of availability on `StartWork`.

Output:

- `BitfieldDistribution::DistributeBitfield`: distribute a locally signed bitfield
- `AvailabilityStore::QueryChunk(CandidateHash, validator_index, response_channel)`

## Functionality

Upon receipt of an `ActiveLeavesUpdate`, launch bitfield signing job for each `activated` head referring to a fresh leaf. Stop the job for each `deactivated` head.

## Bitfield Signing Job

Localized to a specific relay-parent `r`
If not running as a validator, do nothing.

- For each fresh leaf, begin by waiting a fixed period of time so availability distribution has the chance to make candidates available.
- Determine our validator index `i`, the set of backed candidates pending availability in `r`, and which bit of the bitfield each corresponds to.
- Start with an empty bitfield. For each bit in the bitfield, if there is a candidate pending availability, query the [Availability Store](../utility/availability-store.md) for whether we have the availability chunk for our validator index. The `OccupiedCore` struct contains the candidate hash so the full candidate does not need to be fetched from runtime.
- For all chunks we have, set the corresponding bit in the bitfield.
- Sign the bitfield and dispatch a `BitfieldDistribution::DistributeBitfield` message.
