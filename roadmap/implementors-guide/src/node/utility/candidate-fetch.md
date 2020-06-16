# Candidate Fetch

This is a utility subsystem responsible for fetching and reassembling parachain candidates from the erasure-coded chunks stored in validators across the network.

> TODO: how does it integrate with other subsystems to know from whom it can request chunks?

> TODO: is some kind of LRU cache a good idea?

## Protocol

Input:

- `GetCandidate(RelayParentHash, CandidateHash, Sender<Option<ParachainCandidate>>)`

## Functionality

On `GetCandidate`:

- Spawn a short-lived async Candidate Fetch Job responsible for assembling this candidate.

## Candidate Fetch Job

- determine what validators contain chunks for this candidate
- Send each a `ChunkRequest` (availability store?)
- on receiving sufficient chunks, reassemble the candidate and send it via the return channel
- on timeout or sufficient chunk request failures to prove that reassembly is impossible, return `None` via the return channel
