# Availability Subsystems

The availability subsystems are responsible for ensuring that Proofs of Validity of backed candidates are widely available within the validator set, without requiring every node to retain a full copy. They accomplish this by broadly distributing erasure-coded chunks of the PoV, keeping track of which validator has which chunk by means of signed bitfields. They are also responsible for reassembling a complete PoV when required, e.g. when an approval checker needs to validate a parachain block.
