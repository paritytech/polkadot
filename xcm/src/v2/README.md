# XCM Version 2
The comprehensive list of changes can be found in [this PR description](https://github.com/paritytech/polkadot/pull/3629#issue-968428279).

## Changes to be aware of
The biggest change here is the restructuring of XCM messages: instead of having `Order` and `Xcm` types, the `Xcm` type now simply wraps a `Vec` containing `Instruction`s. However, most changes should still be automatically convertible via the `try_from` and `from` conversion functions.

### Junction
- No special attention necessary

### MultiLocation
- No special attention necessary

### MultiAsset
- No special attention necessary

### XCM and Order
- `Xcm` and `Order` variants are now combined under a single `Instruction` enum.
- `Order` is now obsolete and replaced entirely by `Instruction`.
- `Xcm` is now a simple wrapper around a `Vec<Instruction>`.
- During conversion from `Order` to `Instruction`, we do not handle `BuyExecution`s that have nested XCMs, i.e. if the `instructions` field in the `BuyExecution` enum struct variant is not empty, then the conversion will fail. To address this, rewrite the XCM using `Instruction`s in chronological order.
- During conversion from `Xcm` to `Instruction`, we do not handle `RelayedFrom` messages at all.
