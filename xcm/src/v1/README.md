# XCM Version 1
The comprehensive list of changes can be found in [this PR description](https://github.com/paritytech/polkadot/pull/2815#issue-608567900).

## Changes to be aware of
Most changes should automatically be resolved via the conversion traits (i.e. `TryFrom` and `From`). The list here is mostly for incompatible changes that result in an `Err(())` when attempting to convert XCM objects from v0.

### Junction
- `v0::Junction::Parent` cannot be converted to v1, because the way we represent parents in v1 has changed - instead of being a property of the junction, v1 MultiLocations now have an extra field representing the number of parents that the MultiLocation contains.

### MultiLocation
- The `try_from` conversion method will always canonicalize the v0 MultiLocation before attempting to do the proper conversion. Since canonicalization is not a fallible operation, we do not expect v0 MultiLocation to ever fail to be upgraded to v1.

### MultiAsset
- Stronger typing to differentiate between a single class of `MultiAsset` and several classes of `MultiAssets` is introduced. As the name suggests, a `Vec<MultiAsset>` that is used on all APIs will instead be using a new type called `MultiAssets` (note the `s`).
- All `MultiAsset` variants whose name contains "All" in it, namely `v0::MultiAsset::All`, `v0::MultiAsset::AllFungible`, `v0::MultiAsset::AllNonFungible`, `v0::MultiAsset::AllAbstractFungible`, `v0::MultiAsset::AllAbstractNonFungible`, `v0::MultiAsset::AllConcreteFungible` and `v0::MultiAsset::AllConcreteNonFungible`, will fail to convert to v1 MultiAsset, since v1 does not contain these variants.
- Similarly, all `MultiAsset` variants whose name contains "All" in it can be converted into a `WildMultiAsset`.
- `v0::MultiAsset::None` is not represented at all in v1.

### XCM
- No special attention necessary

### Order
- `v1::Order::DepositAsset` and `v1::Order::DepositReserveAsset` both introduced a new `max_asset` field that limits the maximum classes of assets that can be deposited. During conversion from v0, the `max_asset` field defaults to 1.
- v1 Orders that contain `MultiAsset` as argument(s) will need to explicitly specify the amount and details of assets. This is to prevent accidental misuse of `All` to possibly transfer, spend or otherwise perform unintended operations on `All` assets.
- v1 Orders that do allow the notion of `All` to be used as wildcards, will instead use a new type called `MultiAssetFilter`.
