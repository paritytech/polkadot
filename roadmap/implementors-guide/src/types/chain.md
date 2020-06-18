# Chain

Types pertaining to the relay-chain - events, structures, etc.

## Block Import Event

```rust
/// Indicates that a new block has been added to the blockchain.
struct BlockImportEvent {
  /// The block header-hash.
  hash: Hash,
  /// The header itself.
  header: Header,
  /// Whether this block is considered the head of the best chain according to the
  /// event emitter's fork-choice rule.
  new_best: bool,
}
```

## Block Finalization Event

```rust
/// Indicates that a new block has been finalized.
struct BlockFinalizationEvent {
  /// The block header-hash.
  hash: Hash,
  /// The header of the finalized block.
  header: Header,
}
```
