# Glossary

Here you can find definitions of a bunch of jargon, usually specific to the Polkadot project.

- BABE: (Blind Assignment for Blockchain Extension). The algorithm validators use to safely extend the Relay Chain. See [the Polkadot wiki][0] for more information.
- Backable Candidate: A Parachain Candidate which is backed by a majority of validators assigned to a given parachain.
- Backed Candidate: A Backable Candidate noted in a relay-chain block
- Backing: A set of statements proving that a Parachain Candidate is backable.
- Collator: A node who generates Proofs-of-Validity (PoV) for blocks of a specific parachain.
- Extrinsic: An element of a relay-chain block which triggers a specific entry-point of a runtime module with given arguments.
- GRANDPA: (Ghost-based Recursive ANcestor Deriving Prefix Agreement). The algorithm validators use to guarantee finality of the Relay Chain.
- Inclusion Pipeline: The set of steps taken to carry a Parachain Candidate from authoring, to backing, to availability and full inclusion in an active fork of its parachain.
- Module: A component of the Runtime logic, encapsulating storage, routines, and entry-points.
- Module Entry Point: A recipient of new information presented to the Runtime. This may trigger routines.
- Module Routine: A piece of code executed within a module by block initialization, closing, or upon an entry point being triggered. This may execute computation, and read or write storage.
- Node: A participant in the Polkadot network, who follows the protocols of communication and connection to other nodes. Nodes form a peer-to-peer network topology without a central authority.
- Parachain Candidate, or Candidate: A proposed block for inclusion into a parachain.
- Parablock: A block in a parachain.
- Parachain: A constituent chain secured by the Relay Chain's validators.
- Parachain Validators: A subset of validators assigned during a period of time to back candidates for a specific parachain
- Parathread: A parachain which is scheduled on a pay-as-you-go basis.
- Proof-of-Validity (PoV): A stateless-client proof that a parachain candidate is valid, with respect to some validation function.
- Relay Parent: A block in the relay chain, referred to in a context where work is being done in the context of the state at this block.
- Runtime: The relay-chain state machine.
- Runtime Module: See Module.
- Runtime API: A means for the node-side behavior to access structured information based on the state of a fork of the blockchain.
- Secondary Checker: A validator who has been randomly selected to perform secondary approval checks on a parablock which is pending approval.
- Subsystem: A long-running task which is responsible for carrying out a particular category of work.
- Validator: Specially-selected node in the network who is responsible for validating parachain blocks and issuing attestations about their validity.
- Validation Function: A piece of Wasm code that describes the state-transition function of a parachain.

Also of use is the [Substrate Glossary](https://substrate.dev/docs/en/overview/glossary).

[0]: https://wiki.polkadot.network/docs/en/learn-consensus
