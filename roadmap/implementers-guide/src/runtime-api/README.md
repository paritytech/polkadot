# Runtime APIs

Runtime APIs are the means by which the node-side code extracts information from the state of the runtime.

Every block in the relay-chain contains a *state root* which is the root hash of a state trie encapsulating all storage of runtime modules after execution of the block. This is a cryptographic commitment to a unique state. We use the terminology of accessing the *state at* a block to refer accessing the state referred to by the state root of that block.

Although Runtime APIs are often used for simple storage access, they are actually empowered to do arbitrary computation. The implementation of the Runtime APIs lives within the Runtime as Wasm code and exposes `extern` functions that can be invoked with arguments and have a return value. Runtime APIs have access to a variety of host functions, which are contextual functions provided by the Wasm execution context, that allow it to carry out many different types of behaviors.

Abilities provided by host functions includes:

* State Access
* Offchain-DB Access
* Submitting transactions to the transaction queue
* Optimized versions of cryptographic functions
* More

So it is clear that Runtime APIs are a versatile and powerful tool to leverage the state of the chain. In general, we will use Runtime APIs for these purposes:

* Access of a storage item
* Access of a bundle of related storage items
* Deriving a value from storage based on arguments
* Submitting misbehavior reports

More broadly, we have the goal of using Runtime APIs to write Node-side code that fulfills the requirements set by the Runtime. In particular, the constraints set forth by the [Scheduler](../runtime/scheduler.md) and [Inclusion](../runtime/inclusion.md) modules. These modules are responsible for advancing paras with a two-phase protocol where validators are first chosen to validate and back a candidate and then required to ensure availability of referenced data. In the second phase, validators are meant to attest to those para-candidates that they have their availability chunk for. As the Node-side code needs to generate the inputs into these two phases, the runtime API needs to transmit information from the runtime that is aware of the Availability Cores model instantiated by the Scheduler and Inclusion modules.

Node-side code is also responsible for detecting and reporting misbehavior performed by other validators, and the set of Runtime APIs needs to provide methods for observing live disputes and submitting reports as transactions.

The next sections will contain information on specific runtime APIs. The format is this:

```rust
/// Fetch the value of the runtime API at the block.
///
/// Definitionally, the `at` parameter cannot be any block that is not in the chain.
/// Thus the return value is unconditional. However, for in-practice implementations
/// it may be possible to provide an `at` parameter as a hash, which may not refer to a
/// valid block or one which implements the runtime API. In those cases it would be
/// best for the implementation to return an error indicating the failure mode.
fn some_runtime_api(at: Block, arg1: Type1, arg2: Type2, ...) -> ReturnValue;
```

Certain runtime APIs concerning the state of a para require the caller to provide an `OccupiedCoreAssumption`. This indicates how the result of the runtime API should be computed if there is a candidate from the para occupying an availability core in the [Inclusion Module](../runtime/inclusion.md).

The choices of assumption are whether the candidate occupying that core should be assumed to have been made available and included or timed out and discarded, along with a third option to assert that the core was not occupied. This choice affects everything from the parent head-data, the validation code, and the state of message-queues. Typically, users will take the assumption that either the core was free or that the occupying candidate was included, as timeouts are expected only in adversarial circumstances and even so, only in a small minority of blocks directly following validator set rotations.

```rust
/// An assumption being made about the state of an occupied core.
enum OccupiedCoreAssumption {
    /// The candidate occupying the core was made available and included to free the core.
    Included,
    /// The candidate occupying the core timed out and freed the core without advancing the para.
    TimedOut,
    /// The core was not occupied to begin with.
    Free,
}
```