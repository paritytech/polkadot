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

//! Provides glue code over the scheduler and inclusion modules, and accepting
//! one inherent per block that can include new para candidates and bitfields.
//!
//! Unlike other modules in this crate, it does not need to be initialized by the initializer,
//! as it has no initialization logic and its finalization logic depends only on the details of
//! this module.

use sp_std::prelude::*;
use primitives::v1::{
	BackedCandidate, SignedAvailabilityBitfields, INCLUSION_INHERENT_IDENTIFIER,
};
use frame_support::{
	decl_error, decl_module, decl_storage, ensure,
	dispatch::DispatchResultWithPostInfo,
	weights::{DispatchClass, Weight},
	traits::Get,
};
use frame_system::ensure_none;
use crate::{
	inclusion,
	scheduler::{self, FreedReason},
	ump,
};
use inherents::{InherentIdentifier, InherentData, MakeFatalError, ProvideInherent};

// In the future, we should benchmark these consts; these are all untested assumptions for now.
const BACKED_CANDIDATE_WEIGHT: Weight = 100_000;
const INCLUSION_INHERENT_CLAIMED_WEIGHT: Weight = 1_000_000_000;
// we assume that 75% of an inclusion inherent's weight is used processing backed candidates
const MINIMAL_INCLUSION_INHERENT_WEIGHT: Weight = INCLUSION_INHERENT_CLAIMED_WEIGHT / 4;

pub trait Config: inclusion::Config + scheduler::Config {}

decl_storage! {
	trait Store for Module<T: Config> as ParaInclusionInherent {
		/// Whether the inclusion inherent was included within this block.
		///
		/// The `Option<()>` is effectively a bool, but it never hits storage in the `None` variant
		/// due to the guarantees of FRAME's storage APIs.
		///
		/// If this is `None` at the end of the block, we panic and render the block invalid.
		Included: Option<()>;
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// Inclusion inherent called more than once per block.
		TooManyInclusionInherents,
	}
}

decl_module! {
	/// The inclusion inherent module.
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		fn on_initialize() -> Weight {
			T::DbWeight::get().reads_writes(1, 1) // in on_finalize.
		}

		fn on_finalize() {
			if Included::take().is_none() {
				panic!("Bitfields and heads must be included every block");
			}
		}

		/// Include backed candidates and bitfields.
		#[weight = (
			MINIMAL_INCLUSION_INHERENT_WEIGHT + backed_candidates.len() as Weight * BACKED_CANDIDATE_WEIGHT,
			DispatchClass::Mandatory,
		)]
		pub fn inclusion(
			origin,
			signed_bitfields: SignedAvailabilityBitfields,
			backed_candidates: Vec<BackedCandidate<T::Hash>>,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;
			ensure!(!<Included>::exists(), Error::<T>::TooManyInclusionInherents);

			// Process new availability bitfields, yielding any availability cores whose
			// work has now concluded.
			let freed_concluded = <inclusion::Module<T>>::process_bitfields(
				signed_bitfields,
				<scheduler::Module<T>>::core_para,
			)?;

			// Handle timeouts for any availability core work.
			let availability_pred = <scheduler::Module<T>>::availability_timeout_predicate();
			let freed_timeout = if let Some(pred) = availability_pred {
				<inclusion::Module<T>>::collect_pending(pred)
			} else {
				Vec::new()
			};

			// Schedule paras again, given freed cores, and reasons for freeing.
			let freed = freed_concluded.into_iter().map(|c| (c, FreedReason::Concluded))
				.chain(freed_timeout.into_iter().map(|c| (c, FreedReason::TimedOut)));

			<scheduler::Module<T>>::schedule(freed);

			let backed_candidates = limit_backed_candidates::<T>(backed_candidates);
			let backed_candidates_len = backed_candidates.len() as Weight;

			// Process backed candidates according to scheduled cores.
			let occupied = <inclusion::Module<T>>::process_candidates(
				backed_candidates,
				<scheduler::Module<T>>::scheduled(),
				<scheduler::Module<T>>::group_validators,
			)?;

			// Note which of the scheduled cores were actually occupied by a backed candidate.
			<scheduler::Module<T>>::occupied(&occupied);

			// Give some time slice to dispatch pending upward messages.
			<ump::Module<T>>::process_pending_upward_messages();

			// And track that we've finished processing the inherent for this block.
			Included::set(Some(()));

			Ok(Some(
				MINIMAL_INCLUSION_INHERENT_WEIGHT +
				(backed_candidates_len * BACKED_CANDIDATE_WEIGHT)
			).into())
		}
	}
}

