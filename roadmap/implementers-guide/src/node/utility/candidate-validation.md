# Candidate Validation

This subsystem is responsible for handling candidate validation requests. It is a simple request/response server.

A variety of subsystems want to know if a parachain block candidate is valid. None of them care about the detailed mechanics of how a candidate gets validated, just the results. This subsystem handles those details.

## Protocol

Input: [`CandidateValidationMessage`](../../types/overseer-protocol.md#validation-request-type)

Output: Validation result via the provided response side-channel.

## Functionality

This subsystem answers two types of requests: one which draws out validation data from the state, and another which accepts all validation data exhaustively. The goal of both request types is to validate a candidate. There are three possible outputs of validation: either the candidate is valid, the candidate is invalid, or an internal error occurred. Whatever the end result is, it will be returned on the response channel to the requestor.

Parachain candidates are validated against their validation function: A piece of Wasm code that is describes the state-transition of the parachain. Validation function execution is not metered. This means that an execution which is an infinite loop or simply takes too long must be forcibly exited by some other means. For this reason, we recommend dispatching candidate validation to be done on subprocesses which can be killed if they time-out.

Upon receiving a validation request, the first thing the candidate validation subsystem should do is make sure it has all the necessary parameters to the validation function. These are:
  * The Validation Function itself.
  * The [`CandidateDescriptor`](../../types/candidate.md#candidatedescriptor).
  * The [`ValidationData`](../../types/candidate.md#validationdata).
  * The [`PoV`](../../types/availability.md#proofofvalidity).

### Determining Parameters

For a [`CandidateValidationMessage`][CVM]`::ValidateFromExhaustive`, these parameters are exhaustively provided. The [`TransientValidationData`](../../types/candidate.md#transientvalidationdata) is optional, and is used to perform further checks on the outputs of validation.

For a [`CandidateValidationMessage`][CVM]`::ValidateFromChainState`, some more work needs to be done. Due to the uncertainty of Availability Cores (implemented in the [`Scheduler`](../../runtime/scheduler.md) module of the runtime), a candidate at a particular relay-parent and for a particular para may have two different valid validation-data to be executed under depending on what is assumed to happen if the para is occupying a core at the onset of the new block. This is encoded as an `OccupiedCoreAssumption` in the runtime API.

The way that we can determine which assumption the candidate is meant to be executed under is simply to do an exhaustive check of both possibilities based on the state of the relay-parent. First we fetch the validation data under the assumption that the block occupying becomes available. If the `validation_data_hash` of the `CandidateDescriptor` matches this validation data, we use that. Otherwise, if the `validation_data_hash` matches the validation data fetched under the `TimedOut` assumption, we use that. Otherwise, we return a `ValidationResult::Invalid` response and conclude.

Then, we can fetch the validation code from the runtime based on which type of candidate this is. This gives us all the parameters. The descriptor and PoV come from the request itself, and the other parameters have been derived from the state.

> TODO: This would be a great place for caching to avoid making lots of runtime requests. That would need a job, though.

### Execution of the Parachain Wasm

Once we have all parameters, we can spin up a background task to perform the validation in a way that doesn't hold up the entire event loop. Before invoking the validation function itself, this should first do some basic checks:
  * The collator signature is valid
  * The PoV provided matches the `pov_hash` field of the descriptor

After that, we can invoke the validation function. Lastly, if available, we do some final checks on the output using the `TransientValidationData`:
  * The produced head-data is no larger than the maximum allowed.
  * The produced code upgrade, if any, is no larger than the maximum allowed, and a code upgrade was allowed to be signaled.
  * The amount and size of produced upward messages is not too large.

[CVM]: ../../types/overseer-protocol.md#validationrequesttype
