# Candidate Validation

This subsystem is responsible for handling candidate validation requests. It is a simple request/response server.

A variety of subsystems want to know if a parachain block candidate is valid. None of them care about the detailed mechanics of how a candidate gets validated, just the results. This subsystem handles those details.

## Protocol

Input: [`CandidateValidationMessage`](../../types/overseer-protocol.md#validation-request-type)

Output: Validation result via the provided response side-channel.

## Functionality

This subsystem groups the requests it handles in two categories:  *candidate validation* and *PVF pre-checking*. 

The first category can be further subdivided in two request types: one which draws out validation data from the state, and another which accepts all validation data exhaustively. Validation returns three possible outcomes on the response channel: the candidate is valid, the candidate is invalid, or an internal error occurred. 

Parachain candidates are validated against their validation function: A piece of Wasm code that describes the state-transition of the parachain. Validation function execution is not metered. This means that an execution which is an infinite loop or simply takes too long must be forcibly exited by some other means. For this reason, we recommend dispatching candidate validation to be done on subprocesses which can be killed if they time-out.

Upon receiving a validation request, the first thing the candidate validation subsystem should do is make sure it has all the necessary parameters to the validation function. These are:
  * The Validation Function itself.
  * The [`CandidateDescriptor`](../../types/candidate.md#candidatedescriptor).
  * The [`ValidationData`](../../types/candidate.md#validationdata).
  * The [`PoV`](../../types/availability.md#proofofvalidity).
  
The second category is for PVF pre-checking. This is primarly used by the [PVF pre-checker](pvf-prechecker.md) subsystem.

### Determining Parameters

For a [`CandidateValidationMessage`][CVM]`::ValidateFromExhaustive`, these parameters are exhaustively provided.

For a [`CandidateValidationMessage`][CVM]`::ValidateFromChainState`, some more work needs to be done. Due to the uncertainty of Availability Cores (implemented in the [`Scheduler`](../../runtime/scheduler.md) module of the runtime), a candidate at a particular relay-parent and for a particular para may have two different valid validation-data to be executed under depending on what is assumed to happen if the para is occupying a core at the onset of the new block. This is encoded as an `OccupiedCoreAssumption` in the runtime API.

The way that we can determine which assumption the candidate is meant to be executed under is simply to do an exhaustive check of both possibilities based on the state of the relay-parent. First we fetch the validation data under the assumption that the block occupying becomes available. If the `validation_data_hash` of the `CandidateDescriptor` matches this validation data, we use that. Otherwise, if the `validation_data_hash` matches the validation data fetched under the `TimedOut` assumption, we use that. Otherwise, we return a `ValidationResult::Invalid` response and conclude.

Then, we can fetch the validation code from the runtime based on which type of candidate this is. This gives us all the parameters. The descriptor and PoV come from the request itself, and the other parameters have been derived from the state.

> TODO: This would be a great place for caching to avoid making lots of runtime requests. That would need a job, though.

### Execution of the Parachain Wasm

Once we have all parameters, we can spin up a background task to perform the validation in a way that doesn't hold up the entire event loop. Before invoking the validation function itself, this should first do some basic checks:
  * The collator signature is valid
  * The PoV provided matches the `pov_hash` field of the descriptor

### Checking Validation Outputs

If we can assume the presence of the relay-chain state (that is, during processing [`CandidateValidationMessage`][CVM]`::ValidateFromChainState`) we can run all the checks that the relay-chain would run at the inclusion time thus confirming that the candidate will be accepted.

### PVF Host

The PVF host is responsible for handling requests to prepare and execute PVF
code blobs.

One high-level goal is to make PVF operations as deterministic as possible, to
reduce the rate of disputes. Disputes can happen due to e.g. a job timing out on
one machine, but not another. While we do not yet have full determinism, there
are some dispute reduction mechanisms in place right now.

#### Retrying execution requests

If the execution request fails during **preparation**, we will retry if it is
possible that the preparation error was transient (e.g. if the error was a panic
or time out). We will only retry preparation if another request comes in after
15 minutes, to ensure any potential transient conditions had time to be
resolved. We will retry up to 5 times.

If the actual **execution** of the artifact fails, we will retry once if it was
an ambiguous error after a brief delay, to allow any potential transient
conditions to clear.

#### Preparation timeouts

We use timeouts for both preparation and execution jobs to limit the amount of
time they can take. As the time for a job can vary depending on the machine and
load on the machine, this can potentially lead to disputes where some validators
successfuly execute a PVF and others don't.

One dispute mitigation we have in place is a more lenient timeout for
preparation during execution than during pre-checking. The rationale is that the
PVF has already passed pre-checking, so we know it should be valid, and we allow
it to take longer than expected, as this is likely due to an issue with the
machine and not the PVF.

#### CPU clock timeouts

Another timeout-related mitigation we employ is to measure the time taken by
jobs using CPU time, rather than wall clock time. This is because the CPU time
of a process is less variable under different system conditions. When the
overall system is under heavy load, the wall clock time of a job is affected
more than the CPU time.

[CVM]: ../../types/overseer-protocol.md#validationrequesttype
