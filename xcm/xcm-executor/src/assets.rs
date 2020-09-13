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

use sp_std::{prelude::*, mem::swap, collections::{btree_map::BTreeMap, btree_set::BTreeSet}};
use xcm::v0::{MultiAsset, MultiLocation, AssetInstance};
use sp_runtime::RuntimeDebug;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, RuntimeDebug)]
pub enum AssetId {
	Concrete(MultiLocation),
	Abstract(Vec<u8>),
}

impl AssetId {
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		if let AssetId::Concrete(ref mut l) = self {
			l.prepend_with(prepend.clone()).map_err(|_| ())?;
		}
		Ok(())
	}
}

#[derive(Default, Clone, RuntimeDebug)]
pub struct Assets {
	pub fungible: BTreeMap<AssetId, u128>,
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

impl Assets {
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

	pub fn assets_iter<'a>(&'a self) -> impl Iterator<Item=MultiAsset> + 'a {
		let fungible = self.fungible.iter()
			.map(|(id, &amount)| match id.clone() {
				AssetId::Concrete(id) => MultiAsset::ConcreteFungible { id, amount },
				AssetId::Abstract(id) => MultiAsset::AbstractFungible { id, amount },
			});
		let non_fungible = self.non_fungible.iter()
			.map(|&(ref class, ref instance)| match class.clone() {
				AssetId::Concrete(class) => MultiAsset::ConcreteNonFungible { class, instance: instance.clone() },
				AssetId::Abstract(class) => MultiAsset::AbstractNonFungible { class, instance: instance.clone() },
			});
		fungible.chain(non_fungible)
	}

	/// Modify `self` to include `MultiAsset`, saturating if necessary.
	pub fn saturating_subsume(&mut self, asset: MultiAsset) {
		match asset {
			MultiAsset::ConcreteFungible { id, amount } => {
				self.fungible
					.entry(AssetId::Concrete(id))
					.and_modify(|e| *e = e.saturating_add(amount))
					.or_insert(amount);
			}
			MultiAsset::AbstractFungible { id, amount } => {
				self.fungible
					.entry(AssetId::Abstract(id))
					.and_modify(|e| *e = e.saturating_add(amount))
					.or_insert(amount);
			}
			MultiAsset::ConcreteNonFungible { class, instance} => {
				self.non_fungible.insert((AssetId::Concrete(class), instance));
			}
			MultiAsset::AbstractNonFungible { class, instance} => {
				self.non_fungible.insert((AssetId::Abstract(class), instance));
			}
			_ => (),
		}
	}

	pub fn saturating_subsume_fungible(&mut self, id: AssetId, amount: u128) {
		self.fungible
			.entry(id)
			.and_modify(|e| *e = e.saturating_add(amount))
			.or_insert(amount);
	}

	pub fn saturating_subsume_non_fungible(&mut self, class: AssetId, instance: AssetInstance) {
		self.non_fungible.insert((class, instance));
	}

	/// Alter any concretely identified assets according to the given `MultiLocation`.
	///
	/// WARNING: For now we consider this infallible and swallow any errors. It is thus the caller's responsibility to
	/// ensure that any internal asset IDs are able to be prepended without overflow.
	pub fn reanchor(&mut self, prepend: &MultiLocation) {
		let mut fungible = Default::default();
		sp_std::mem::swap(&mut self.fungible, &mut fungible);
		self.fungible = fungible.into_iter()
			.map(|(mut id, amount)| { let _ = id.reanchor(prepend); (id, amount) })
			.collect();
		let mut non_fungible = Default::default();
		sp_std::mem::swap(&mut self.non_fungible, &mut non_fungible);
		self.non_fungible = non_fungible.into_iter()
			.map(|(mut class, inst)| { let _ = class.reanchor(prepend); (class, inst) })
			.collect();
	}

	/// Return the assets in `self`, but (asset-wise) of no greater value than `assets`.
	///
	/// Result is undefined if `assets` includes elements which match to the same asset more than once.
	pub fn min<'a, I: Iterator<Item=&'a MultiAsset>>(&self, assets: I) -> Self {
		let mut result = Assets::default();
		for asset in assets.into_iter() {
			match asset {
				MultiAsset::None => (),
				MultiAsset::All => return self.clone(),
				x @ MultiAsset::ConcreteFungible { .. } | x @ MultiAsset::AbstractFungible { .. } => {
					let (id, amount) = match x {
						MultiAsset::ConcreteFungible { id, amount } => (AssetId::Concrete(id.clone()), *amount),
						MultiAsset::AbstractFungible { id, amount } => (AssetId::Abstract(id.clone()), *amount),
						_ => unreachable!(),
					};
					if let Some(v) = self.fungible.get(&id) {
						result.saturating_subsume_fungible(id, amount.max(*v));
					}
				}
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
				// TODO: implement partial wildcards.
				_ => (),
				// MultiAsset::AllFungible
				// | MultiAsset::AllNonFungible
				// | MultiAsset::AllAbstractFungible { id }
				// | MultiAsset::AllAbstractNonFungible { class }
				// | MultiAsset::AllConcreteFungible { id }
				// | MultiAsset::AllConcreteNonFungible { class } => (),
			}
		}
		result
	}

	/// Take all possible assets up to `assets` from `self`, mutating `self` and returning the
	/// assets taken.
	///
	/// Wildcards work.
	pub fn saturating_take(&mut self, assets: Vec<MultiAsset>) -> Assets {
		let mut result = Assets::default();
		for asset in assets.into_iter() {
			match asset {
				MultiAsset::None => (),
				MultiAsset::All => return self.swapped(Assets::default()),
				x @ MultiAsset::ConcreteFungible {..} | x @ MultiAsset::AbstractFungible {..} => {
					let (id, amount) = match x {
						MultiAsset::ConcreteFungible { id, amount } => (AssetId::Concrete(id), amount),
						MultiAsset::AbstractFungible { id, amount } => (AssetId::Abstract(id), amount),
						_ => unreachable!(),
					};
					// remove the maxmimum possible up to id/amount from self, add the removed onto
					// result
					self.fungible.entry(id.clone())
						.and_modify(|e| if *e >= amount {
							result.saturating_subsume_fungible(id, amount);
							*e = *e - amount;
						} else {
							result.saturating_subsume_fungible(id, *e);
							*e = 0
						});
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
						self.non_fungible.insert(entry);
					}
				}
				// TODO: implement partial wildcards.
				_ => {
					Default::default()
				}
				// MultiAsset::AllFungible
				// | MultiAsset::AllNonFungible
				// | MultiAsset::AllAbstractFungible { id }
				// | MultiAsset::AllAbstractNonFungible { class }
				// | MultiAsset::AllConcreteFungible { id }
				// | MultiAsset::AllConcreteNonFungible { class } => (),
			}
		}
		result
	}

	pub fn swapped(&mut self, mut with: Assets) -> Self {
		swap(&mut *self, &mut with);
		with
	}
}
