# Approvals subsystem

The approval subsystem determines whether a relay chain block can be considered for finality.  It does so by running validity checks on the candidates included in, aka declared available in, that relay chain block.  

These approval validity checks differ from the backing validity checks performed before starting availability:

- In backing, adversaries could select when they propose invalid candidates based upon when they control the parachain's backing validators who perform the checks.

- In approvals, we randomly assign individual validators to check specific candidates without giving adversaries' foreknowledge about either which honest validators get assigned to which candidates, or even how many check.  Availability prevents adversaries from choosing which validators obtain their possibly invalid candidate.

As such, approval checks provide significantly more security than backing checks, so Polkadot achieves some fixed security level most efficiently when we perform more approval checks per backing check or per relay chain block.  

...

Approval has roughly two parts:

- **Assignments** determines which validators performs approval checks on which candidates.  It ensures that each candidate receives enough random checkers, while reducing adversaries' odds for obtaining enough checkers, and limiting adversaries' foreknowledge.  It tracks approval votes to identify when "no show" approval check takes suspiciously long, perhaps indicating the node being under attack, and assigns more checks in this case.  It tracks relay chain equivocations to determine when adversaries possibly gained foreknowledge about assignments, and adds additional checks in this case.

- **Approval checks** listens to the assignments subsystem for outgoing assignment notices that we shall check specific candidates.  It then performs these checks by first invoking the reconstruction subsystem to obtain the candidate, second invoking the candidate validity utility subsystem upon the candidate, and finally sending out an approval vote, or perhaps initiating a dispute.

These both run first as off-chain consensus protocols using messages gossiped among all validators, and second as an on-chain record of this off-chain protocols' progress after the fact.  We need the on-chain protocol to provide rewards for the on-chain protocol, and doing an on-chain protocol simplify interaction with GRANDPA.  

Approval requires two gossiped message types, assignment notices created by its assignments subsystem, and approval votes sent by our approval checks subsystem when authorized by the candidate validity utility subsystem.  

...

Any validators resyncing the chain after falling behind should track approvals using only the on-chain protocol.  In particular, they should avoid sending their own assignment noticed and thus save themselves considerable validation work util they have a full synced chain.

### Approval keys

We need two separate keys for the approval subsystem:

- **Approval assignment keys** are sr25519/schnorrkel keys used only for the assignment criteria VRFs.  We implicitly sign assignment notices with approval assignment keys by including their relay chain context and additional data in the VRF's extra message, but exclude these from its VRF input.  

- **Approval vote keys** would only sign off on candidate parablock validity and has no natural key type restrictions.  We could reuse the ed25519 grandpa keys for this purpose since these signatures control access to grandpa, although distant future node configurations might favor separate roles.

Approval vote keys could relatively easily be handled by some hardened signer tooling, perhaps even HSMs assuming we select ed25519 for approval vote keys.  Approval assignment keys might or might not support hardened signer tooling, but doing so sounds far more complex.  In fact, assignment keys determine only VRF outputs that determine approval checker assignments, for which they can only act or not act, so they cannot equivocate, lie, etc. and represent little if any slashing risk for validator operators. 

In future, we shall determine which among the several hardening techniques best benefits the netwrok as a whole.  We could provide a multi-process multi-machine architecture for validators, perhaps even reminiscent of GNUNet, or perhaps more resembling smart HSM tooling.  We might instead design a system that more resembled full systems, like like Cosmos' sentry nodes.  In either case, approval assignments might be handled by a slightly hardened machine, but not necessarily nearly as hardened as approval votes, but approval votes machines must similarly run foreign WASM code, which increases their risk, so assignments being separate sounds helpful.  

### Gossip

Any validator could send their assignment notices and/or approval votes too early.  We gossip the approval votes because they represent a major commitment by the validator.  We delay gossiping the assignment notices unless their delay tranche exceeds our local clock excessively.

### Future work

We could consider additional gossip messages with which nodes claims "slow availability" and/or "slow candidate" to fine tune the assignments "no show" system, but long enough "no show" delays suffice probably.

We shall develop more practical experience with UDP once the availability system works using direct UDP connections.  In this, we should discover if reconstruction performs adequately with a complete graphs or  
benefits from topology restrictions.  At this point, an assignment notices could implicitly request pieces from a random 1/3rd, perhaps topology restricted, which saves one gossip round.  If this preliminary fast reconstruction fails, then nodes' request alternative pieces directly.  There is an interesting design space in how this overlaps with "slow availability" claims.
