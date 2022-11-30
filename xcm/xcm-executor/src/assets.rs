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

use sp_runtime::{traits::Saturating, RuntimeDebug};
use sp_std::{
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	mem,
	prelude::*,
};
use xcm::latest::{
	AssetId, AssetInstance,
	Fungibility::{Fungible, NonFungible},
	MultiAsset, MultiAssetFilter, MultiAssets, MultiLocation,
	WildFungibility::{Fungible as WildFungible, NonFungible as WildNonFungible},
	WildMultiAsset::{All, AllOf},
};

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

impl From<MultiAsset> for Assets {
	fn from(asset: MultiAsset) -> Assets {
		let mut result = Self::default();
		result.subsume(asset);
		result
	}
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

/// An error emitted by `take` operations.
#[derive(Debug)]
pub enum TakeError {
	/// There was an attempt to take an asset without saturating (enough of) which did not exist.
	AssetUnderflow(MultiAsset),
}

impl Assets {
	/// New value, containing no assets.
	pub fn new() -> Self {
		Self::default()
	}

	/// Total number of distinct assets.
	pub fn len(&self) -> usize {
		self.fungible.len() + self.non_fungible.len()
	}

	/// Returns `true` if `self` contains no assets.
	pub fn is_empty(&self) -> bool {
		self.fungible.is_empty() && self.non_fungible.is_empty()
	}

	/// A borrowing iterator over the fungible assets.
	pub fn fungible_assets_iter(&self) -> impl Iterator<Item = MultiAsset> + '_ {
		self.fungible
			.iter()
			.map(|(id, &amount)| MultiAsset { fun: Fungible(amount), id: id.clone() })
	}

