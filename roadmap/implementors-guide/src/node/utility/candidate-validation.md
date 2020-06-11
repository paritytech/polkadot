# Candidate Validation

This subsystem is responsible for handling candidate validation requests. It is a simple request/response server.

## Protocol

Input:

- [`CandidateValidationMessage`](/type-definitions.html#validation-request-type)

## Functionality

Given a candidate, its validation code, and its PoV, determine whether the candidate is valid. There are a few different situations this code will be called in, and this will lead to variance in where the parameters originate. Determining the parameters is beyond the scope of this subsystem.
