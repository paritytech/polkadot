# Whence Parachains

Parachains are the solution to a problem. As with any solution, it cannot be understood without first understanding the problem. So let's start by going over the issues faced by blockchain technology that led to us beginning to explore the design space for something like parachains.

## Issue 1: Scalability

It became clear a few years ago that the transaction throughput of simple Proof-of-Work (PoW) blockchains such as Bitcoin, Ethereum, and myriad others was simply too low.

> TODO: what if there were more blockchains, etc.

Proof-of-Stake (PoS) systems can accomplish higher throughput than PoW blockchains. PoS systems are secured by bonded capital as opposed to spent effort - liquidity opportunity cost vs. burning electricity. The way they work is by selecting a set of validators with known economic identity who lock up tokens in exchange for earning the right to "validate" or participate in the consensus process. If they are found to carry out that process wrongly, they will be slashed, meaning some or all of the locked tokens will be burned. This provides a strong disincentive in the direction of misbehavior.

Since the consensus protocol doesn't revolve around wasting effort, block times and agreement can occur much faster. Solutions to PoW challenges don't have to be found before a block can be authored, so the overhead of authoring a block is reduced to only the costs of creating and distributing the block.

However, consensus on a PoS chain requires full agreement of 2/3+ of the validator set for everything that occurs at Layer 1: all logic which is carried out as part of the blockchain's state machine. This means that everybody still needs to check everything. Furthermore, validators may have different views of the system based on the information that they receive over an asynchronous network, making agreement on the latest state more difficult.

Parachains are an example of a **sharded** protocol. Sharding is a concept borrowed from traditional database architecture. Rather than requiring every participant to check every transaction, we require each participant to check some subset of transactions, with enough redundancy baked in that byzantine (arbitrarily malicious) participants can't sneak in invalid transactions - at least not without being detected and getting slashed, with those transactions reverted.

Sharding and Proof-of-Stake in coordination with each other allow a parachain host to provide full security on many parachains, even without all participants checking all state transitions.

> TODO: note about network effects & bridging

## Issue 2: Flexibility / Specialization

"dumb" VMs don't give you the flexibility. Any engineer knows that being able to specialize on a problem gives them and their users more _leverage_.

> TODO: expand on leverage

Having recognized these issues, we set out to find a solution to these problems, which could allow developers to create and deploy purpose-built blockchains unified under a common source of security, with the capability of message-passing between them; a _heterogeneous sharding solution_, which we have come to know as **Parachains**.