	/// A borrowing iterator over the non-fungible assets.
	pub fn non_fungible_assets_iter(&self) -> impl Iterator<Item = MultiAsset> + '_ {
		self.non_fungible
			.iter()
			.map(|(id, instance)| MultiAsset { fun: NonFungible(instance.clone()), id: id.clone() })
	}

	/// A consuming iterator over all assets.
	pub fn into_assets_iter(self) -> impl Iterator<Item = MultiAsset> {
		self.fungible
			.into_iter()
			.map(|(id, amount)| MultiAsset { fun: Fungible(amount), id })
			.chain(
				self.non_fungible
					.into_iter()
					.map(|(id, instance)| MultiAsset { fun: NonFungible(instance), id }),
			)
	}

	/// A borrowing iterator over all assets.
	pub fn assets_iter(&self) -> impl Iterator<Item = MultiAsset> + '_ {
		self.fungible_assets_iter().chain(self.non_fungible_assets_iter())
	}

	/// Mutate `self` to contain all given `assets`, saturating if necessary.
	///
	/// NOTE: [`Assets`] are always sorted, allowing us to optimize this function from `O(n^2)` to `O(n)`.
	pub fn subsume_assets(&mut self, mut assets: Assets) {
		let mut f_iter = assets.fungible.iter_mut();
		let mut g_iter = self.fungible.iter_mut();
		if let (Some(mut f), Some(mut g)) = (f_iter.next(), g_iter.next()) {
			loop {
				if f.0 == g.0 {
					// keys are equal. in this case, we add `self`'s balance for the asset onto `assets`, balance, knowing
					// that the `append` operation which follows will clobber `self`'s value and only use `assets`'s.
					(*f.1).saturating_accrue(*g.1);
				}
				if f.0 <= g.0 {
					f = match f_iter.next() {
						Some(x) => x,
						None => break,
					};
				}
				if f.0 >= g.0 {
					g = match g_iter.next() {
						Some(x) => x,
						None => break,
					};
				}
			}
		}
		self.fungible.append(&mut assets.fungible);
		self.non_fungible.append(&mut assets.non_fungible);
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
			},
			NonFungible(instance) => {
				self.non_fungible.insert((asset.id, instance));
			},
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
		self.fungible = fungible
			.into_iter()
			.map(|(mut id, amount)| {
				let _ = id.prepend_with(prepend);
				(id, amount)
			})
			.collect();
		let mut non_fungible = Default::default();
		mem::swap(&mut self.non_fungible, &mut non_fungible);
		self.non_fungible = non_fungible
			.into_iter()
			.map(|(mut class, inst)| {
				let _ = class.prepend_with(prepend);
				(class, inst)
			})
			.collect();
	}

	/// Mutate the assets to be interpreted as the same assets from the perspective of a `target`
	/// chain. The local chain's `ancestry` is provided.
	///
	/// Any assets which were unable to be reanchored are introduced into `failed_bin`.
	pub fn reanchor(
		&mut self,
		target: &MultiLocation,
		ancestry: &MultiLocation,
		mut maybe_failed_bin: Option<&mut Self>,
	) {
		let mut fungible = Default::default();
		mem::swap(&mut self.fungible, &mut fungible);
		self.fungible = fungible
			.into_iter()
			.filter_map(|(mut id, amount)| match id.reanchor(target, ancestry) {
				Ok(()) => Some((id, amount)),
				Err(()) => {
					maybe_failed_bin.as_mut().map(|f| f.fungible.insert(id, amount));
					None
				},
			})
			.collect();
		let mut non_fungible = Default::default();
		mem::swap(&mut self.non_fungible, &mut non_fungible);
		self.non_fungible = non_fungible
			.into_iter()
			.filter_map(|(mut class, inst)| match class.reanchor(target, ancestry) {
				Ok(()) => Some((class, inst)),
				Err(()) => {
					maybe_failed_bin.as_mut().map(|f| f.non_fungible.insert((class, inst)));
					None
				},
			})
			.collect();
	}

	/// Returns an error unless all `assets` are contained in `self`. In the case of an error, the first asset in
	/// `assets` which is not wholly in `self` is returned.
	pub fn ensure_contains(&self, assets: &MultiAssets) -> Result<(), TakeError> {
		for asset in assets.inner().iter() {
			match asset {
				MultiAsset { fun: Fungible(ref amount), ref id } => {
					if self.fungible.get(id).map_or(true, |a| a < amount) {
						return Err(TakeError::AssetUnderflow((id.clone(), *amount).into()))
					}
				},
				MultiAsset { fun: NonFungible(ref instance), ref id } => {
					let id_instance = (id.clone(), instance.clone());
					if !self.non_fungible.contains(&id_instance) {
						return Err(TakeError::AssetUnderflow(id_instance.into()))
					}
				},
			}
		}
		return Ok(())
	}

	/// Mutates `self` to its original value less `mask` and returns assets that were removed.
	///
	/// If `saturate` is `true`, then `self` is considered to be masked by `mask`, thereby avoiding any attempt at
	/// reducing it by assets it does not contain. In this case, the function is infallible. If `saturate` is `false`
	/// and `mask` references a definite asset which `self` does not contain then an error is returned.
	///
	/// The number of unique assets which are removed will never be any greater than `limit`.
	///
	/// Returns `Ok` with the definite assets token from `self` and mutates `self` to its value minus
	/// `mask`. Returns `Err` in the non-saturating case where `self` did not contain (enough of) a definite asset to
	/// be removed.
	fn general_take(
		&mut self,
		mask: MultiAssetFilter,
		saturate: bool,
		limit: usize,
	) -> Result<Assets, TakeError> {
		let mut taken = Assets::new();
		match mask {
			MultiAssetFilter::Wild(All) =>
				if self.fungible.len() + self.non_fungible.len() <= limit {
					return Ok(self.swapped(Assets::new()))
				} else {
					let fungible = mem::replace(&mut self.fungible, Default::default());
					fungible.into_iter().for_each(|(c, amount)| {
						if taken.len() < limit {
							taken.fungible.insert(c, amount);
						} else {
							self.fungible.insert(c, amount);
						}
					});
					let non_fungible = mem::replace(&mut self.non_fungible, Default::default());
					non_fungible.into_iter().for_each(|(c, instance)| {
						if taken.len() < limit {
							taken.non_fungible.insert((c, instance));
						} else {
							self.non_fungible.insert((c, instance));
						}
					});
				},
			MultiAssetFilter::Wild(AllOf { fun: WildFungible, id }) => {
				if let Some((id, amount)) = self.fungible.remove_entry(&id) {
					taken.fungible.insert(id, amount);
				}
			},
			MultiAssetFilter::Wild(AllOf { fun: WildNonFungible, id }) => {
				let non_fungible = mem::replace(&mut self.non_fungible, Default::default());
				non_fungible.into_iter().for_each(|(c, instance)| {
					if c == id && taken.len() < limit {
						taken.non_fungible.insert((c, instance));
					} else {
						self.non_fungible.insert((c, instance));
					}
				});
			},
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
								},
								None => (false, 0),
							};
							if remove {
								self.fungible.remove(&id);
							}
							if amount > 0 {
								taken.subsume(MultiAsset::from((id, amount)).into());
							}
						},
						MultiAsset { fun: NonFungible(instance), id } => {
							let id_instance = (id, instance);
							if self.non_fungible.remove(&id_instance) {
								taken.subsume(id_instance.into())
							}
						},
					}
					if taken.len() == limit {
						break
					}
				}
			},
		}
		Ok(taken)
	}

	/// Mutates `self` to its original value less `mask` and returns `true` iff it contains at least `mask`.
	///
	/// Returns `Ok` with the non-wildcard equivalence of `mask` taken and mutates `self` to its value minus
	/// `mask` if `self` contains `asset`, and return `Err` otherwise.
	pub fn saturating_take(&mut self, asset: MultiAssetFilter) -> Assets {
		self.general_take(asset, true, usize::max_value())
			.expect("general_take never results in error when saturating")
	}

	/// Mutates `self` to its original value less `mask` and returns `true` iff it contains at least `mask`.
	///
	/// Returns `Ok` with the non-wildcard equivalence of `mask` taken and mutates `self` to its value minus
	/// `mask` if `self` contains `asset`, and return `Err` otherwise.
	pub fn limited_saturating_take(&mut self, asset: MultiAssetFilter, limit: usize) -> Assets {
		self.general_take(asset, true, limit)
			.expect("general_take never results in error when saturating")
	}

	/// Mutates `self` to its original value less `mask` and returns `true` iff it contains at least `mask`.
	///
	/// Returns `Ok` with the non-wildcard equivalence of `asset` taken and mutates `self` to its value minus
	/// `asset` if `self` contains `asset`, and return `Err` otherwise.
	pub fn try_take(&mut self, mask: MultiAssetFilter) -> Result<Assets, TakeError> {
		self.general_take(mask, false, usize::max_value())
	}

	/// Consumes `self` and returns its original value excluding `asset` iff it contains at least `asset`.
	pub fn checked_sub(mut self, asset: MultiAsset) -> Result<Assets, Assets> {
		match asset.fun {
			Fungible(amount) => {
				let remove = if let Some(balance) = self.fungible.get_mut(&asset.id) {
					if *balance >= amount {
						*balance -= amount;
						*balance == 0
					} else {
						return Err(self)
					}
				} else {
					return Err(self)
				};
				if remove {
					self.fungible.remove(&asset.id);
				}
				Ok(self)
			},
			NonFungible(instance) =>
				if self.non_fungible.remove(&(asset.id, instance)) {
					Ok(self)
				} else {
					Err(self)
				},
		}
	}

	/// Return the assets in `self`, but (asset-wise) of no greater value than `mask`.
	///
	/// Result is undefined if `mask` includes elements which match to the same asset more than once.
	///
	/// Example:
	///
	/// ```
	/// use xcm_executor::Assets;
	/// use xcm::latest::prelude::*;
	/// let assets_i_have: Assets = vec![ (Here, 100).into(), (vec![0], 100).into() ].into();
	/// let assets_they_want: MultiAssetFilter = vec![ (Here, 200).into(), (vec![0], 50).into() ].into();
	///
	/// let assets_we_can_trade: Assets = assets_i_have.min(&assets_they_want);
	/// assert_eq!(assets_we_can_trade.into_assets_iter().collect::<Vec<_>>(), vec![
	/// 	(Here, 100).into(), (vec![0], 50).into(),
	/// ]);
	/// ```
	pub fn min(&self, mask: &MultiAssetFilter) -> Assets {
		let mut masked = Assets::new();
		match mask {
			MultiAssetFilter::Wild(All) => return self.clone(),
			MultiAssetFilter::Wild(AllOf { fun: WildFungible, id }) => {
				if let Some(&amount) = self.fungible.get(&id) {
					masked.fungible.insert(id.clone(), amount);
				}
			},
			MultiAssetFilter::Wild(AllOf { fun: WildNonFungible, id }) => {
				self.non_fungible.iter().for_each(|(ref c, ref instance)| {
					if c == id {
						masked.non_fungible.insert((c.clone(), instance.clone()));
					}
				});
			},
			MultiAssetFilter::Definite(assets) =>
				for asset in assets.inner().iter() {
					match asset {
						MultiAsset { fun: Fungible(ref amount), ref id } => {
							if let Some(m) = self.fungible.get(id) {
								masked.subsume((id.clone(), Fungible(*amount.min(m))).into());
							}
						},
						MultiAsset { fun: NonFungible(ref instance), ref id } => {
							let id_instance = (id.clone(), instance.clone());
							if self.non_fungible.contains(&id_instance) {
								masked.subsume(id_instance.into());
							}
						},
					}
				},
		}
		masked
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use xcm::latest::prelude::*;
	#[allow(non_snake_case)]
	/// Abstract fungible constructor
	fn AF(id: u8, amount: u128) -> MultiAsset {
		(vec![id], amount).into()
	}
	#[allow(non_snake_case)]
	/// Abstract non-fungible constructor
	fn ANF(class: u8, instance_id: u8) -> MultiAsset {
		(vec![class], vec![instance_id]).into()
	}
	#[allow(non_snake_case)]
	/// Concrete fungible constructor
	fn CF(amount: u128) -> MultiAsset {
		(Here, amount).into()
	}
	#[allow(non_snake_case)]
	/// Concrete non-fungible constructor
	fn CNF(instance_id: u8) -> MultiAsset {
		(Here, [instance_id; 4]).into()
	}

	fn test_assets() -> Assets {
		let mut assets = Assets::new();
		assets.subsume(AF(1, 100));
		assets.subsume(ANF(2, 20));
		assets.subsume(CF(300));
		assets.subsume(CNF(40));
		assets
	}

	#[test]
	fn subsume_assets_works() {
		let t1 = test_assets();
		let mut t2 = Assets::new();
		t2.subsume(AF(1, 50));
		t2.subsume(ANF(2, 10));
		t2.subsume(CF(300));
		t2.subsume(CNF(50));
		let mut r1 = t1.clone();
		r1.subsume_assets(t2.clone());
		let mut r2 = t1.clone();
		for a in t2.assets_iter() {
			r2.subsume(a)
		}
		assert_eq!(r1, r2);
	}

	#[test]
	fn checked_sub_works() {
		let t = test_assets();
		let t = t.checked_sub(AF(1, 50)).unwrap();
		let t = t.checked_sub(AF(1, 51)).unwrap_err();
		let t = t.checked_sub(AF(1, 50)).unwrap();
		let t = t.checked_sub(AF(1, 1)).unwrap_err();
		let t = t.checked_sub(CF(150)).unwrap();
		let t = t.checked_sub(CF(151)).unwrap_err();
		let t = t.checked_sub(CF(150)).unwrap();
		let t = t.checked_sub(CF(1)).unwrap_err();
		let t = t.checked_sub(ANF(2, 21)).unwrap_err();
		let t = t.checked_sub(ANF(2, 20)).unwrap();
		let t = t.checked_sub(ANF(2, 20)).unwrap_err();
		let t = t.checked_sub(CNF(41)).unwrap_err();
		let t = t.checked_sub(CNF(40)).unwrap();
		let t = t.checked_sub(CNF(40)).unwrap_err();
		assert_eq!(t, Assets::new());
	}

	#[test]
	fn into_assets_iter_works() {
		let assets = test_assets();
		let mut iter = assets.into_assets_iter();
		// Order defined by implementation: CF, AF, CNF, ANF
		assert_eq!(Some(CF(300)), iter.next());
		assert_eq!(Some(AF(1, 100)), iter.next());
		assert_eq!(Some(CNF(40)), iter.next());
		assert_eq!(Some(ANF(2, 20)), iter.next());
		assert_eq!(None, iter.next());
	}

	#[test]
	fn assets_into_works() {
		let mut assets_vec: Vec<MultiAsset> = Vec::new();
		assets_vec.push(AF(1, 100));
		assets_vec.push(ANF(2, 20));
		assets_vec.push(CF(300));
		assets_vec.push(CNF(40));
		// Push same group of tokens again
		assets_vec.push(AF(1, 100));
		assets_vec.push(ANF(2, 20));
		assets_vec.push(CF(300));
		assets_vec.push(CNF(40));

		let assets: Assets = assets_vec.into();
		let mut iter = assets.into_assets_iter();
		// Fungibles add
		assert_eq!(Some(CF(600)), iter.next());
		assert_eq!(Some(AF(1, 200)), iter.next());
		// Non-fungibles collapse
		assert_eq!(Some(CNF(40)), iter.next());
		assert_eq!(Some(ANF(2, 20)), iter.next());
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

	#[test]
	fn min_all_abstract_works() {
		let assets = test_assets();
		let fungible = Wild((vec![1], WildFungible).into());
		let non_fungible = Wild((vec![2], WildNonFungible).into());

		let fungible = assets.min(&fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![AF(1, 100)]);
		let non_fungible = assets.min(&non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![ANF(2, 20)]);
	}

	#[test]
	fn min_all_concrete_works() {
		let assets = test_assets();
		let fungible = Wild((Here, WildFungible).into());
		let non_fungible = Wild((Here, WildNonFungible).into());

		let fungible = assets.min(&fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![CF(300)]);
		let non_fungible = assets.min(&non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![CNF(40)]);
	}

	#[test]
	fn min_basic_works() {
		let assets1 = test_assets();

		let mut assets2 = Assets::new();
		// This is less than 100, so it will decrease to 50
		assets2.subsume(AF(1, 50));
		// This asset does not exist, so not included
		assets2.subsume(ANF(2, 40));
		// This is more then 300, so it should stay at 300
		assets2.subsume(CF(600));
		// This asset should be included
		assets2.subsume(CNF(40));
		let assets2: MultiAssets = assets2.into();

		let assets_min = assets1.min(&assets2.into());
		let assets_min = assets_min.into_assets_iter().collect::<Vec<_>>();
		assert_eq!(assets_min, vec![CF(300), AF(1, 50), CNF(40)]);
	}

	#[test]
	fn saturating_take_all_and_none_works() {
		let mut assets = test_assets();

		let taken_none = assets.saturating_take(vec![].into());
		assert_eq!(None, taken_none.assets_iter().next());
		let taken_all = assets.saturating_take(All.into());
		// Everything taken
		assert_eq!(None, assets.assets_iter().next());
		let all_iter = taken_all.assets_iter();
		assert!(all_iter.eq(test_assets().assets_iter()));
	}

	#[test]
	fn saturating_take_all_abstract_works() {
		let mut assets = test_assets();
		let fungible = Wild((vec![1], WildFungible).into());
		let non_fungible = Wild((vec![2], WildNonFungible).into());

		let fungible = assets.saturating_take(fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![AF(1, 100)]);
		let non_fungible = assets.saturating_take(non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![ANF(2, 20)]);
		// Assets drained of abstract
		let final_assets = assets.assets_iter().collect::<Vec<_>>();
		assert_eq!(final_assets, vec![CF(300), CNF(40)]);
	}

	#[test]
	fn saturating_take_all_concrete_works() {
		let mut assets = test_assets();
		let fungible = Wild((Here, WildFungible).into());
		let non_fungible = Wild((Here, WildNonFungible).into());

		let fungible = assets.saturating_take(fungible);
		let fungible = fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(fungible, vec![CF(300)]);
		let non_fungible = assets.saturating_take(non_fungible);
		let non_fungible = non_fungible.assets_iter().collect::<Vec<_>>();
		assert_eq!(non_fungible, vec![CNF(40)]);
		// Assets drained of concrete
		let assets = assets.assets_iter().collect::<Vec<_>>();
		assert_eq!(assets, vec![AF(1, 100), ANF(2, 20)]);
	}

	#[test]
	fn saturating_take_basic_works() {
		let mut assets1 = test_assets();

		let mut assets2 = Assets::new();
		// We should take 50
		assets2.subsume(AF(1, 50));
		// This asset should not be taken
		assets2.subsume(ANF(2, 40));
		// This is more then 300, so it takes everything
		assets2.subsume(CF(600));
		// This asset should be taken
		assets2.subsume(CNF(40));
		let assets2: MultiAssets = assets2.into();

		let taken = assets1.saturating_take(assets2.into());
		let taken = taken.into_assets_iter().collect::<Vec<_>>();
		assert_eq!(taken, vec![CF(300), AF(1, 50), CNF(40)]);

		let assets = assets1.into_assets_iter().collect::<Vec<_>>();
		assert_eq!(assets, vec![AF(1, 50), ANF(2, 20)]);
	}
}
