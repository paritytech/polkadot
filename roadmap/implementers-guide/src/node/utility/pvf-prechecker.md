# PVF Pre-checker

The PVF pre-checker is a subsystem that is responsible for watching the relay chain for new PVFs that require pre-checking. Head over to [overview] for the PVF pre-checking process overview.

## Protocol

There is no dedicated input mechanism for PVF pre-checker. Instead, PVF pre-checker looks on the `ActiveLeavesUpdate` event stream for work.

This subsytem does not produce any output messages either. The subsystem will, however, send messages to the [Runtime API] subsystem to query for the pending PVFs and to submit votes. In addition to that, it will also communicate with [Candidate Validation] Subsystem to request PVF pre-check.

## Functionality

If the node is running in a collator mode, this subsystem will be disabled. The PVF pre-checker subsystem keeps track of the PVFs that are relevant for the subsystem. 

To be relevant for the subsystem, a PVF must be returned by the [`pvfs_require_precheck` runtime API][PVF pre-checking runtime API] in any of the active leaves. If the PVF is not present in any of the active leaves, it ceases to be relevant.

When a PVF just becomes relevant, the subsystem will send a message to the [Candidate Validation] subsystem asking for the pre-check.

Upon receving a message from the candidate-validation subsystem, the pre-checker will note down that the PVF has its judgement and will also sign and submit a [`PvfCheckStatement`][PvfCheckStatement] via the [`submit_pvf_check_statement` runtime API][PVF pre-checking runtime API]. In case, a judgement was received for a PVF that is no longer in view it is ignored. It is possible that the candidate validation was not able to check the PVF. In that case, the PVF pre-checker will abstain and won't submit any check statements.

Since a vote only is valid during [one session][overview], the subsystem will have to resign and submit the statements for for the new session. The new session is assumed to be started if at least one of the leaves has a greater session index that was previously observed in any of the leaves.

The subsystem tracks all the statement that it submitted within a session. If for some reason a PVF became irrelevant and then becomes relevant again, the subsystem will not submit a new statement for that PVF within the same session.

If the node is not in the active validator set, it will still perform all the checks. However, it will only submit the check statements when the node is in the active validator set.

[overview]: ../../pvf-prechecking.md
[Runtime API]: runtime-api.md
[PVF pre-checking runtime API]: ../../runtime-api/pvf-prechecking.md
[Candidate Validation]: candidate-validation.md
[PvfCheckStatement]: ../../types/pvf-prechecking.md#pvfcheckstatement
