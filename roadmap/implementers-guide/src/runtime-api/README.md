# Runtime APIs

Runtime APIs are the means by which the node-side code extracts information from the state of the runtime.

Every block in the relay-chain contains a *state root* which is the root hash of a state trie encapsulating all storage of runtime modules after execution of the block. This is a cryptographic commitment to a unique state. We use the terminology of accessing the *state at* a block to refer accessing the state referred to by the state root of that block.

Although Runtime APIs are often used for simple storage access, they are actually empowered to do arbitrary computation. The implementation of the Runtime APIs lives within the Runtime as Wasm code and exposes extern functions that can be invoked with arguments and have a return value. Runtime APIs have access to a variety of host functions, which are contextual functions provided by the Wasm execution context, that allow it to carry out many different types of behaviors.

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

The next sections will contain information on specific runtime APIs.
