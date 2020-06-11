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

//! The inclusion module is responsible for inclusion and availability of scheduled parachains
//! and parathreads.
//!
//! It is responsible for carrying candidates from being backable to being backed, and then from backed
//! to included.

use sp_std::prelude::*;
use primitives::{
	parachain::{
		ValidatorId, AbridgedCandidateReceipt, ValidatorIndex, Id as ParaId,
		AvailabilityBitfield as AvailabilityBitfield, SignedAvailabilityBitfields,
	},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
	dispatch::DispatchResult,
	weights::{DispatchClass, Weight},
};
use codec::{Encode, Decode};
use system::ensure_root;
use bitvec::vec::BitVec;

use crate::{configuration, paras, scheduler::CoreIndex};

/// A bitfield signed by a validator indicating that it is keeping its piece of the erasure-coding
/// for any backed candidates referred to by a `1` bit available.
#[derive(Encode, Decode)]
#[cfg_attr(test, derive(Debug))]
pub struct AvailabilityBitfieldRecord<N> {
	bitfield: AvailabilityBitfield, // one bit per core.
	submitted_at: N, // for accounting, as meaning of bits may change over time.
}

/// A backed candidate pending availability.
#[derive(Encode, Decode)]
#[cfg_attr(test, derive(Debug))]
pub struct CandidatePendingAvailability<N> {
	/// The availability core this is assigned to.
	core: CoreIndex,
	/// The candidate receipt itself.
	receipt: AbridgedCandidateReceipt,
	/// The received availability votes. One bit per validator.
	availability_votes: bitvec::vec::BitVec<bitvec::order::Lsb0, u8>,
	/// The block number of the relay-parent of the receipt.
	relay_parent_number: N,
	/// The block number of the relay-chain block this was backed in.
	backed_in_number: N,
}

pub trait Trait: system::Trait + paras::Trait + configuration::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as ParaInclusion {
		/// The latest bitfield for each validator, referred to by their index in the validator set.
		AvailabilityBitfields: map hasher(twox_64_concat) ValidatorIndex
			=> Option<AvailabilityBitfieldRecord<T::BlockNumber>>;

		/// Candidates pending availability by `ParaId`.
		PendingAvailability: map hasher(twox_64_concat) ParaId
			=> Option<CandidatePendingAvailability<T::BlockNumber>>
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The parachain-candidate inclusion module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {

	/// Process a set of incoming bitfields.
	pub(crate) fn process_bitfields(
		signed_bitfields: SignedAvailabilityBitfields,
		core_lookup: impl Fn(CoreIndex) -> Option<ParaId>,
	) -> DispatchResult {
		let n_validators = unimplemented!(); // TODO [now]: at least one module needs to store all validators
		let config = <configuration::Module<T>>::config();
		let parachains = <paras::Module<T>>::parachains();

		let n_bits = parachains.len() + config.parathread_cores as usize;

		// do sanity checks on the bitfields:
		// 1. no more than one bitfield per validator
		// 2. bitfields are ascending by validator index.
		// 3. each bitfield has exactly `n_bits`
		// 4. signature is valid.
		{
			let mut last_index = None;
			let mut payload_encode_buf = Vec::new();

			for signed_bitfield in &signed_bitfields.0 {
				let signing_context = unimplemented!(); // TODO [now].

				if signed_bitfield.bitfield.0.len() != n_bits {
					// TODO [now] return "malformed bitfield"
				}

				if last_index.map_or(false, |last| last >= signed_bitfield.validator_index) {
					// TODO [now] return "bitfield duplicate or out of order"
				}

				if let Err(()) = primitives::parachain::check_availability_bitfield_signature(
					&signed_bitfield.bitfield,
					&unimplemented!(), // TODO [now] get validator public key by index.
					&signed_bitfield.signature,
					&signing_context,
					Some(&mut payload_encode_buf),
				) {
					// TODO [now] return "invalid bitfield signature"
				}

				last_index = Some(signed_bitfield.validator_index);
				payload_encode_buf.clear();
			}
		}

		Ok(())
	}
}
