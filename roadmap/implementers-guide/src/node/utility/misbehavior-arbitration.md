# Misbehavior Arbitration

The Misbehavior Arbitration subsystem collects reports of validator misbehavior, and slashes the stake of both misbehaving validator nodes and false accusers.

> TODO: It is not yet fully specified; that problem is postponed to a future PR.

One policy question we've decided even so: in the event that MA has to call all validators to check some block about which some validators disagree, the minority voters all get slashed, and the majority voters all get rewarded. Validators which abstain have a minor slash penalty, but probably not in the same order of magnitude as those who vote wrong.
