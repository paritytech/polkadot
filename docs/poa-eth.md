# PoA Ethereum High Level Documentation

NOTE: This is from the old README

### Ethereum Bridge Runtime Module
The main job of this runtime module is to keep track of useful information an Ethereum PoA chain
which has been submitted by a bridge relayer. This includes:

  - Ethereum headers and their status (e.g are they the best header, are they finalized, etc.)
  - Current validator set, and upcoming validator sets

This runtime module has more responsibilties than simply storing headers and validator sets. It is
able to perform checks on the incoming headers to verify their general integrity, as well as whether
or not they've been finalized by the authorities on the PoA chain.

This module is laid out as so:

```
├── ethereum
│  └── src
│     ├── error.rs        // Runtime error handling
│     ├── finality.rs     // Manage finality operations
│     ├── import.rs       // Import new Ethereum headers
│     ├── lib.rs          // Store headers and validator set info
│     ├── validators.rs   // Track current and future PoA validator sets
│     └── verification.rs // Verify validity of incoming Ethereum headers
```

### Currency Exchange Runtime Module
The currency exchange module is used to faciliate cross-chain funds transfers. It works by accepting
a transaction which proves that funds were locked on one chain, and releases a corresponding amount
of funds on the recieving chain.

For example: Alice would like to send funds from chain A to chain B. What she would do is send a
transaction to chain A indicating that she would like to send funds to an address on chain B. This
transaction would contain the amount of funds she would like to send, as well as the address of the
recipient on chain B. These funds would now be locked on chain A. Once the block containing this
"locked-funds" transaction is finalized it can be relayed to chain B. Chain B will verify that this
transaction was included in a finalized block on chain A, and if successful deposit funds into the
recipient account on chain B.

Chain B would need a way to convert from a foreign currency to its local currency. How this is done
is left to the runtime developer for chain B.

This module is one example of how an on-chain light client can be used to prove a particular action
was taken on a foreign chain. In particular it enables transfers of the foreign chain's native
currency, but more sophisticated modules such as ERC20 token transfers or arbitrary message transfers
are being worked on as well.

## Ethereum Node
On the Ethereum side of things, we require two things. First, a Solidity smart contract to track the
Substrate headers which have been submitted to the bridge (by the relay), and a built-in contract to
be able to verify that headers have been finalized by the GRANDPA finality gadget. Together this
allows the Ethereum PoA chain to verify the integrity and finality of incoming Substrate headers.

The Solidity smart contract is not part of this repo, but can be found
[here](https://github.com/svyatonik/substrate-bridge-sol/blob/master/substrate-bridge.sol) if you're
curious. We have the contract ABI in the `ethereum/relays/res` directory.

## Rialto Runtime
The node runtime consists of several runtime modules, however not all of them are used at the same
time. When running an Ethereum PoA to Substrate bridge the modules required are the Ethereum module
and the currency exchange module. When running a Substrate to Substrate bridge the Substrate and
currency exchange modules are required.

Below is a brief description of each of the runtime modules.

## Bridge Relay
The bridge relay is responsible for syncing the chains which are being bridged, and passing messages
between them. The current implementation of the relay supportings syncing and interacting with
Ethereum PoA and Substrate chains.
