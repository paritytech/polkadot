# Candidate Selection

The Candidate Selection Subsystem is run by validators, and is responsible for interfacing with Collators to select a candidate, along with its PoV, to second during the backing process relative to a specific relay parent.

This subsystem includes networking code for communicating with collators, and tracks which collations specific collators have submitted. This subsystem is responsible for disconnecting and blacklisting collators who have submitted collations that are found to have submitted invalid collations by other subsystems.

This subsystem is only ever interested in parablocks assigned to the particular parachain which this validator is currently handling.

New parablock candidates may arrive from a potentially unbounded set of collators. This subsystem chooses either 0 or 1 of them per relay parent to second. If it chooses to second a candidate, it sends an appropriate message to the [Candidate Backing subsystem](/node/backing/candidate-backing.html) to generate an appropriate [`Statement`](/type-definitions.html#statement-type).

In the event that a parablock candidate proves invalid, this subsystem will receive a message back from the Candidate Backing subsystem indicating so. If that parablock candidate originated from a collator, this subsystem will blacklist that collator. If that parablock candidate originated from a peer, this subsystem generates a report for the [Misbehavior Arbitration subsystem](/node/utility/misbehavior-arbitration.html).

## Protocol

Input: None

Output:

- Validation requests to Validation subsystem
- [`CandidateBackingMessage`](/type-definitions.html#candidate-backing-message)`::Second`
- Peer set manager: report peers (collators who have misbehaved)

## Functionality

Overarching network protocol + job for every relay-parent

> TODO The Candidate Selection network protocol is currently intentionally unspecified pending further discussion.

Several approaches have been selected, but all have some issues:

- The most straightforward approach is for this subsystem to simply second the first valid parablock candidate which it sees per relay head. However, that protocol is vulnerable to a single collator which, as an attack or simply through chance, gets its block candidate to the node more often than its fair share of the time.
- It may be possible to do some BABE-like selection algorithm to choose an "Official" collator for the round, but that is tricky because the collator which produces the PoV does not necessarily actually produce the block.
- We could use relay-chain BABE randomness to generate some delay `D` on the order of 1 second, +- 1 second. The collator would then second the first valid parablock which arrives after `D`, or in case none has arrived by `2*D`, the last valid parablock which has arrived. This makes it very hard for a collator to game the system to always get its block nominated, but it reduces the maximum throughput of the system by introducing delay into an already tight schedule.
- A variation of that scheme would be to randomly choose a number `I`, and have a fixed acceptance window `D` for parablock candidates. At the end of the period `D`, count `C`: the number of parablock candidates received. Second the one with index `I % C`. Its drawback is the same: it must wait the full `D` period before seconding any of its received candidates, reducing throughput.

## Candidate Selection Job

- Aware of validator key and assignment
- One job for each relay-parent, which selects up to one collation for the Candidate Backing Subsystem
