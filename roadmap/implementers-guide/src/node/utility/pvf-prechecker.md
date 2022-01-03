# PVF Pre-checker

The PVF pre-checker is a subsystem that is responsible for watching the relay chain for new PVFs that require pre-checking. Head over to [overview] for the PVF pre-checking process overview.

## Protocol

There is no dedicated input mechanism for PVF pre-checker. Instead, PVF pre-checker looks on the `ActiveLeavesUpdate` event stream for work.

This subsytem does not produce any output messages either. The subsystem will, however, send messages to the [Runtime API] subsystem to query for the pending PVFs and to submit votes. In addition to that, it will also communicate with [Candidate Validation] Subsystem to request PVF pre-check.

## Functionality

TODO: Write up the description of the functionality of the PVF pre-checker. https://github.com/paritytech/polkadot/issues/4611

[overview]: ../../pvf-prechecking.md
[Runtime API]: runtime-api.md
[Candidate Validation]: candidate-validation.md
