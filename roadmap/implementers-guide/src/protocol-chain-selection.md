# Chain Selection

Chain selection processes in blockchains are used for the purpose of selecting blocks to build on and finalize. It is important for these processes to be consistent among nodes and resilient to a maximum proportion of malicious nodes which do not obey the chain selection process.

The parachain host uses both a block authoring system and a finality gadget. The chain selection strategy of the parachain host involves two key components: a _leaf-selection_ rule and a set of _finality constraints_. When it's a validator's turn to author on a block, they are expected to select the best block via the leaf-selection rule to build on top of. When a validator is participating in finality, there is a minimum block which can be voted on, which is usually the finalized block. The validator should select the best chain according to the leaf-selection rule and subsequently apply the finality constraints to arrive at the actual vote cast by that validator.

Before diving into the particularities of the leaf-selection rule and the finality constraints, it's important to discuss the goals that these components are meant to achieve. For this it is useful to create the definitions of _viable_ and _finalizable_ blocks.

### Property Definitions

A block is considered **viable** when all of the following hold:
  1. It is or descends from the finalized block
  2. It is not **stagnant**
  3. It is not **reverted**.

A block is considered a **viable leaf** when all of the following hold:
  1. It is **viable**
  2. It has no **viable** descendant.

A block is considered **stagnant** when either:
  1. It is unfinalized, is not approved, and has not been approved within 2 minutes
  2. Its parent is **stagnant**.

A block is considered **reverted** when either:
  1. It is unfinalized and includes a candidate which has lost a dispute
  2. Its parent is **reverted**

A block is considered **finalizable** when all of the following hold:
  1. It is **viable**
  2. Its parent, if unfinalized, is **finalizable**.
  3. It is either finalized or approved.
  4. It is either finalized or includes no candidates which have unresolved disputes or have lost a dispute.


### The leaf-selection rule

We assume that every block has an implicit weight or score which can be used to compare blocks. In BABE, this is determined by the number of primary slots included in the chain. In PoW, this is the chain with either the most work or GHOST weight.

The leaf-selection rule based on our definitions above is simple: we take the maximum-scoring viable leaf we are aware of. In the case of a tie we select the one with a lower lexicographical block hash.

### The best-chain-containing rule

Finality gadgets, as mentioned above, will often impose an additional requirement to vote on a chain containing a specific block, known as the **required** block. Although this is typically the most recently finalized block, it is possible that it may be a block that is unfinalized. When receiving such a request:
1. If the required block is the best finalized block, then select the best viable leaf.
2. If the required block is unfinalized and non-viable, then select the required block and go no further. This is likely an indication that something bad will be finalized in the network, which will never happen when approvals & disputes are functioning correctly. Nevertheless we account for the case here.
3. If the required block is unfinalized and viable, then iterate over the viable leaves in descending order by score and select the first one which contains the required block in its chain. Backwards iteration is a simple way to check this, but if unfinalized chains grow long then Merkle Mountain-Ranges will most likely be more efficient.

Once selecting a leaf, the chain should be constrained to the maximum of the required block or the highest **finalizable** ancestor.
