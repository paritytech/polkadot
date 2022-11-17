# PVF Types

## `ValidationHost`

```rust
/// A handle to the async process serving the validation host requests.
pub struct ValidationHost {
	to_host_tx: mpsc::Sender<ToHost>,
}
```

## `Pvf`

```rust
/// A struct that carries code of a parachain validation function and its hash.
pub struct Pvf {
	pub(crate) code: Arc<Vec<u8>>,
	pub(crate) code_hash: ValidationCodeHash,
}
```

## `Priority`

```rust
/// A priority assigned to execution of a PVF.
pub enum Priority {
	/// Normal priority for things that do not require immediate response, but still need to be
	/// done pretty quick.
	///
	/// Approvals and disputes fall into this category.
	Normal,
	/// This priority is used for requests that are required to be processed as soon as possible.
	///
	/// For example, backing is on a critical path and requires execution as soon as possible.
	Critical,
}
```
