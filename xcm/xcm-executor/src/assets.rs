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
use xcm::v0::{MultiAsset, MultiLocation, AssetInstance};
use sp_runtime::RuntimeDebug;

/// Classification of an asset being concrete or abstract.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, RuntimeDebug)]
pub enum AssetId {
	Concrete(MultiLocation),
	Abstract(Vec<u8>),
}

impl AssetId {
	/// Prepend a MultiLocation to a concrete asset, giving it a new root location.
	pub fn prepend_location(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		if let AssetId::Concrete(ref mut l) = self {
			l.prepend_with(prepend.clone()).map_err(|_| ())?;
		}
		Ok(())
	}

	/// Use the value of `self` along with an `amount to create the corresponding `MultiAsset` value for a
	/// fungible asset.
	pub fn into_fungible_multiasset(self, amount: u128) -> MultiAsset {
		match self {
			AssetId::Concrete(id) => MultiAsset::ConcreteFungible { id, amount },
			AssetId::Abstract(id) => MultiAsset::AbstractFungible { id, amount },
		}
	}

	/// Use the value of `self` along with an `instance to create the corresponding `MultiAsset` value for a
	/// non-fungible asset.
	pub fn into_non_fungible_multiasset(self, instance: AssetInstance) -> MultiAsset {
		match self {
			AssetId::Concrete(class) => MultiAsset::ConcreteNonFungible { class, instance },
			AssetId::Abstract(class) => MultiAsset::AbstractNonFungible { class, instance },
		}
	}
}

/// List of non-wildcard fungible and non-fungible assets.
#[derive(Default, Clone, RuntimeDebug, Eq, PartialEq)]
pub struct Assets {
	/// The fungible assets.
	pub fungible: BTreeMap<AssetId, u128>,

	/// The non-fungible assets.
	// OPTIMIZE: Consider BTreeMap<AssetId, BTreeSet<AssetInstance>>
	//   or even BTreeMap<AssetId, SortedVec<AssetInstance>>
	pub non_fungible: BTreeSet<(AssetId, AssetInstance)>,
}

impl From<Vec<MultiAsset>> for Assets {
	fn from(assets: Vec<MultiAsset>) -> Assets {
		let mut result = Self::default();
		for asset in assets.into_iter() {
			result.saturating_subsume(asset)
		}
		result
	}
}

impl From<Assets> for Vec<MultiAsset> {
	fn from(a: Assets) -> Self {
		a.into_assets_iter().collect()
	}
}

impl From<MultiAsset> for Assets {
	fn from(asset: MultiAsset) -> Assets {
		let mut result = Self::default();
		result.saturating_subsume(asset);
		result
	}
}

impl Assets {
	/// New value, containing no assets.
	pub fn new() -> Self { Self::default() }

	/// Substitute all abstract `MultiAsset` values for equivalent concrete values.
	///
	/// If at least one of the values cannot be substituted, then return Err.
	pub fn concretize(&mut self) -> Result<(), ()> {
		todo!();
	}

