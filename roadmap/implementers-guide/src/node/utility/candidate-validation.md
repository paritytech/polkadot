# Candidate Validation

This subsystem is responsible for handling candidate validation requests. It is a simple request/response server.

A variety of subsystems want to know if a parachain block candidate is valid. None of them care about the detailed mechanics of how a candidate gets validated, just the results. This subsystem handles those details.

## Protocol

Input: [`CandidateValidationMessage`](../../types/overseer-protocol.md#validation-request-type)

Output: Validation result via the provided response side-channel.

## Functionality

Given the hashes of a relay parent and a parachain candidate block, and either its PoV or the information with which to retrieve the PoV from the network, spawn a short-lived async job to determine whether the candidate is valid.

Each job follows this process:

- Get the full candidate from the current relay chain state
- Check the candidate's proof
   > TODO: that's extremely hand-wavey. What does that actually entail?
- Generate either `Statement::Valid` or `Statement::Invalid`. Note that this never generates `Statement::Seconded`; Candidate Backing is the only subsystem which upgrades valid to seconded.
- Return the statement on the provided channel.
