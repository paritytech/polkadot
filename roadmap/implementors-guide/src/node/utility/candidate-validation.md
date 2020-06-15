# Candidate Validation

This subsystem is responsible for handling candidate validation requests. It is a simple request/response server.

A variety of subsystems want to know if a parachain block candidate is valid: Candidate Selection needs to know if a candidate it's considering for seconding is valid; Candidate Backing needs to know whether peer-seconded candidates are actually valid; Misbehavior Arbitration farms out disputes to a variety of validators to handle disputes; etc. None of them care about the detailed mechanics of how a candidate gets validated, just the results. This subsystem handles those details.

## Protocol

Input: [`CandidateValidationMessage`](/type-definitions.html#validation-request-type)

Output: [`Statement`](/type-definitions.html#statement-type) via the provided `Sender<Statement>`.

## Functionality

Given a candidate, its validation code, and its PoV, determine whether the candidate is valid. There are a few different situations this code will be called in, and this will lead to variance in where the parameters originate. Determining the parameters is beyond the scope of this subsystem.
