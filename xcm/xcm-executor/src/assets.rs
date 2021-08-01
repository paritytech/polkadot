// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use sp_std::{prelude::*, mem, collections::{btree_map::BTreeMap, btree_set::BTreeSet}};
use xcm::v0::{
	MultiAsset, MultiAssets, MultiLocation, AssetInstance, MultiAssetFilter, AssetId,
	WildMultiAsset::{All, AllOf},
	Fungibility::{Fungible, NonFungible},
	WildFungibility::{Fungible as WildFungible, NonFungible as WildNonFungible},
};
use sp_runtime::RuntimeDebug;

/// List of non-wildcard fungible and non-fungible assets.
#[derive(Default, Clone, RuntimeDebug, Eq, PartialEq)]
pub struct Assets {
	/// The fungible assets.
	pub fungible: BTreeMap<AssetId, u128>,

	/// The non-fungible assets.
	// TODO: Consider BTreeMap<AssetId, BTreeSet<AssetInstance>>
	//   or even BTreeMap<AssetId, SortedVec<AssetInstance>>
	pub non_fungible: BTreeSet<(AssetId, AssetInstance)>,
}

impl From<Vec<MultiAsset>> for Assets {
	fn from(assets: Vec<MultiAsset>) -> Assets {
		let mut result = Self::default();
		for asset in assets.into_iter() {
			result.subsume(asset)
		}
		result
	}
}

impl From<MultiAssets> for Assets {
	fn from(assets: MultiAssets) -> Assets {
		assets.drain().into()
	}
}

impl From<Assets> for Vec<MultiAsset> {
	fn from(a: Assets) -> Self {
		a.into_assets_iter().collect()
	}
}

impl From<Assets> for MultiAssets {
	fn from(a: Assets) -> Self {
		a.into_assets_iter().collect::<Vec<MultiAsset>>().into()
	}
}

impl From<MultiAsset> for Assets {
	fn from(asset: MultiAsset) -> Assets {
		let mut result = Self::default();
		result.subsume(asset);
		result
	}
}

/// An error emitted by `take` operations.
#[derive(Debug)]
pub enum TakeError {
	/// There was an attempt to take an asset without saturating (enough of) which did not exist.
	AssetUnderflow(MultiAsset),
}

impl Assets {
	/// New value, containing no assets.
	pub fn new() -> Self { Self::default() }

