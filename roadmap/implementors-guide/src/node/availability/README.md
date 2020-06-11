# Availability Module

The availability module is responsible for ensuring that Proofs of Validity of backed candidates are widely available within the validator set, without requiring every node to retain a full copy. It accomplishes this by broadly distributing erasure-coded chunks of the PoV, keeping track of which validator has which chunk by means of signed bitfields. It is also responsible for reassembling a complete PoV when required, e.g. when a fisherman reports a potentially invalid block.
