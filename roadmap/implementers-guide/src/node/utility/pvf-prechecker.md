# PVF Pre-checker

The PVF pre-checker is a subsystem that is responsible for watching the relay chain for new PVFs that require pre-checking. Head over to [overview] for the PVF pre-checking process overview.

## Protocol

There is no dedicated input mechanism for PVF pre-checker. Instead, PVF pre-checker looks on the `ActiveLeavesUpdate` event stream for work.

This subsytem does not produce any output messages either. The subsystem will, however, send messages to the [Runtime API] subsystem to query for the pending PVFs and to submit votes. In addition to that, it will also communicate with [Candidate Validation] Subsystem to request PVF pre-check.

## Functionality

If the node is running in a collator mode, this subsystem will be disabled. The PVF pre-checker subsystem keeps track of the PVFs that are relevant for the subsystem. 

To be relevant for the subsystem, a PVF must be returned by the [`pvfs_require_precheck` runtime API][PVF pre-checking runtime API] in any of the active leaves. If the PVF is not present in any of the active leaves, it ceases to be relevant.

When a PVF just becomes relevant, the subsystem will send a message to the [Candidate Validation] subsystem asking for the pre-check.

Upon receving a message from the candidate-validation subsystem, the pre-checker will note down that the PVF has its judgement and will also sign and submit a [`PvfCheckStatement`][PvfCheckStatement] via the [`submit_pvf_check_statement` runtime API][PVF pre-checking runtime API]. In case, a judgement was received for a PVF that is no longer in view it is ignored.

Since a vote only is valid during [one session][overview], the subsystem will have to resign and submit the statements for the new session. The new session is assumed to be started if at least one of the leaves has a greater session index that was previously observed in any of the leaves.

The subsystem tracks all the statements that it submitted within a session. If for some reason a PVF became irrelevant and then becomes relevant again, the subsystem will not submit a new statement for that PVF within the same session.

If the node is not in the active validator set, it will still perform all the checks. However, it will only submit the check statements when the node is in the active validator set.

### Rejecting failed PVFs

It is possible that the candidate validation was not able to check the PVF, e.g. if it timed out. In that case, the PVF pre-checker will vote against it. This is considered safe, as there is no slashing for being on the wrong side of a pre-check vote.

Rejecting instead of abstaining is better in several ways:

1. Conclusion is reached faster - we have actual votes, instead of relying on a timeout.
1. Being strict in pre-checking makes it safer to be more lenient in preparation errors afterwards. Hence we have more leeway in avoiding raising dubious disputes, without making things less secure.

Also, if we only abstain, an attacker can specially craft a PVF wasm blob so that it will fail on e.g. 50% of the validators. In that case a supermajority will never be reached and the vote will repeat multiple times, most likely with the same result (since all votes are cleared on a session change). This is avoided by rejecting failed PVFs, and by only requiring 1/3 of validators to reject a PVF to reach a decision.

### Note on Disputes

Having a pre-checking phase allows us to make certain assumptions later when preparing the PVF for execution. If a runtime passed pre-checking, then we know that the runtime should be valid, and therefore any issue during preparation for execution can be assumed to be a local problem on the current node.

For this reason, even deterministic preparation errors should not trigger disputes. And since we do not dispute as a result of the pre-checking phase, as stated above, it should be impossible for preparation in general to result in disputes.

[overview]: ../../pvf-prechecking.md
[Runtime API]: runtime-api.md
[PVF pre-checking runtime API]: ../../runtime-api/pvf-prechecking.md
[Candidate Validation]: candidate-validation.md
[PvfCheckStatement]: ../../types/pvf-prechecking.md#pvfcheckstatement
