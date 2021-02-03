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

//! A pallet responsible for creating Merkle Mountain Range (MMR) leaf for current block.

use beefy_primitives::ValidatorSetId;
use sp_core::H256;
use sp_std::prelude::*;
use frame_support::{decl_module, decl_storage, RuntimeDebug,};
use pallet_mmr::primitives::LeafDataProvider;
use parity_scale_codec::{Encode, Decode};
use runtime_parachains::paras;

/// A leaf that gets added every block to the MMR constructed by [pallet_mmr].
#[derive(RuntimeDebug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct MmrLeaf<BlockNumber, Hash, MerkleRoot> {
	/// Current block parent number and hash.
	pub parent_number_and_hash: (BlockNumber, Hash),
	/// A merkle root of all registered parachain heads.
	pub parachain_heads: MerkleRoot,
	/// A merkle root of the next BEEFY authority set.
	pub beefy_next_authority_set: (ValidatorSetId, MerkleRoot),
}

type MerkleRootOf<T> = <T as pallet_mmr::Config>::Hash;

/// The module's configuration trait.
pub trait Config: pallet_mmr::Config + paras::Config + pallet_beefy::Config
{}

/// Blanket-impl the trait for every runtime that has both MMR pallet and parachains configuration.
///
/// NOTE Remember that you still need to register the [Module] in `construct_runtime` macro.
impl<R: pallet_mmr::Config + paras::Config + pallet_beefy::Config> Config for R {}

decl_storage! {
	trait Store for Module<T: Config> as Beefy {
		/// The merkle root of the next BEEFY authority set.
		pub BeefyNextAuthoritiesRoot get(fn beefy_next_authorities_root): MerkleRootOf<T>;
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
	}
}

impl<T: Config> LeafDataProvider for Module<T> where
	MerkleRootOf<T>: From<H256>,
{
	type LeafData = MmrLeaf<
		<T as frame_system::Config>::BlockNumber,
		<T as frame_system::Config>::Hash,
		MerkleRootOf<T>,
	>;

	fn leaf_data() -> Self::LeafData {
		MmrLeaf {
			parent_number_and_hash: frame_system::Module::<T>::leaf_data(),
			parachain_heads: Module::<T>::parachain_heads_merkle_root(),
			beefy_next_authority_set: (
				Module::<T>::beefy_next_authority_set_id(),
				Module::<T>::beefy_next_authority_set_merkle_root()
			),
		}
	}
}

impl<T: Config> Module<T> where
	MerkleRootOf<T>: From<H256>,
{
	/// Returns latest root hash of a merkle tree constructed from all registered parachain headers.
	///
	/// NOTE this does not include parathreads - only parachains are part of the merkle tree.
	///
	/// TODO [ToDr] describe merkle tree construction.
	///
	/// NOTE This is an initial and inefficient implementation, which re-constructs
	/// the merkle tree every block. Instead we should update the merkle root in [Self::on_initialize]
	/// call of this pallet and update the merkle tree efficiently (use on-chain storage to persist inner nodes).
	fn parachain_heads_merkle_root() -> MerkleRootOf<T> {
		let para_heads = paras::Module::<T>::parachains()
			.into_iter()
			.map(paras::Module::<T>::para_head)
			.map(|maybe_para_head| maybe_para_head.encode())
			.collect::<Vec<_>>();

		sp_io::trie::keccak_256_ordered_root(para_heads).into()
	}

	/// Returns a merkle root of a tree constructed from secp256k1 public keys of current BEEFY authority set.
	///
	/// NOTE This is an initial and inefficient implementation, which re-constructs
	/// the merkle tree every block. Instead we should update the merkle root in [on_new_session]
	/// callback, cause we know it will only change every session - in future it should be optimized
	/// to change every era instead.
	fn beefy_next_authority_set_merkle_root() -> MerkleRootOf<T> {
		Self::beefy_next_authorities_root()
	}

	fn beefy_next_authority_set_id() -> ValidatorSetId {
		pallet_beefy::Module::<T>::validator_set_id() + 1
	}

	fn update_beefy_next_authorities() {
		let beefy_public_keys = pallet_beefy::Module::<T>::next_authorities()
			.into_iter()
			.map(|authority_id| authority_id.encode())
			.collect::<Vec<_>>();
		let merkle_root: MerkleRootOf<T> = sp_io::trie
			::keccak_256_ordered_root(beefy_public_keys).into();
		BeefyNextAuthoritiesRoot::<T>::put(merkle_root)
	}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = <T as pallet_beefy::Config>::AuthorityId;
}

impl<T: Config> frame_support::traits::OneSessionHandler<T::AccountId> for Module<T> where
	T: pallet_session::Config,
	MerkleRootOf<T>: From<H256>,
{
	type Key = <T as pallet_beefy::Config>::AuthorityId;

	fn on_genesis_session<'a, I: 'a>(_validators: I) where
		I: Iterator<Item=(&'a T::AccountId, Self::Key)>, Self::Key: 'a
	{
		Self::update_beefy_next_authorities();
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, _validators: I, _queued_validators: I) where
		I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		Self::update_beefy_next_authorities();
	}

	fn on_disabled(_validator_index: usize) {}
}
