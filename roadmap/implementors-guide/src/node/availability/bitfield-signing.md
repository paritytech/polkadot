# Bitfield Signing

Validators vote on the availability of a backed candidate by issuing signed bitfields, where each bit corresponds to a single candidate. These bitfields can be used to compactly determine which backed candidates are available or not based on a 2/3+ quorum.

## Protocol

Output:

- BitfieldDistribution::DistributeBitfield: distribute a locally signed bitfield
- AvailabilityStore::QueryChunk(CandidateHash, validator_index, response_channel)

## Functionality

Upon onset of a new relay-chain head with `StartWork`, launch bitfield signing job for the head. Stop the job on `StopWork`.

## Bitfield Signing Job

Localized to a specific relay-parent `r`
If not running as a validator, do nothing.

- Determine our validator index `i`, the set of backed candidates pending availability in `r`, and which bit of the bitfield each corresponds to.
- > TODO: wait T time for availability distribution?
- Start with an empty bitfield. For each bit in the bitfield, if there is a candidate pending availability, query the [Availability Store](../utility/availability-store.md) for whether we have the availability chunk for our validator index.
- For all chunks we have, set the corresponding bit in the bitfield.
- Sign the bitfield and dispatch a `BitfieldDistribution::DistributeBitfield` message.
