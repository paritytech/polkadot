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

For this reason this subsystem uses a convenient Runtime API endpoint — [`AssumedValidationData`](../../types/overseer-protocol.md#runtime-api-message). It accepts two parameters: parachain ID and the expected hash of the validation data, — the one we obtain from the `CandidateDescriptor`. Then the runtime tries to construct the validation data for the given parachain first under the assumption that the block occupying the core didn't advance the para, i.e. it didn't reach the availability, if the data hash doesn't match the expected one, tries again with force enacting the core, temporarily updating the state as if the block had been deemed available. In case of a match the API returns the constructed validation data along with the corresponding validation code hash, or `None` otherwise. The latter means that the validation data hash in the descriptor is not based on the relay parent and thus given candidate is invalid.

The validation backend, the one that is responsible for actually compiling and executing wasm code, keeps an artifact cache. This allows the subsystem to attempt the validation by the code hash obtained earlier. If the code with the given hash is missing though, we will have to perform another request necessary to obtain the validation function: `ValidationCodeByHash`.

This gives us all the parameters. The descriptor and PoV come from the request itself, and the other parameters have been derived from the state.

### Execution of the Parachain Wasm

Once we have all parameters, we can spin up a background task to perform the validation in a way that doesn't hold up the entire event loop. Before invoking the validation function itself, this should first do some basic checks:
  * The collator signature is valid
  * The PoV provided matches the `pov_hash` field of the descriptor

### Checking Validation Outputs

If we can assume the presence of the relay-chain state (that is, during processing [`CandidateValidationMessage`][CVM]`::ValidateFromChainState`) we can run all the checks that the relay-chain would run at the inclusion time thus confirming that the candidate will be accepted.

[CVM]: ../../types/overseer-protocol.md#validationrequesttype
