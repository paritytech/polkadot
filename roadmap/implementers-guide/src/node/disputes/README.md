# Disputes Subsystems

If approval voting finds an invalid candidate, a dispute is raised. The disputes
subsystems are concerned with the following:

1. Disputes can be raised
2. Disputes (votes) get propagated to all other validators
3. Votes get recorded as necessary
3. Nodes will participate in disputes in a sensible fashion
4. Finality is stopped while a candidate is being disputed on chain
5. Chains can be reverted in case a dispute concludes invalid
6. Votes are provided to the provisioner for importing on chain, in order for
   slashing to work.

The dispute-coordinator subsystem interfaces with the provisioner and chain
selection to make the bulk of this possible. `dispute-distribution` is concerned
with getting votes out to other validators and receiving them in a spam
resilient way.
