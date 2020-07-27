# Backing Subsystems

The backing subsystems, when conceived as a black box, receive an arbitrary quantity of parablock candidates and associated proofs of validity from arbitrary untrusted collators. From these, they produce a bounded quantity of backable candidates which relay chain block authors may choose to include in a subsequent block.

In broad strokes, the flow operates like this:

- **Candidate Selection** winnows the field of parablock candidates, selecting up to one of them to second.
- **Candidate Backing** ensures that a seconding candidate is valid, then generates the appropriate `Statement`. It also keeps track of which candidates have received the backing of a quorum of other validators.
- **Statement Distribution** is the networking component which ensures that all validators receive each others' statements.
- **PoV Distribution** is the networking component which ensures that validators considering a candidate can get the appropriate PoV.
