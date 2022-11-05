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
use frame_system::RawOrigin;
use frame_benchmarking::{benchmarks, whitelisted_caller};
use sp_std::prelude::*;
use xcm::latest::prelude::*;

benchmarks! {
	send {
		let caller = whitelisted_caller::<T::AccountId>();
		let msg = Xcm(vec![ClearOrigin]);
		let versioned_dest: VersionedMultiLocation = Parachain(2000).into();
		let versioned_msg = VersionedXcm::from(msg);
	}: _(RawOrigin::Signed(caller.clone()), Box::new(versioned_dest), Box::new(versioned_msg))

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
		let msg = Xcm(vec![ClearOrigin]);
		let versioned_msg = VersionedXcm::from(msg);
	}: _(RawOrigin::Root, Box::new(versioned_msg), Weight::zero())

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