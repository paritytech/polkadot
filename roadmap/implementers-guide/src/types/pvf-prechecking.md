# PVF Pre-checking types

## `PvfCheckStatement`

> ⚠️ This type was added in v2.

One of the main units of information on which PVF pre-checking voting is build is the `PvfCheckStatement`.

This is a statement by the validator who ran the pre-checking process for a PVF. A PVF is identified by the `ValidationCodeHash`.

The statement is valid only during a single session, specified in the `session_index`.

```rust
struct PvfCheckStatement {
    /// `true` if the subject passed pre-checking and `false` otherwise.
    pub accept: bool,
    /// The validation code hash that was checked.
    pub subject: ValidationCodeHash,
    /// The index of a session during which this statement is considered valid.
    pub session_index: SessionIndex,
    /// The index of the validator from which this statement originates.
    pub validator_index: ValidatorIndex,
}
```
