# Historical Validation Code

Fetch the historical validation code used by a para for candidates executed in the context of a given block height in the current chain.

```rust
fn historical_validation_code(at: Block, para_id: ParaId, context_height: BlockNumber) -> Option<ValidationCode>;
```
