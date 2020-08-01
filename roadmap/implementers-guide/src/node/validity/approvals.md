# Approval subsystem

The approval subsystem determines whether a relay chain block can be considered for finality.  It does so by running validity checks on the candidates declared available in that relay chain block.  

These approval validity checks differ from the backing validity checks performed before starting availability:

- In backing, adversaries could select when they propose invalid candidates based upon when they control the parachain's backing validators who perform the checks.

- In approvals, we randomly assigns individual validators to check specific candidates without giving adversaries foreknowledge about either which honest validators get assigned to which candidates, or even how many check.  Availability prevents adversaries from choosing which validators obtain their possibly invalid candidate.

As such, approval checks provide significantly more security than backing checks, so polkadot achieves some fixed security level most efficiently when we perform more approval checks per backing check or per relay chain block.  

...

Approval requires two gossiped message types, assignment notices created by its assignments subsystem, and approval votes sent by our approval checks subsystem when authorized by the candidate validity utility subsystem.  

Approval has roughly two parts:

- **Assignments** ensures that each candidates receives enough random checkers, while reducing adversaries odds for obtaining enough checkers, and limiting adversaries foreknowledge.  It tracks approval votes to identify "no show" approval check takes suspiciously long, perhaps indicating the node being under attack, and assigns more checks in this case.  It tracks relay chain equivocations to determine when adversaries possibly gained foreknowledge about assignments, and adds additional checks in this case.

- **Approval checks** listens to the assignments subsystem for outgoing assignment notices that we shall check specific candidates.  It then performs these checks by first invoking the reconstruction subsystem to obtain the candidate, second invoking the candidate validity utility subsystem upon the candidate, and finally sending out an approval vote, or perhaps initiating a dispute.

There are rewards computations on-chain too, which runs the assignments code to determine approval, but does so after the fact.

...

### Gossip

Any validator could send their assignment notices and/or approval votes too early.  We gossip the approval votes because they represent a major commitment by the validator.  We delay gossiping the assignment notices until they agree with our local clock.

### Future work

We could consider additional gossip messages with which nodes claims "slow availability" and/or "slow candidate" to fine tune the assignments "no show" system, but long enough "no show" delays suffice probably.

We shall develop more practical experience with UDP once the availability system works using direct UDP connections.  In this, we should discover if reconstruction performs adequately with a complete graphs or  
benefits from topology restrictions.  At this point, an assignment notices could implicitly request pieces from a random 1/3rd, perhaps topology restricted, which saves one gossip round.  If this preliminary fast reconstruction fails, then nodes' request alternative pieces directly.  There is an interesting design space in how this overlaps with "slow availability" claims.