	/// A borrowing iterator over the fungible assets.
	pub fn fungible_assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		self.fungible.iter().map(|(id, &amount)| MultiAsset { fun: Fungible(amount), id: id.clone() })
	}

	/// A borrowing iterator over the non-fungible assets.
	pub fn non_fungible_assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		self.non_fungible.iter().map(|(id, instance)| MultiAsset { fun: NonFungible(instance.clone()), id: id.clone() })
	}

	/// A consuming iterator over all assets.
	pub fn into_assets_iter(self) -> impl Iterator<Item=MultiAsset> {
		self.fungible.into_iter().map(|(id, amount)| MultiAsset { fun: Fungible(amount), id })
			.chain(self.non_fungible.into_iter().map(|(id, instance)| MultiAsset { fun: NonFungible(instance), id }))
	}

	/// A borrowing iterator over all assets.
	pub fn assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		self.fungible_assets_iter().chain(self.non_fungible_assets_iter())
	}

	/// Mutate `self` to contain all given `assets`, saturating if necessary.
	pub fn subsume_assets(&mut self, assets: Assets) {
		// TODO: Could be done with a much faster btree entry merge and only sum the entries with the
		//   same key.
		for asset in assets.into_assets_iter() {
			self.subsume(asset)
		}
	}

	/// Mutate `self` to contain the given `asset`, saturating if necessary.
	///
	/// Wildcard values of `asset` do nothing.
	pub fn subsume(&mut self, asset: MultiAsset) {
		match asset.fun {
			Fungible(amount) => {
				self.fungible
					.entry(asset.id)
					.and_modify(|e| *e = e.saturating_add(amount))
					.or_insert(amount);
			}
			NonFungible(instance) => {
				self.non_fungible.insert((asset.id, instance));
			}
		}
	}

	/// Swaps two mutable Assets, without deinitializing either one.
	pub fn swapped(&mut self, mut with: Assets) -> Self {
		mem::swap(&mut *self, &mut with);
		with
	}

	/// Alter any concretely identified assets by prepending the given `MultiLocation`.
	///
	/// WARNING: For now we consider this infallible and swallow any errors. It is thus the caller's responsibility to
	/// ensure that any internal asset IDs are able to be prepended without overflow.
	pub fn prepend_location(&mut self, prepend: &MultiLocation) {
		let mut fungible = Default::default();
		mem::swap(&mut self.fungible, &mut fungible);
		self.fungible = fungible.into_iter()
			.map(|(mut id, amount)| { let _ = id.reanchor(prepend); (id, amount) })
			.collect();
		let mut non_fungible = Default::default();
		mem::swap(&mut self.non_fungible, &mut non_fungible);
		self.non_fungible = non_fungible.into_iter()
			.map(|(mut class, inst)| { let _ = class.reanchor(prepend); (class, inst) })
			.collect();
	}

	/// Returns an error unless all `assets` are contained in `self`. In the case of an error, the first asset in
	/// `assets` which is not wholly in `self` is returned.
	fn ensure_contains(&self, assets: &MultiAssets) -> Result<(), TakeError> {
		for asset in assets.inner().iter() {
			match asset {
				MultiAsset { fun: Fungible(ref amount), ref id } => {
					if self.fungible.get(id).map_or(true, |a| a < amount) {
						return Err(TakeError::AssetUnderflow((id.clone(), *amount).into()))
					}
				}
				MultiAsset { fun: NonFungible(ref instance), ref id } => {
					let id_instance = (id.clone(), instance.clone());
					if !self.non_fungible.contains(&id_instance) {
						return Err(TakeError::AssetUnderflow(id_instance.into()))
					}
				}
			}
		}
		return Ok(())
	}

	/// Mutates `self` to its original value less `mask` and returns `true`.
	///
	/// If `saturate` is `true`, then `self` is considered to be masked by `mask`, thereby avoiding any attempt at
	/// reducing it by assets it does not contain. In this case, the function is infallible. If `saturate` is `false`
	/// and `mask` references a definite asset which `self` does not contain then an error is returned.
	///
	/// Returns `Ok` with the definite assets token from `self` and mutates `self` to its value minus
	/// `mask`. Returns `Err` in the non-saturating case where `self` did not contain (enough of) a definite asset to
	/// be removed.
	fn general_take(&mut self, mask: MultiAssetFilter, saturate: bool) -> Result<Assets, TakeError> {
		let mut taken = Assets::new();
		match mask {
			MultiAssetFilter::Wild(All) => return Ok(self.swapped(Assets::new())),
			MultiAssetFilter::Wild(AllOf(WildFungible, id)) => {
				if let Some((id, amount)) = self.fungible.remove_entry(&id) {
					taken.fungible.insert(id, amount);
				}
			}
			MultiAssetFilter::Wild(AllOf(WildNonFungible, id)) => {
				let non_fungible = mem::replace(&mut self.non_fungible, Default::default());
				non_fungible.into_iter().for_each(|(c, instance)| {
					if c == id {
						taken.non_fungible.insert((c, instance));
					} else {
						self.non_fungible.insert((c, instance));
					}
				});
			}
			MultiAssetFilter::Definite(assets) => {
				if !saturate {
					self.ensure_contains(&assets)?;
				}
				for asset in assets.drain().into_iter() {
					match asset {
						MultiAsset { fun: Fungible(amount), id } => {
							let (remove, amount) = match self.fungible.get_mut(&id) {
								Some(self_amount) => {
									let amount = amount.min(*self_amount);
									*self_amount -= amount;
									(*self_amount == 0, amount)
								}
								None => (false, 0),
							};
							if remove {
								self.fungible.remove(&id);
							}
							if amount > 0 {
								taken.subsume(MultiAsset::from((id, amount)).into());
							}
						}
						MultiAsset { fun: NonFungible(instance), id } => {
							let id_instance = (id, instance);
							if self.non_fungible.remove(&id_instance) {
								taken.subsume(id_instance.into())
							}
						}
					}
				}
			}
		}
		Ok(taken)
	}

	/// Mutates `self` to its original value less `mask` and returns `true` iff it contains at least `mask`.
	///
	/// Returns `Ok` with the non-wildcard equivalence of `mask` taken and mutates `self` to its value minus
	/// `mask` if `self` contains `asset`, and return `Err` otherwise.
	pub fn saturating_take(&mut self, asset: MultiAssetFilter) -> Assets {
		self.general_take(asset, true)
			.expect("general_take never results in error when saturating")
	}

	/// Mutates `self` to its original value less `mask` and returns `true` iff it contains at least `mask`.
	///
	/// Returns `Ok` with the non-wildcard equivalence of `asset` taken and mutates `self` to its value minus
	/// `asset` if `self` contains `asset`, and return `Err` otherwise.
	pub fn try_take(&mut self, mask: MultiAssetFilter) -> Result<Assets, TakeError> {
		self.general_take(mask, false)
	}

	/// Consumes `self` and returns its original value excluding `asset` iff it contains at least `asset`.
	pub fn checked_sub(mut self, asset: MultiAsset) -> Result<Assets, Self> {
		// TODO: Optimize by doing this operation directly rather than converting into a MultiAssetFilter and
		//   constructing the unused `_taken` return value.
		match self.try_take(asset.into()) {
			Ok(_taken) => Ok(self),
			Err(_) => Err(self),
		}
	}

	/// Return the assets in `self`, but (asset-wise) of no greater value than `assets`.
	///
	/// Result is undefined if `assets` includes elements which match to the same asset more than once.
	///
	/// Example:
	///
	/// ```
	/// use xcm_executor::Assets;
	/// use xcm::v0::{MultiAsset, MultiLocation};
	/// let assets_i_have: Assets = vec![
	/// 	MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount: 100 },
	/// 	MultiAsset::AbstractFungible { id: vec![0], amount: 100 },
	/// ].into();
	/// let assets_they_want: Assets = vec![
	/// 	MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount: 200 },
	/// 	MultiAsset::AbstractFungible { id: vec![0], amount: 50 },
	/// ].into();
	///
	/// let assets_we_can_trade: Assets = assets_i_have.min(assets_they_want.assets_iter());
	/// assert_eq!(assets_we_can_trade.into_assets_iter().collect::<Vec<_>>(), vec![
	/// 	MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount: 100 },
	/// 	MultiAsset::AbstractFungible { id: vec![0], amount: 50 },
	/// ]);
	/// ```
	pub fn min(&self, mask: &MultiAssetFilter) -> Assets {
		let mut masked = Assets::new();
		match mask {
			MultiAssetFilter::Wild(All) => return self.clone(),
			MultiAssetFilter::Wild(AllOf(WildFungible, id)) => {
				if let Some(&amount) = self.fungible.get(&id) {
					masked.fungible.insert(id.clone(), amount);
				}
			}
			MultiAssetFilter::Wild(AllOf(WildNonFungible, id)) => {
				self.non_fungible.iter().for_each(|(ref c, ref instance)| {
					if c == id {
						masked.non_fungible.insert((c.clone(), instance.clone()));
					}
				});
			}
			MultiAssetFilter::Definite(assets) => {
				for asset in assets.inner().iter() {
					match asset {
						MultiAsset { fun: Fungible(ref amount), ref id } => {
							if let Some(m) = self.fungible.get(id) {
								masked.subsume((id.clone(), Fungible(*amount.min(m))).into());
							}
						}
						MultiAsset { fun: NonFungible(ref instance), ref id } => {
							let id_instance = (id.clone(), instance.clone());
							if self.non_fungible.contains(&id_instance) {
								masked.subsume(id_instance.into());
							}
						}
					}
				}
			}
		}
		masked
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use xcm::v0::prelude::*;
	use MultiLocation::Null;
	#[allow(non_snake_case)]
	fn AF(id: u8, amount: u128) -> MultiAsset {
		(vec![id], amount).into()
	}
	#[allow(non_snake_case)]
	fn ANF(class: u8, instance_id: u128) -> MultiAsset {
		(vec![class], AssetInstance::Index { id: instance_id }).into()
	}
	#[allow(non_snake_case)]
	fn CF(amount: u128) -> MultiAsset {
		(Null, amount).into()
	}
	#[allow(non_snake_case)]
	fn CNF(instance_id: u128) -> MultiAsset {
		(Null, AssetInstance::Index { id: instance_id }).into()
	}

	fn test_assets() -> Assets {
		let mut assets = Assets::new();
		assets.subsume(AF(1, 100));
		assets.subsume(ANF(2, 200));
		assets.subsume(CF(300));
		assets.subsume(CNF(400));
		assets
	}

	#[test]
	fn into_assets_iter_works() {
		let assets = test_assets();
		let mut iter = assets.into_assets_iter();
		// Order defined by implementation: CF, AF, CNF, ANF
		assert_eq!(Some(CF(300)), iter.next());
		assert_eq!(Some(AF(1, 100)), iter.next());
		assert_eq!(Some(CNF(400)), iter.next());
		assert_eq!(Some(ANF(2, 200)), iter.next());
		assert_eq!(None, iter.next());
	}

	#[test]
	fn assets_into_works() {
		let mut assets_vec: Vec<MultiAsset> = Vec::new();
		assets_vec.push(AF(1, 100));
		assets_vec.push(ANF(2, 200));
		assets_vec.push(CF(300));
		assets_vec.push(CNF(400));
		// Push same group of tokens again
		assets_vec.push(AF(1, 100));
		assets_vec.push(ANF(2, 200));
		assets_vec.push(CF(300));
		assets_vec.push(CNF(400));

		let assets: Assets = assets_vec.into();
		let mut iter = assets.into_assets_iter();
		// Fungibles add
		assert_eq!(Some(CF(600)), iter.next());
		assert_eq!(Some(AF(1, 200)), iter.next());
		// Non-fungibles collapse
		assert_eq!(Some(CNF(400)), iter.next());
		assert_eq!(Some(ANF(2, 200)), iter.next());
		assert_eq!(None, iter.next());
	}

	#[test]
	fn min_all_and_none_works() {
		let assets = test_assets();
		let none = MultiAssets::new().into();
		let all = All.into();

		let none_min = assets.min(&none);
		assert_eq!(None, none_min.assets_iter().next());
		let all_min = assets.min(&all);
		assert!(all_min.assets_iter().eq(assets.assets_iter()));
	}
/*
	#[test]
	fn min_all_fungible_and_all_non_fungible_works() {
		let assets = test_assets();
		let fungible = vec![MultiAsset::AllFungible];
		let non_fungible = vec![MultiAsset::AllNonFungible];

		let fungible = assets.min(fungible.iter());
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![CF(300), AF(1, 100)]);
		let non_fungible = assets.min(non_fungible.iter());
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![CNF(400), ANF(2, 200)]);
	}

	#[test]
	fn min_all_abstract_works() {
		let assets = test_assets();
		let fungible = vec![MultiAsset::AllAbstractFungible { id: vec![1] }];
		let non_fungible = vec![MultiAsset::AllAbstractNonFungible { class: vec![2] }];

		let fungible = assets.min(fungible.iter());
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![AF(1, 100)]);
		let non_fungible = assets.min(non_fungible.iter());
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![ANF(2, 200)]);
	}

	#[test]
	fn min_all_concrete_works() {
		let assets = test_assets();
		let fungible = vec![MultiAsset::AllConcreteFungible { id: MultiLocation::Null }];
		let non_fungible = vec![MultiAsset::AllConcreteNonFungible { class: MultiLocation::Null }];

		let fungible = assets.min(fungible.iter());
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![CF(300)]);
		let non_fungible = assets.min(non_fungible.iter());
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![CNF(400)]);
	}

	#[test]
	fn min_basic_works() {
		let assets1 = test_assets();

		let mut assets2_vec: Vec<MultiAsset> = Vec::new();
		// This is less than 100, so it will decrease to 50
		assets2_vec.push(AF(1, 50));
		// This asset does not exist, so not included
		assets2_vec.push(ANF(2, 400));
		// This is more then 300, so it should stay at 300
		assets2_vec.push(CF(600));
		// This asset should be included
		assets2_vec.push(CNF(400));
		let assets2: Assets = assets2_vec.into();

		let assets_min = assets1.min(assets2.assets_iter());
		let assets_min = assets_min.into_assets_iter().collect::<Vec<_>>();
		assert_eq!(assets_min, vec![CF(300), AF(1, 50), CNF(400)]);
	}

	#[test]
	fn saturating_take_all_and_none_works() {
		let mut assets = test_assets();
		let none = vec![MultiAsset::None];
		let all = vec![MultiAsset::All];

		let taken_none = assets.saturating_take(none);
		assert_eq!(None, taken_none.assets_iter().next());
		let taken_all = assets.saturating_take(all);
		// Everything taken
		assert_eq!(None, assets.assets_iter().next());
		let all_iter = taken_all.assets_iter();
		assert!(all_iter.eq(test_assets().assets_iter()));
	}

	#[test]
	fn saturating_take_all_fungible_and_all_non_fungible_works() {
		let mut assets = test_assets();
		let fungible = vec![MultiAsset::AllFungible];
		let non_fungible = vec![MultiAsset::AllNonFungible];

		let fungible = assets.saturating_take(fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![CF(300), AF(1, 100)]);
		let non_fungible = assets.saturating_take(non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, [CNF(400), ANF(2, 200)]);
		// Assets completely drained
		assert_eq!(None, assets.assets_iter().next());
	}

	#[test]
	fn saturating_take_all_abstract_works() {
		let mut assets = test_assets();
		let fungible = vec![MultiAsset::AllAbstractFungible { id: vec![1] }];
		let non_fungible = vec![MultiAsset::AllAbstractNonFungible { class: vec![2] }];

		let fungible = assets.saturating_take(fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![AF(1, 100)]);
		let non_fungible = assets.saturating_take(non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![ANF(2, 200)]);
		// Assets drained of abstract
		let final_assets = assets.assets_iter().collect::<Vec<_>>();
		assert_eq!(final_assets, vec![CF(300), CNF(400)]);
	}

	#[test]
	fn saturating_take_all_concrete_works() {
		let mut assets = test_assets();
		let fungible = vec![MultiAsset::AllConcreteFungible { id: MultiLocation::Null }];
		let non_fungible = vec![MultiAsset::AllConcreteNonFungible { class: MultiLocation::Null }];

		let fungible = assets.saturating_take(fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![CF(300)]);
		let non_fungible = assets.saturating_take(non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![CNF(400)]);
		// Assets drained of concrete
		let assets = assets.assets_iter().collect::<Vec<_>>();
		assert_eq!(assets, vec![AF(1, 100), ANF(2, 200)]);
	}

	#[test]
	fn saturating_take_basic_works() {
		let mut assets1 = test_assets();

		let mut assets2_vec: Vec<MultiAsset> = Vec::new();
		// We should take 50
		assets2_vec.push(AF(1, 50));
		// This asset should not be taken
		assets2_vec.push(ANF(2, 400));
		// This is more then 300, so it takes everything
		assets2_vec.push(CF(600));
		// This asset should be taken
		assets2_vec.push(CNF(400));

		let taken = assets1.saturating_take(assets2_vec);
		let taken = taken.into_assets_iter().collect::<Vec<_>>();
		assert_eq!(taken, vec![CF(300), AF(1, 50), CNF(400)]);

		let assets = assets1.into_assets_iter().collect::<Vec<_>>();
		assert_eq!(assets, vec![AF(1, 50), ANF(2, 200)]);
	}*/
}
