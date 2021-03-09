// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate Finality Verifier Pallet
//!
//! The goal of this pallet is to provide a safe interface for writing finalized headers to an
//! external pallet which tracks headers and finality proofs. By safe, we mean that only headers
//! whose finality has been verified will be written to the underlying pallet.
//!
//! By verifying the finality of headers before writing them to storage we prevent DoS vectors in
//! which unfinalized headers get written to storage even if they don't have a chance of being
//! finalized in the future (such as in the case where a different fork gets finalized).
//!
//! The underlying pallet used for storage is assumed to be a pallet which tracks headers and
//! GRANDPA authority set changes. This information is used during the verification of GRANDPA
//! finality proofs.

#![cfg_attr(not(feature = "std"), no_std)]
// Runtime-generated enums
#![allow(clippy::large_enum_variant)]

use bp_header_chain::{justification::verify_justification, AncestryChecker, HeaderChain};
use bp_runtime::{Chain, HeaderOf};
use finality_grandpa::voter_set::VoterSet;
use frame_support::{dispatch::DispatchError, ensure};
use frame_system::ensure_signed;
use sp_runtime::traits::Header as HeaderT;
use sp_std::vec::Vec;

#[cfg(test)]
mod mock;

// Re-export in crate namespace for `construct_runtime!`
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	/// Header of the bridged chain.
	pub(crate) type BridgedHeader<T> = HeaderOf<<T as Config>::BridgedChain>;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The chain we are bridging to here.
		type BridgedChain: Chain;

		/// The pallet which we will use as our underlying storage mechanism.
		type HeaderChain: HeaderChain<<Self::BridgedChain as Chain>::Header, DispatchError>;

		/// The type of ancestry proof used by the pallet.
		///
		/// Will be used by the ancestry checker to verify that the header being finalized is
		/// related to the best finalized header in storage.
		type AncestryProof: Parameter;

		/// The type through which we will verify that a given header is related to the last
		/// finalized header in our storage pallet.
		type AncestryChecker: AncestryChecker<<Self::BridgedChain as Chain>::Header, Self::AncestryProof>;

		/// The upper bound on the number of requests allowed by the pallet.
		///
		/// Once this bound is reached the pallet will not allow any dispatchables to be called
		/// until the request count has decreased.
		#[pallet::constant]
		type MaxRequests: Get<u32>;
	}

	#[pallet::pallet]
	pub struct Pallet<T>(PhantomData<T>);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_n: T::BlockNumber) -> frame_support::weights::Weight {
			<RequestCount<T>>::mutate(|count| *count = count.saturating_sub(1));

			(0_u64)
				.saturating_add(T::DbWeight::get().reads(1))
				.saturating_add(T::DbWeight::get().writes(1))
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Verify a target header is finalized according to the given finality proof.
		///
		/// It will use the underlying storage pallet to fetch information about the current
		/// authorities and best finalized header in order to verify that the header is finalized.
		///
		/// If successful in verification, it will write the target header to the underlying storage
		/// pallet.
		#[pallet::weight(0)]
		pub fn submit_finality_proof(
			origin: OriginFor<T>,
			finality_target: BridgedHeader<T>,
			justification: Vec<u8>,
			ancestry_proof: T::AncestryProof,
		) -> DispatchResultWithPostInfo {
			let _ = ensure_signed(origin)?;

			ensure!(
				Self::request_count() < T::MaxRequests::get(),
				<Error<T>>::TooManyRequests
			);
			<RequestCount<T>>::mutate(|count| *count += 1);

			frame_support::debug::trace!("Going to try and finalize header {:?}", finality_target);

			let authority_set = T::HeaderChain::authority_set();
			let voter_set = VoterSet::new(authority_set.authorities).ok_or(<Error<T>>::InvalidAuthoritySet)?;
			let set_id = authority_set.set_id;

			let (hash, number) = (finality_target.hash(), *finality_target.number());
			verify_justification::<BridgedHeader<T>>((hash, number), set_id, voter_set, &justification).map_err(
				|e| {
					frame_support::debug::error!("Received invalid justification for {:?}: {:?}", finality_target, e);
					<Error<T>>::InvalidJustification
				},
			)?;

			let best_finalized = T::HeaderChain::best_finalized();
			frame_support::debug::trace!("Checking ancestry against best finalized header: {:?}", &best_finalized);

			ensure!(
				T::AncestryChecker::are_ancestors(&best_finalized, &finality_target, &ancestry_proof),
				<Error<T>>::InvalidAncestryProof
			);

			T::HeaderChain::append_header(finality_target);
			frame_support::debug::info!("Succesfully imported finalized header with hash {:?}!", hash);

			Ok(().into())
		}
	}

	/// The current number of requests for calling dispatchables.
	///
	/// If the `RequestCount` hits `MaxRequests`, no more calls will be allowed to the pallet until
	/// the request capacity is increased.
	///
	/// The `RequestCount` is decreased by one at the beginning of every block. This is to ensure
	/// that the pallet can always make progress.
	#[pallet::storage]
	#[pallet::getter(fn request_count)]
	pub(super) type RequestCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	#[pallet::error]
	pub enum Error<T> {
		/// The given justification is invalid for the given header.
		InvalidJustification,
		/// The given ancestry proof is unable to verify that the child and ancestor headers are
		/// related.
		InvalidAncestryProof,
		/// The authority set from the underlying header chain is invalid.
		InvalidAuthoritySet,
		/// Failed to write a header to the underlying header chain.
		FailedToWriteHeader,
		/// There are too many requests for the current window to handle.
		TooManyRequests,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{run_test, test_header, Origin, TestRuntime};
	use bp_test_utils::{authority_list, make_justification_for_header};
	use codec::Encode;
	use frame_support::{assert_err, assert_ok};

	fn initialize_substrate_bridge() {
		let genesis = test_header(0);

		let init_data = pallet_substrate_bridge::InitializationData {
			header: genesis,
			authority_list: authority_list(),
			set_id: 1,
			scheduled_change: None,
			is_halted: false,
		};

		assert_ok!(pallet_substrate_bridge::Module::<TestRuntime>::initialize(
			Origin::root(),
			init_data
		));
	}

	fn submit_finality_proof() -> frame_support::dispatch::DispatchResultWithPostInfo {
		let child = test_header(1);
		let header = test_header(2);

		let set_id = 1;
		let grandpa_round = 1;
		let justification = make_justification_for_header(&header, grandpa_round, set_id, &authority_list()).encode();
		let ancestry_proof = vec![child, header.clone()];

		Module::<TestRuntime>::submit_finality_proof(Origin::signed(1), header, justification, ancestry_proof)
	}

	fn next_block() {
		use frame_support::traits::OnInitialize;

		let current_number = frame_system::Module::<TestRuntime>::block_number();
		frame_system::Module::<TestRuntime>::set_block_number(current_number + 1);
		let _ = Module::<TestRuntime>::on_initialize(current_number);
	}

	#[test]
	fn succesfully_imports_header_with_valid_finality_and_ancestry_proofs() {
		run_test(|| {
			initialize_substrate_bridge();

			assert_ok!(submit_finality_proof());

			let header = test_header(2);
			assert_eq!(
				pallet_substrate_bridge::Module::<TestRuntime>::best_headers(),
				vec![(*header.number(), header.hash())]
			);

			assert_eq!(pallet_substrate_bridge::Module::<TestRuntime>::best_finalized(), header);
		})
	}

	#[test]
	fn rejects_justification_that_skips_authority_set_transition() {
		run_test(|| {
			initialize_substrate_bridge();

			let child = test_header(1);
			let header = test_header(2);

			let set_id = 2;
			let grandpa_round = 1;
			let justification =
				make_justification_for_header(&header, grandpa_round, set_id, &authority_list()).encode();
			let ancestry_proof = vec![child, header.clone()];

			assert_err!(
				Module::<TestRuntime>::submit_finality_proof(Origin::signed(1), header, justification, ancestry_proof,),
				<Error<TestRuntime>>::InvalidJustification
			);
		})
	}

	#[test]
	fn does_not_import_header_with_invalid_finality_proof() {
		run_test(|| {
			initialize_substrate_bridge();

			let child = test_header(1);
			let header = test_header(2);

			let justification = [1u8; 32].encode();
			let ancestry_proof = vec![child, header.clone()];

			assert_err!(
				Module::<TestRuntime>::submit_finality_proof(Origin::signed(1), header, justification, ancestry_proof,),
				<Error<TestRuntime>>::InvalidJustification
			);
		})
	}

	#[test]
	fn does_not_import_header_with_invalid_ancestry_proof() {
		run_test(|| {
			initialize_substrate_bridge();

			let header = test_header(2);

			let set_id = 1;
			let grandpa_round = 1;
			let justification =
				make_justification_for_header(&header, grandpa_round, set_id, &authority_list()).encode();

			// For testing, we've made it so that an empty ancestry proof is invalid
			let ancestry_proof = vec![];

			assert_err!(
				Module::<TestRuntime>::submit_finality_proof(Origin::signed(1), header, justification, ancestry_proof,),
				<Error<TestRuntime>>::InvalidAncestryProof
			);
		})
	}

	#[test]
	fn disallows_invalid_authority_set() {
		run_test(|| {
			use bp_test_utils::{alice, bob};

			let genesis = test_header(0);

			let invalid_authority_list = vec![(alice(), u64::MAX), (bob(), u64::MAX)];
			let init_data = pallet_substrate_bridge::InitializationData {
				header: genesis,
				authority_list: invalid_authority_list,
				set_id: 1,
				scheduled_change: None,
				is_halted: false,
			};

			assert_ok!(pallet_substrate_bridge::Module::<TestRuntime>::initialize(
				Origin::root(),
				init_data
			));

			let header = test_header(1);
			let justification = [1u8; 32].encode();
			let ancestry_proof = vec![];

			assert_err!(
				Module::<TestRuntime>::submit_finality_proof(Origin::signed(1), header, justification, ancestry_proof,),
				<Error<TestRuntime>>::InvalidAuthoritySet
			);
		})
	}

	#[test]
	fn disallows_imports_once_limit_is_hit_in_single_block() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof());
			assert_ok!(submit_finality_proof());
			assert_err!(submit_finality_proof(), <Error<TestRuntime>>::TooManyRequests);
		})
	}

	#[test]
	fn allows_request_after_new_block_has_started() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof());
			assert_ok!(submit_finality_proof());

			next_block();
			assert_ok!(submit_finality_proof());
		})
	}

	#[test]
	fn disallows_imports_once_limit_is_hit_across_different_blocks() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof());
			assert_ok!(submit_finality_proof());

			next_block();
			assert_ok!(submit_finality_proof());
			assert_err!(submit_finality_proof(), <Error<TestRuntime>>::TooManyRequests);
		})
	}

	#[test]
	fn allows_max_requests_after_long_time_with_no_activity() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof());
			assert_ok!(submit_finality_proof());

			next_block();
			next_block();

			next_block();
			assert_ok!(submit_finality_proof());
			assert_ok!(submit_finality_proof());
		})
	}
}
