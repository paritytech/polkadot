# Session Index

Get the session index that is expected at the child of a block.

In the [`Initializer`](../runtime/initializer.md) module, session changes are buffered by one block. The session index of the child of any relay block is always predictable by that block's state.

This session index can be used to derive a [`SigningContext`](../types/candidate.md#signing-context).

```rust
/// Returns the session index expected at a child of the block.
fn session_index_for_child(at: Block) -> SessionIndex;
```
