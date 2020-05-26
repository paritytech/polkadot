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

//! This module is responsible for maintaining a consistent initialization order for all other
//! parachains modules. It's also responsible for finalization and session change notifications.
//!
//! This module can throw fatal errors if session-change notifications are received after initialization.

use sp_std::prelude::*;
use sp_std::result;
use codec::{Decode, Encode};
use sp_runtime::{
	KeyTypeId, Perbill, RuntimeDebug,
	traits::{
		Hash as HashT, BlakeTwo256, Saturating, One, Zero, Dispatchable,
		AccountIdConversion, BadOrigin, Convert, SignedExtension, AppVerify,
		DispatchInfoOf,
	},
	transaction_validity::{TransactionValidityError, ValidTransaction, TransactionValidity},
};
use sp_staking::{
	SessionIndex,
	offence::{ReportOffence, Offence, Kind},
};
use frame_support::{
	traits::KeyOwnerProofSystem,
	dispatch::{IsSubType},
	weights::{DispatchClass, Weight},
};
use primitives::{
	Balance,
	BlockNumber,
	parachain::{
		Id as ParaId, Chain, DutyRoster, AttestedCandidate, Statement, ParachainDispatchOrigin,
		UpwardMessage, ValidatorId, ActiveParas, CollatorId, Retriable, OmittedValidationData,
		CandidateReceipt, GlobalValidationSchedule, AbridgedCandidateReceipt,
		LocalValidationData, Scheduling, ValidityAttestation, NEW_HEADS_IDENTIFIER, PARACHAIN_KEY_TYPE_ID,
		ValidatorSignature, SigningContext, HeadData, ValidationCode,
	},
};
use frame_support::{
	Parameter, dispatch::DispatchResult, decl_storage, decl_module, decl_error, ensure,
	traits::{Currency, Get, WithdrawReason, ExistenceRequirement, Randomness},
};
use sp_runtime::{
	transaction_validity::InvalidTransaction,
};

use inherents::{ProvideInherent, InherentData, MakeFatalError, InherentIdentifier};

use system::{
	ensure_none, ensure_signed,
	offchain::{CreateSignedTransaction, SendSignedTransaction, Signer},
};

pub trait Trait: system::Trait { }

decl_storage! {
	trait Storage for Module<T: Trait> as Initializer {
		/// Whether the parachains modules have been initialized within this block.
		/// Should never hit the trie.
		HasInitialized: Option<()>,
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The initializer module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;

		fn on_initialize(now: T::BlockNumber) -> Weight {
			HasInitialized::set(Some(()));

			0
		}

		fn on_finalize() {
			HasInitialized::take();
		}
	}
}
