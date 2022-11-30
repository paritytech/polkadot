# Glossary

Here you can find definitions of a bunch of jargon, usually specific to the Polkadot project.

- **Approval Checker:** A validator who randomly self-selects so to perform validity checks on a parablock which is pending approval.
- **BABE:** (Blind Assignment for Blockchain Extension). The algorithm validators use to safely extend the Relay Chain. See [the Polkadot wiki][0] for more information.
- **Backable Candidate:** A Parachain Candidate which is backed by a majority of validators assigned to a given parachain.
- **Backed Candidate:** A Backable Candidate noted in a relay-chain block
- **Backing:** A set of statements proving that a Parachain Candidate is backable.
- **Collator:** A node who generates Proofs-of-Validity (PoV) for blocks of a specific parachain.
- **DMP:** (Downward Message Passing). Message passing from the relay-chain to a parachain. Also there is a runtime parachains module with the same name.
- **DMQ:** (Downward Message Queue). A message queue for messages from the relay-chain down to a parachain. A parachain has
exactly one downward message queue.
- **Extrinsic:** An element of a relay-chain block which triggers a specific entry-point of a runtime module with given arguments.
- **GRANDPA:** (Ghost-based Recursive ANcestor Deriving Prefix Agreement). The algorithm validators use to guarantee finality of the Relay Chain.
- **HRMP:** (Horizontally Relay-routed Message Passing). A mechanism for message passing between parachains (hence horizontal) that leverages the relay-chain storage. Predates XCMP. Also there is a runtime parachains module with the same name.
- **Inclusion Pipeline:** The set of steps taken to carry a Parachain Candidate from authoring, to backing, to availability and full inclusion in an active fork of its parachain.
- **Module:** A component of the Runtime logic, encapsulating storage, routines, and entry-points.
- **Module Entry Point:** A recipient of new information presented to the Runtime. This may trigger routines.
- **Module Routine:** A piece of code executed within a module by block initialization, closing, or upon an entry point being triggered. This may execute computation, and read or write storage.
- **MQC:** (Message Queue Chain). A cryptographic data structure that resembles an append-only linked list which doesn't store original values but only their hashes. The whole structure is described by a single hash, referred as a "head". When a value is appended, it's contents hashed with the previous head creating a hash that becomes a new head.
- **Node:** A participant in the Polkadot network, who follows the protocols of communication and connection to other nodes. Nodes form a peer-to-peer network topology without a central authority.
- **Parachain Candidate, or Candidate:** A proposed block for inclusion into a parachain.
- **Parablock:** A block in a parachain.
- **Parachain:** A constituent chain secured by the Relay Chain's validators.
- **Parachain Validators:** A subset of validators assigned during a period of time to back candidates for a specific parachain
- **Parathread:** A parachain which is scheduled on a pay-as-you-go basis.
- **PDK (Parachain Development Kit):** A toolset that allows one to develop a parachain. Cumulus is a PDK.
- **Preimage:** In our context, if `H(X) = Y` where `H` is a hash function and `Y` is the hash, then `X` is the hash preimage.
- **Proof-of-Validity (PoV):** A stateless-client proof that a parachain candidate is valid, with respect to some validation function.
- **PVF:** Parachain Validation Function. The validation code that is run by validators on parachains or parathreads.
- **PVF Prechecking:** This is the process of initially checking the PVF when it is first added. We attempt preparation of the PVF and make sure it succeeds within a given timeout.
- **PVF Preparation:** This is the process of preparing the WASM blob and includes both prevalidation and compilation. As prevalidation is pretty minimal right now, preparation mostly consists of compilation.
- **Relay Parent:** A block in the relay chain, referred to in a context where work is being done in the context of the state at this block.
- **Runtime:** The relay-chain state machine.
- **Runtime Module:** See Module.
- **Runtime API:** A means for the node-side behavior to access structured information based on the state of a fork of the blockchain.
- **Subsystem:** A long-running task which is responsible for carrying out a particular category of work.
- **UMP:** (Upward Message Passing) A vertical message passing mechanism from a parachain to the relay chain.
- **Validator:** Specially-selected node in the network who is responsible for validating parachain blocks and issuing attestations about their validity.
- **Validation Function:** A piece of Wasm code that describes the state-transition function of a parachain.
- **VMP:** (Vertical Message Passing) A family of mechanisms that are responsible for message exchange between the relay chain and parachains.
- **XCMP:** (Cross-Chain Message Passing) A type of horizontal message passing (i.e. between parachains) that allows secure message passing directly between parachains and has minimal resource requirements from the relay chain, thus highly scalable.

## See Also

Also of use is the [Substrate Glossary](https://substrate.dev/docs/en/knowledgebase/getting-started/glossary).

[0]: https://wiki.polkadot.network/docs/learn-consensus
