// Copyright 2022 Parity Technologies (UK) Ltd.
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

use super::*;
use frame_benchmarking::{benchmarks, BenchmarkError, BenchmarkResult};
use frame_support::weights::Weight;
use frame_system::RawOrigin;
use sp_std::prelude::*;
use xcm::latest::prelude::*;

type RuntimeOrigin<T> = <T as frame_system::Config>::RuntimeOrigin;

benchmarks! {
	send {
		let send_origin = T::SendXcmOrigin::successful_origin();
		let msg = Xcm(vec![ClearOrigin]);
		let versioned_dest: VersionedMultiLocation = Parachain(2000).into();
		let versioned_msg = VersionedXcm::from(msg);
	}: _<RuntimeOrigin<T>>(send_origin, Box::new(versioned_dest), Box::new(versioned_msg))

	teleport_assets {
		let recipient = [0u8; 32];
		let versioned_dest: VersionedMultiLocation = Parachain(2000).into();
		let versioned_beneficiary: VersionedMultiLocation =
			AccountId32 { network: None, id: recipient.into() }.into();
		let versioned_assets: VersionedMultiAssets = (Here, 10).into();
	}: _(RawOrigin::Root, Box::new(versioned_dest), Box::new(versioned_beneficiary), Box::new(versioned_assets), 0)

	reserve_transfer_assets {
		let recipient = [0u8; 32];
		let versioned_dest: VersionedMultiLocation = Parachain(2000).into();
		let versioned_beneficiary: VersionedMultiLocation =
			AccountId32 { network: None, id: recipient.into() }.into();
		let versioned_assets: VersionedMultiAssets = (Here, 10).into();
	}: _(RawOrigin::Root, Box::new(versioned_dest), Box::new(versioned_beneficiary), Box::new(versioned_assets), 0)

	execute {
		let execute_origin = T::ExecuteXcmOrigin::successful_origin();
		let origin_location = T::ExecuteXcmOrigin::try_origin(execute_origin.clone())
			.map_err(|_| BenchmarkError::Override(BenchmarkResult::from_weight(Weight::MAX)))?;
		let msg = Xcm(vec![ClearOrigin]);
		if !T::XcmExecuteFilter::contains(&(origin_location, msg.clone())) {
			return Err(BenchmarkError::Skip)
		}
		let versioned_msg = VersionedXcm::from(msg);
	}: _<RuntimeOrigin<T>>(execute_origin, Box::new(versioned_msg), Weight::zero())

	force_xcm_version {
		let loc = Parachain(2000).into_location();
		let xcm_version = 2;
	}: _(RawOrigin::Root, Box::new(loc), xcm_version)

	force_default_xcm_version {}: _(RawOrigin::Root, Some(2))

	force_subscribe_version_notify {
		let versioned_loc: VersionedMultiLocation = Parachain(2000).into();
	}: _(RawOrigin::Root, Box::new(versioned_loc))

	force_unsubscribe_version_notify {
		let versioned_loc: VersionedMultiLocation = Parachain(2000).into();
		let _ = Pallet::<T>::request_version_notify(Parachain(2000));
	}: _(RawOrigin::Root, Box::new(versioned_loc))

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext_with_balances(Vec::new()),
		crate::mock::Test
	);
}
