# Provisioner

This subsystem is responsible for providing data to an external block authorship service beyond the scope of the [Overseer](/node/overseer.html) so that the block authorship service can author blocks containing data produced by various subsystems.

In particular, the data to provide:

- backable candidates and their backings
- signed bitfields
- misbehavior reports
- dispute inherent
    > TODO: needs fleshing out in validity module, related to blacklisting

## Protocol

Input:

- Bitfield(relay_parent, signed_bitfield)
- BackableCandidate(relay_parent, candidate_receipt, backing)
- RequestBlockAuthorshipData(relay_parent, response_channel)

## Functionality

Use `StartWork` and `StopWork` to manage a set of jobs for relay-parents we might be building upon.
Forward all messages to corresponding job, if any.

## Block Authorship Provisioning Job

Track all signed bitfields, all backable candidates received. Provide them to the `RequestBlockAuthorshipData` requester via the `response_channel`. If more than one backable candidate exists for a given `Para`, provide the first one received.

> TODO: better candidate-choice rules.