	/// An iterator over the fungible assets.
	pub fn fungible_assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		self.fungible.iter()
			.map(|(id, &amount)| match id.clone() {
				AssetId::Concrete(id) => MultiAsset::ConcreteFungible { id, amount },
				AssetId::Abstract(id) => MultiAsset::AbstractFungible { id, amount },
			})
	}

	/// An iterator over the non-fungible assets.
	pub fn non_fungible_assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		self.non_fungible.iter()
			.map(|&(ref class, ref instance)| match class.clone() {
				AssetId::Concrete(class) => MultiAsset::ConcreteNonFungible { class, instance: instance.clone() },
				AssetId::Abstract(class) => MultiAsset::AbstractNonFungible { class, instance: instance.clone() },
			})
	}

	/// An iterator over all assets.
	pub fn into_assets_iter(self) -> impl Iterator<Item=MultiAsset> {
		let fungible = self.fungible.into_iter()
			.map(|(id, amount)| match id {
				AssetId::Concrete(id) => MultiAsset::ConcreteFungible { id, amount },
				AssetId::Abstract(id) => MultiAsset::AbstractFungible { id, amount },
			});
		let non_fungible = self.non_fungible.into_iter()
			.map(|(id, instance)| match id {
				AssetId::Concrete(class) => MultiAsset::ConcreteNonFungible { class, instance },
				AssetId::Abstract(class) => MultiAsset::AbstractNonFungible { class, instance },
			});
		fungible.chain(non_fungible)
	}

	/// An iterator over all assets.
	pub fn assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		let fungible = self.fungible_assets_iter();
		let non_fungible = self.non_fungible_assets_iter();
		fungible.chain(non_fungible)
	}

	/// Mutate `self` to contain all given `assets`, saturating if necessary.
	///
	/// Wildcards in `assets` are ignored.
	pub fn saturating_subsume_all(&mut self, assets: Assets) {
		// OPTIMIZE: Could be done with a much faster btree entry merge and only sum the entries with the
		// same key.
		for asset in assets.into_assets_iter() {
			self.saturating_subsume(asset)
		}
	}

	/// Mutate `self` to contain the given `asset`, saturating if necessary.
	///
	/// Wildcard values of `asset` do nothing.
	pub fn saturating_subsume(&mut self, asset: MultiAsset) {
		match asset {
			MultiAsset::ConcreteFungible { id, amount } => {
				self.saturating_subsume_fungible(AssetId::Concrete(id), amount);
			}
			MultiAsset::AbstractFungible { id, amount } => {
				self.saturating_subsume_fungible(AssetId::Abstract(id), amount);
			}
			MultiAsset::ConcreteNonFungible { class, instance} => {
				self.saturating_subsume_non_fungible(AssetId::Concrete(class), instance);
			}
			MultiAsset::AbstractNonFungible { class, instance} => {
				self.saturating_subsume_non_fungible(AssetId::Abstract(class), instance);
			}
			_ => (),
		}
	}

	/// Consumes `self` and returns its original value excluding `asset` iff it contains at least `asset`.
	///
	/// Wildcard assets in `self` will result in an error.
	///
	/// `asset` may be a wildcard and are evaluated in the context of `self`.
	///
	/// Returns `Ok` with the `self` minus `asset` and the non-wildcard equivalence of `asset` taken if `self`
	/// contains `asset`, and `Err` with `self` otherwise.
	pub fn less(mut self, asset: MultiAsset) -> Result<(Self, Assets), Self> {
		match self.try_take(asset) {
			Ok(taken) => Ok((self, taken)),
			Err(()) => Err(self),
		}
	}

	/// Mutates `self` to its original value less `asset` and returns `true` iff it contains at least `asset`.
	///
	/// Wildcard assets in `self` will result in an error.
	///
	/// `asset` may be a wildcard and are evaluated in the context of `self`.
	///
	/// Returns `Ok` with the non-wildcard equivalence of `asset` taken and mutates `self` to its value minus
	/// `asset` if `self` contains `asset`, and return `Err` otherwise.
	pub fn try_take(&mut self, asset: MultiAsset) -> Result<Assets, ()> {
		match asset {
			MultiAsset::None => Ok(Assets::new()),
			MultiAsset::ConcreteFungible { id, amount } => self.try_take_fungible(AssetId::Concrete(id), amount),
			MultiAsset::AbstractFungible { id, amount } => self.try_take_fungible(AssetId::Abstract(id), amount),
			MultiAsset::ConcreteNonFungible { class, instance} => self.try_take_non_fungible(AssetId::Concrete(class), instance),
			MultiAsset::AbstractNonFungible { class, instance} => self.try_take_non_fungible(AssetId::Abstract(class), instance),
			MultiAsset::AllAbstractFungible { id } => Ok(self.take_fungible(&AssetId::Abstract(id))),
			MultiAsset::AllConcreteFungible { id } => Ok(self.take_fungible(&AssetId::Concrete(id))),
			MultiAsset::AllAbstractNonFungible { class } => Ok(self.take_non_fungible(&AssetId::Abstract(class))),
			MultiAsset::AllConcreteNonFungible { class } => Ok(self.take_non_fungible(&AssetId::Concrete(class))),
			MultiAsset::AllFungible => {
				let mut taken = Assets::new();
				mem::swap(&mut self.fungible, &mut taken.fungible);
				Ok(taken)
			},
			MultiAsset::AllNonFungible => {
				let mut taken = Assets::new();
				mem::swap(&mut self.non_fungible, &mut taken.non_fungible);
				Ok(taken)
			},
			MultiAsset::All => Ok(self.swapped(Assets::new())),
		}
	}

	pub fn try_take_fungible(&mut self, id: AssetId, amount: u128) -> Result<Assets, ()> {
		self.try_remove_fungible(&id, amount)?;
		Ok(id.into_fungible_multiasset(amount).into())
	}

	pub fn try_take_non_fungible(&mut self, id: AssetId, instance: AssetInstance) -> Result<Assets, ()> {
		let asset_id_instance = (id, instance);
		self.try_remove_non_fungible(&asset_id_instance)?;
		let (asset_id, instance) = asset_id_instance;
		Ok(asset_id.into_non_fungible_multiasset(instance).into())
	}

	pub fn take_fungible(&mut self, id: &AssetId) -> Assets {
		let mut taken = Assets::new();
		if let Some((id, amount)) = self.fungible.remove_entry(&id) {
			taken.fungible.insert(id, amount);
		}
		taken
	}

	pub fn take_non_fungible(&mut self, id: &AssetId) -> Assets {
		let mut taken = Assets::new();
		let non_fungible = mem::replace(&mut self.non_fungible, Default::default());
		non_fungible.into_iter().for_each(|(c, instance)| {
			if &c == id {
				taken.non_fungible.insert((c, instance));
			} else {
				self.non_fungible.insert((c, instance));
			}
		});
		taken
	}

	pub fn try_remove_fungible(&mut self, id: &AssetId, amount: u128) -> Result<(), ()> {
		let self_amount = self.fungible.get_mut(&id).ok_or(())?;
		*self_amount = self_amount.checked_sub(amount).ok_or(())?;
		Ok(())
	}

	pub fn try_remove_non_fungible(&mut self, class_instance: &(AssetId, AssetInstance)) -> Result<(), ()> {
		match self.non_fungible.remove(class_instance) {
			true => Ok(()),
			false => Err(()),
		}
	}

	/// Modify `self` to include a new fungible asset by `id` and `amount`,
	/// saturating if necessary.
	pub fn saturating_subsume_fungible(&mut self, id: AssetId, amount: u128) {
		self.fungible
			.entry(id)
			.and_modify(|e| *e = e.saturating_add(amount))
			.or_insert(amount);
	}

	/// Modify `self` to include a new non-fungible asset by `class` and `instance`.
	pub fn saturating_subsume_non_fungible(&mut self, class: AssetId, instance: AssetInstance) {
		self.non_fungible.insert((class, instance));
	}

	/// Alter any concretely identified assets by prepending the given `MultiLocation`.
	///
	/// WARNING: For now we consider this infallible and swallow any errors. It is thus the caller's responsibility to
	/// ensure that any internal asset IDs are able to be prepended without overflow.
	pub fn prepend_location(&mut self, prepend: &MultiLocation) {
		let mut fungible = Default::default();
		mem::swap(&mut self.fungible, &mut fungible);
		self.fungible = fungible.into_iter()
			.map(|(mut id, amount)| { let _ = id.prepend_location(prepend); (id, amount) })
			.collect();
		let mut non_fungible = Default::default();
		mem::swap(&mut self.non_fungible, &mut non_fungible);
		self.non_fungible = non_fungible.into_iter()
			.map(|(mut class, inst)| { let _ = class.prepend_location(prepend); (class, inst) })
			.collect();
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
	pub fn min<'a, M, I>(&self, assets: I) -> Self
	where
		M: 'a + sp_std::borrow::Borrow<MultiAsset>,
		I: IntoIterator<Item = M>,
	{
		let mut result = Assets::default();
		for asset in assets.into_iter() {
			match asset.borrow() {
				MultiAsset::None => (),
				MultiAsset::All => return self.clone(),
				MultiAsset::AllFungible => {
					// Replace `result.fungible` with all fungible assets,
					// keeping `result.non_fungible` the same.
					result = Assets {
						fungible: self.fungible.clone(),
						non_fungible: result.non_fungible,
					}
				},
				MultiAsset::AllNonFungible => {
					// Replace `result.non_fungible` with all non-fungible assets,
					// keeping `result.fungible` the same.
					result = Assets {
						fungible: result.fungible,
						non_fungible: self.non_fungible.clone(),
					}
				},
				MultiAsset::AllAbstractFungible { id } => {
					for asset in self.fungible_assets_iter() {
						match &asset {
							MultiAsset::AbstractFungible { id: identifier, .. } => {
								if id == identifier { result.saturating_subsume(asset) }
							},
							_ => (),
						}
					}
				},
				MultiAsset::AllAbstractNonFungible { class } => {
					for asset in self.non_fungible_assets_iter() {
						match &asset {
							MultiAsset::AbstractNonFungible { class: c, .. } => {
								if class == c { result.saturating_subsume(asset) }
							},
							_ => (),
						}
					}
				}
				MultiAsset::AllConcreteFungible { id } => {
					for asset in self.fungible_assets_iter() {
						match &asset {
							MultiAsset::ConcreteFungible { id: identifier, .. } => {
								if id == identifier { result.saturating_subsume(asset) }
							},
							_ => (),
						}
					}
				},
				MultiAsset::AllConcreteNonFungible { class } => {
					for asset in self.non_fungible_assets_iter() {
						match &asset {
							MultiAsset::ConcreteNonFungible { class: c, .. } => {
								if class == c { result.saturating_subsume(asset) }
							},
							_ => (),
						}
					}
				}
				x @ MultiAsset::ConcreteFungible { .. } | x @ MultiAsset::AbstractFungible { .. } => {
					let (id, amount) = match x {
						MultiAsset::ConcreteFungible { id, amount } => (AssetId::Concrete(id.clone()), *amount),
						MultiAsset::AbstractFungible { id, amount } => (AssetId::Abstract(id.clone()), *amount),
						_ => unreachable!(),
					};
					if let Some(v) = self.fungible.get(&id) {
						result.saturating_subsume_fungible(id, amount.min(*v));
					}
				},
				x @ MultiAsset::ConcreteNonFungible { .. } | x @ MultiAsset::AbstractNonFungible { .. } => {
					let (class, instance) = match x {
						MultiAsset::ConcreteNonFungible { class, instance } => (AssetId::Concrete(class.clone()), instance.clone()),
						MultiAsset::AbstractNonFungible { class, instance } => (AssetId::Abstract(class.clone()), instance.clone()),
						_ => unreachable!(),
					};
					let item = (class, instance);
					if self.non_fungible.contains(&item) {
						result.non_fungible.insert(item);
					}
				}
			}
		}
		result
	}

	/// Take all possible assets up to `assets` from `self`, mutating `self` and returning the
	/// assets taken.
	///
	/// Wildcards work.
	///
	/// Example:
	///
	/// ```
	/// use xcm_executor::Assets;
	/// use xcm::v0::{MultiAsset, MultiLocation};
	/// let mut assets_i_have: Assets = vec![
	/// 	MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount: 100 },
	/// 	MultiAsset::AbstractFungible { id: vec![0], amount: 100 },
	/// ].into();
	/// let assets_they_want = vec![
	/// 	MultiAsset::AllAbstractFungible { id: vec![0] },
	/// ];
	///
	/// let assets_they_took: Assets = assets_i_have.saturating_take(assets_they_want);
	/// assert_eq!(assets_they_took.into_assets_iter().collect::<Vec<_>>(), vec![
	/// 	MultiAsset::AbstractFungible { id: vec![0], amount: 100 },
	/// ]);
	/// assert_eq!(assets_i_have.into_assets_iter().collect::<Vec<_>>(), vec![
	/// 	MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount: 100 },
	/// ]);
	/// ```
	pub fn saturating_take<I>(&mut self, assets: I) -> Assets
	where
		I: IntoIterator<Item = MultiAsset>,
	{
		let mut result = Assets::default();
		for asset in assets.into_iter() {
			match asset {
				MultiAsset::None => (),
				MultiAsset::All => return self.swapped(Assets::default()),
				MultiAsset::AllFungible => {
					// Remove all fungible assets, and copy them into `result`.
					let fungible = mem::replace(&mut self.fungible, Default::default());
					fungible.into_iter().for_each(|(id, amount)| {
						result.saturating_subsume_fungible(id, amount);
					})
				},
				MultiAsset::AllNonFungible => {
					// Remove all non-fungible assets, and copy them into `result`.
					let non_fungible = mem::replace(&mut self.non_fungible, Default::default());
					non_fungible.into_iter().for_each(|(class, instance)| {
						result.saturating_subsume_non_fungible(class, instance);
					});
				},
				x @ MultiAsset::AllAbstractFungible { .. } | x @ MultiAsset::AllConcreteFungible { .. } => {
					let id = match x {
						MultiAsset::AllConcreteFungible { id } => AssetId::Concrete(id),
						MultiAsset::AllAbstractFungible { id } => AssetId::Abstract(id),
						_ => unreachable!(),
					};
					// At the end of this block, we will be left with only the non-matching fungibles.
					let mut non_matching_fungibles = BTreeMap::<AssetId, u128>::new();
					let fungible = mem::replace(&mut self.fungible, Default::default());
					fungible.into_iter().for_each(|(iden, amount)| {
							if iden == id {
								result.saturating_subsume_fungible(iden, amount);
							} else {
								non_matching_fungibles.insert(iden, amount);
							}
						});
					self.fungible = non_matching_fungibles;
				},
				x @ MultiAsset::AllAbstractNonFungible { .. } | x @ MultiAsset::AllConcreteNonFungible { .. } => {
					let class = match x {
						MultiAsset::AllConcreteNonFungible { class } => AssetId::Concrete(class),
						MultiAsset::AllAbstractNonFungible { class } => AssetId::Abstract(class),
						_ => unreachable!(),
					};
					// At the end of this block, we will be left with only the non-matching non-fungibles.
					let mut non_matching_non_fungibles = BTreeSet::<(AssetId, AssetInstance)>::new();
					let non_fungible = mem::replace(&mut self.non_fungible, Default::default());
					non_fungible.into_iter().for_each(|(c, instance)| {
							if class == c {
								result.saturating_subsume_non_fungible(c, instance);
							} else {
								non_matching_non_fungibles.insert((c, instance));
							}
						});
					self.non_fungible = non_matching_non_fungibles;
				},
				x @ MultiAsset::ConcreteFungible {..} | x @ MultiAsset::AbstractFungible {..} => {
					let (id, amount) = match x {
						MultiAsset::ConcreteFungible { id, amount } => (AssetId::Concrete(id), amount),
						MultiAsset::AbstractFungible { id, amount } => (AssetId::Abstract(id), amount),
						_ => unreachable!(),
					};
					// remove the maxmimum possible up to id/amount from self, add the removed onto
					// result
					let maybe_value = self.fungible.get(&id);
					if let Some(&e) = maybe_value {
						if e > amount {
							self.fungible.insert(id.clone(), e - amount);
							result.saturating_subsume_fungible(id, amount);
						} else {
							self.fungible.remove(&id);
							result.saturating_subsume_fungible(id, e.clone());
						}
					}
				}
				x @ MultiAsset::ConcreteNonFungible {..} | x @ MultiAsset::AbstractNonFungible {..} => {
					let (class, instance) = match x {
						MultiAsset::ConcreteNonFungible { class, instance } => (AssetId::Concrete(class), instance),
						MultiAsset::AbstractNonFungible { class, instance } => (AssetId::Abstract(class), instance),
						_ => unreachable!(),
					};
					// remove the maxmimum possible up to id/amount from self, add the removed onto
					// result
					if let Some(entry) = self.non_fungible.take(&(class, instance)) {
						result.non_fungible.insert(entry);
					}
				}
			}
		}
		result
	}

	/// Swaps two mutable Assets, without deinitializing either one.
	pub fn swapped(&mut self, mut with: Assets) -> Self {
		mem::swap(&mut *self, &mut with);
		with
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[allow(non_snake_case)]
	fn AF(id: u8, amount: u128) -> MultiAsset {
		MultiAsset::AbstractFungible { id: vec![id], amount }
	}
	#[allow(non_snake_case)]
	fn ANF(class: u8, instance_id: u128) -> MultiAsset {
		MultiAsset::AbstractNonFungible { class: vec![class], instance: AssetInstance::Index { id: instance_id } }
	}
	#[allow(non_snake_case)]
	fn CF(amount: u128) -> MultiAsset {
		MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount }
	}
	#[allow(non_snake_case)]
	fn CNF(instance_id: u128) -> MultiAsset {
		MultiAsset::ConcreteNonFungible { class: MultiLocation::Null, instance: AssetInstance::Index { id: instance_id } }
	}

	fn test_assets() -> Assets {
		let mut assets_vec: Vec<MultiAsset> = Vec::new();
		assets_vec.push(AF(1, 100));
		assets_vec.push(ANF(2, 200));
		assets_vec.push(CF(300));
		assets_vec.push(CNF(400));
		assets_vec.into()
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
		let none = vec![MultiAsset::None];
		let all = vec![MultiAsset::All];

		let none_min = assets.min(none.iter());
		assert_eq!(None, none_min.assets_iter().next());
		let all_min = assets.min(all.iter());
		assert!(all_min.assets_iter().eq(assets.assets_iter()));
	}

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
	}
}