/// Limit the number of backed candidates processed in order to stay within block weight limits.
///
/// Use a configured assumption about the weight required to process a backed candidate and the
/// current block weight as of the execution of this function to ensure that we don't overload
/// the block with candidate processing.
///
/// If the backed candidates exceed the available block weight remaining, then skips all of them.
/// This is somewhat less desirable than attempting to fit some of them, but is more fair in the
/// even that we can't trust the provisioner to provide a fair / random ordering of candidates.
fn limit_backed_candidates<T: Config>(
	backed_candidates: Vec<BackedCandidate<T::Hash>>,
) -> Vec<BackedCandidate<T::Hash>> {
	// the weight of the inclusion inherent is already included in the current block weight,
	// so our operation is simple: if the block is currently overloaded, make this intrinsic smaller
	if frame_system::Module::<T>::block_weight().total() > <T as frame_system::Config>::BlockWeights::get().max_block {
		Vec::new()
	} else {
		backed_candidates
	}
}

/// We should only include the inherent under certain circumstances.
///
/// Most importantly, we check that the inherent is itself valid. It may not be, for example, in the
/// event of a session change.
fn should_include_inherent<T: Config>(
	signed_bitfields: &SignedAvailabilityBitfields,
	backed_candidates: &[BackedCandidate<T::Hash>],
) -> bool {
	// Sanity check: session changes can invalidate an inherent, and we _really_ don't want that to happen.
	// See github.com/paritytech/polkadot/issues/1327
	Module::<T>::inclusion(
		frame_system::RawOrigin::None.into(),
		signed_bitfields.clone(),
		backed_candidates.to_vec(),
	).is_ok()
}

impl<T: Config> ProvideInherent for Module<T> {
	type Call = Call<T>;
	type Error = MakeFatalError<()>;
	const INHERENT_IDENTIFIER: InherentIdentifier = INCLUSION_INHERENT_IDENTIFIER;

	fn create_inherent(data: &InherentData) -> Option<Self::Call> {
		data.get_data(&Self::INHERENT_IDENTIFIER)
			.expect("inclusion inherent data failed to decode")
			.map(|(signed_bitfields, backed_candidates): (SignedAvailabilityBitfields, Vec<BackedCandidate<T::Hash>>)| {
				if should_include_inherent::<T>(&signed_bitfields, &backed_candidates) {
					Call::inclusion(signed_bitfields, backed_candidates)
				} else {
					Call::inclusion(Vec::new().into(), Vec::new())
				}
			})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::mock::{
		new_test_ext, System, GenesisConfig as MockGenesisConfig, Test
	};

	mod limit_backed_candidates {
		use super::*;

		#[test]
		fn does_not_truncate_on_empty_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default()];
				System::set_block_consumed_resources(0, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 1);
			});
		}

		#[test]
		fn does_not_truncate_on_exactly_full_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default()];
				let max_block_weight = <Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 1);
			});
		}

		#[test]
		fn truncates_on_over_full_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default()];
				let max_block_weight = <Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight + 1, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 0);
			});
		}

		#[test]
		fn all_backed_candidates_get_truncated() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default(); 10];
				let max_block_weight = <Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight + 1, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 0);
			});
		}
	}

}
