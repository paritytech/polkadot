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
use primitives::{
	inclusion_inherent,
	parachain::{BackedCandidate, SignedAvailabilityBitfields},
};
use frame_support::{
	decl_error, decl_module, decl_storage, ensure,
	dispatch::DispatchResult,
	weights::{DispatchClass, Weight},
	traits::Get,
};
use system::ensure_none;
use crate::{
	inclusion,
	scheduler::{self, FreedReason},
};
use inherents::{InherentIdentifier, InherentData, MakeFatalError, ProvideInherent};

pub trait Trait: inclusion::Trait + scheduler::Trait {}

decl_storage! {
	trait Store for Module<T: Trait> as ParaInclusionInherent {
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
	pub enum Error for Module<T: Trait> {
		/// Inclusion inherent called more than once per block.
		TooManyInclusionInherents,
	}
}

decl_module! {
	/// The inclusion inherent module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin, system = system {
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
		#[weight = (1_000_000_000, DispatchClass::Mandatory)]
		pub fn inclusion(
			origin,
			signed_bitfields: SignedAvailabilityBitfields,
			backed_candidates: Vec<BackedCandidate<T::Hash>>,
		) -> DispatchResult {
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

			<scheduler::Module<T>>::schedule(freed.collect());

			// Process backed candidates according to scheduled cores.
			let occupied = <inclusion::Module<T>>::process_candidates(
				backed_candidates,
				<scheduler::Module<T>>::scheduled(),
				<scheduler::Module<T>>::group_validators,
			)?;

			// Note which of the scheduled cores were actually occupied by a backed candidate.
			<scheduler::Module<T>>::occupied(&occupied);

			// And track that we've finished processing the inherent for this block.
			Included::set(Some(()));

			Ok(())
		}
	}
}

impl<T: Trait> ProvideInherent for Module<T> {
	type Call = Call<T>;
	type Error = MakeFatalError<()>;
	const INHERENT_IDENTIFIER: InherentIdentifier = inclusion_inherent::INHERENT_IDENTIFIER;

	fn create_inherent(data: &InherentData) -> Option<Self::Call> {
		data.get_data(&Self::INHERENT_IDENTIFIER)
			.expect("inclusion inherent data failed to decode")
			.map(|(signed_bitfields, backed_candidates): (SignedAvailabilityBitfields, Vec<BackedCandidate<T::Hash>>)| {
				// Sanity check: session changes can invalidate an inherent, and we _really_ don't want that to happen.
				// See github.com/paritytech/polkadot/issues/1327
				if Self::inclusion(system::RawOrigin::None.into(), signed_bitfields.clone(), backed_candidates.clone()).is_ok() {
					Call::inclusion(signed_bitfields, backed_candidates)
				} else {
					Call::inclusion(Vec::new().into(), Vec::new())
				}
			})
	}
}
