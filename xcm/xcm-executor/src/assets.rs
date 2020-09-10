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
use sp_runtime::traits::Saturating;
use xcm::v0::{MultiAsset, MultiLocation, AssetInstance};

pub enum AssetId {
	Concrete(MultiLocation),
	Abstract(Vec<u8>),
}

#[derive(Default, Clone)]
pub struct Assets {
	pub fungible: BTreeMap<AssetId, u128>,
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

impl Assets {
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
			MultiAsset::Each(ref assets) => {
				for asset in assets.into_iter() {
					self.saturating_subsume(asset.clone())
				}
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
