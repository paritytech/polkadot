// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Code for elections.

use frame_support::{
	parameter_types,
	weights::{DispatchClass, Weight},
};
use sp_runtime::{
	traits::{Zero, Dispatchable},
	FixedU128, FixedPointNumber, Perbill,
};
use pallet_transaction_payment::OnChargeTransaction;
use frame_support::weights::{DispatchInfo, Pays};
use super::{BlockExecutionWeight, BlockLength, BlockWeights};

parameter_types! {
	/// A limit for off-chain phragmen unsigned solution submission.
	///
	/// We want to keep it as high as possible, but can't risk having it reject,
	/// so we always subtract the base block execution weight.
	pub OffchainSolutionWeightLimit: Weight = BlockWeights::get()
		.get(DispatchClass::Normal)
		.max_extrinsic
		.expect("Normal extrinsics have weight limit configured by default; qed")
		.saturating_sub(BlockExecutionWeight::get());

	/// A limit for off-chain phragmen unsigned solution length.
	///
	/// We allow up to 90% of the block's size to be consumed by the solution.
	pub OffchainSolutionLengthLimit: u32 = Perbill::from_rational(90_u32, 100) *
		*BlockLength::get()
		.max
		.get(DispatchClass::Normal);
}

pub fn fee_for_submit_call<T>(
	multiplier: FixedU128,
	weight: Weight,
	length: u32,
) -> primitives::v1::Balance
where
	T: pallet_transaction_payment::Config,
	<T as pallet_transaction_payment::Config>::OnChargeTransaction:
		OnChargeTransaction<T, Balance = primitives::v1::Balance>,
	<T as frame_system::Config>::Call: Dispatchable<Info = DispatchInfo>,
{
	let info = DispatchInfo { weight, class: DispatchClass::Normal, pays_fee: Pays::Yes };
	multiplier.saturating_mul_int(pallet_transaction_payment::Pallet::<T>::compute_fee(
		length,
		&info,
		Zero::zero(),
	))
}
