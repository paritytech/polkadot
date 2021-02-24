// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Primitives that may be used at (bridges) runtime level.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::Encode;
use sp_core::hash::H256;
use sp_io::hashing::blake2_256;
use sp_std::convert::TryFrom;

pub use chain::{BlockNumberOf, Chain, HashOf, HasherOf, HeaderOf};

mod chain;

/// Use this when something must be shared among all instances.
pub const NO_INSTANCE_ID: InstanceId = [0, 0, 0, 0];

/// Bridge-with-Rialto instance id.
pub const RIALTO_BRIDGE_INSTANCE: InstanceId = *b"rlto";

/// Bridge-with-Millau instance id.
pub const MILLAU_BRIDGE_INSTANCE: InstanceId = *b"mlau";

/// Bridge-with-Polkadot instance id.
pub const POLKADOT_BRIDGE_INSTANCE: InstanceId = *b"pdot";

/// Bridge-with-Kusama instance id.
pub const KUSAMA_BRIDGE_INSTANCE: InstanceId = *b"ksma";

/// Call-dispatch module prefix.
pub const CALL_DISPATCH_MODULE_PREFIX: &[u8] = b"pallet-bridge/call-dispatch";

/// Message-lane module prefix.
pub const MESSAGE_LANE_MODULE_PREFIX: &[u8] = b"pallet-bridge/message-lane";

/// A unique prefix for entropy when generating cross-chain account IDs.
pub const ACCOUNT_DERIVATION_PREFIX: &[u8] = b"pallet-bridge/account-derivation/account";

/// A unique prefix for entropy when generating a cross-chain account ID for the Root account.
pub const ROOT_ACCOUNT_DERIVATION_PREFIX: &[u8] = b"pallet-bridge/account-derivation/root";

/// Id of deployed module instance. We have a bunch of pallets that may be used in
/// different bridges. E.g. message-lane pallet may be deployed twice in the same
/// runtime to bridge ThisChain with Chain1 and Chain2. Sometimes we need to be able
/// to identify deployed instance dynamically. This type is used for that.
pub type InstanceId = [u8; 4];

/// Type of accounts on the source chain.
pub enum SourceAccount<T> {
	/// An account that belongs to Root (priviledged origin).
	Root,
	/// A non-priviledged account.
	///
	/// The embedded account ID may or may not have a private key depending on the "owner" of the
	/// account (private key, pallet, proxy, etc.).
	Account(T),
}

/// Derive an account ID from a foreign account ID.
///
/// This function returns an encoded Blake2 hash. It is the responsibility of the caller to ensure
/// this can be succesfully decoded into an AccountId.
///
/// The `bridge_id` is used to provide extra entropy when producing account IDs. This helps prevent
/// AccountId collisions between different bridges on a single target chain.
///
/// Note: If the same `bridge_id` is used across different chains (for example, if one source chain
/// is bridged to multiple target chains), then all the derived accounts would be the same across
/// the different chains. This could negatively impact users' privacy across chains.
pub fn derive_account_id<AccountId>(bridge_id: InstanceId, id: SourceAccount<AccountId>) -> H256
where
	AccountId: Encode,
{
	match id {
		SourceAccount::Root => (ROOT_ACCOUNT_DERIVATION_PREFIX, bridge_id).using_encoded(blake2_256),
		SourceAccount::Account(id) => (ACCOUNT_DERIVATION_PREFIX, bridge_id, id).using_encoded(blake2_256),
	}
	.into()
}

/// Derive the account ID of the shared relayer fund account.
///
/// This account is used to collect fees for relayers that are passing messages across the bridge.
///
/// The account ID can be the same across different instances of `message-lane` if the same
/// `bridge_id` is used.
pub fn derive_relayer_fund_account_id(bridge_id: InstanceId) -> H256 {
	("relayer-fund-account", bridge_id).using_encoded(blake2_256).into()
}

/// Anything that has size.
pub trait Size {
	/// Return approximate size of this object (in bytes).
	///
	/// This function should be lightweight. The result should not necessary be absolutely
	/// accurate.
	fn size_hint(&self) -> u32;
}

impl Size for () {
	fn size_hint(&self) -> u32 {
		0
	}
}

/// Pre-computed size.
pub struct PreComputedSize(pub usize);

impl Size for PreComputedSize {
	fn size_hint(&self) -> u32 {
		u32::try_from(self.0).unwrap_or(u32::MAX)
	}
}
