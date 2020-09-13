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

//! Primitive types which are strictly necessary from a parachain-execution point
//! of view.

use sp_std::vec::Vec;

use codec::{Encode, Decode, CompactAs};
use sp_core::{RuntimeDebug, TypeId};

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

#[cfg(feature = "std")]
use sp_core::bytes;

/// Block number type used by the relay chain.
pub use polkadot_core_primitives::BlockNumber as RelayChainBlockNumber;

/// Deprecated - use sp_runtime::traits::AccountIdConversion directly instead.
#[deprecated]
pub use sp_runtime::traits::AccountIdConversion;

/// Parachain head data included in the chain.
#[derive(PartialEq, Eq, Clone, PartialOrd, Ord, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Default))]
pub struct HeadData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

impl From<Vec<u8>> for HeadData {
	fn from(head: Vec<u8>) -> Self {
		HeadData(head)
	}
}

/// Parachain validation code.
#[derive(Default, PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct ValidationCode(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

impl From<Vec<u8>> for ValidationCode {
	fn from(code: Vec<u8>) -> Self {
		ValidationCode(code)
	}
}

/// Parachain block data.
///
/// Contains everything required to validate para-block, may contain block and witness data.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct BlockData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Unique identifier of a parachain.
#[derive(
	Clone, CompactAs, Copy, Decode, Default, Encode, Eq,
	Hash, Ord, PartialEq, PartialOrd, RuntimeDebug,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize, derive_more::Display))]
pub struct Id(u32);

impl TypeId for Id {
	const TYPE_ID: [u8; 4] = *b"para";
}

impl From<Id> for u32 {
	fn from(x: Id) -> Self { x.0 }
}

impl From<u32> for Id {
	fn from(x: u32) -> Self { Id(x) }
}

impl From<usize> for Id {
	fn from(x: usize) -> Self {
		use sp_std::convert::TryInto;
		// can't panic, so need to truncate
		let x = x.try_into().unwrap_or(u32::MAX);
		Id(x)
	}
}

// When we added a second From impl for Id, type inference could no longer
// determine which impl should apply for things like `5.into()`. It therefore
// raised a bunch of errors in our test code, scattered throughout the
// various modules' tests, that there is no impl of `From<i32>` (`i32` being
// the default numeric type).
//
// We can't use `cfg(test)` here, because that configuration directive does not
// propagate between crates, which would fail to fix tests in crates other than
// this one.
//
// Instead, let's take advantage of the observation that what really matters for a
// ParaId within a test context is that it is unique and constant. I believe that
// there is no case where someone does `(-1).into()` anyway, but if they do, it
// never matters whether the actual contained ID is `-1` or `4294967295`. Nobody
// does arithmetic on a `ParaId`; doing so would be a bug.
impl From<i32> for Id {
	fn from(x: i32) -> Self {
		Id(x as u32)
	}
}

const USER_INDEX_START: u32 = 1000;

/// The ID of the first user (non-system) parachain.
pub const LOWEST_USER_ID: Id = Id(USER_INDEX_START);

impl Id {
	/// Create an `Id`.
	pub const fn new(id: u32) -> Self {
		Self(id)
	}

	/// Returns `true` if this parachain runs with system-level privileges.
	/// Use IsSystem instead.
	#[deprecated]
	pub fn is_system(&self) -> bool { self.0 < USER_INDEX_START }
}

pub trait IsSystem {
	fn is_system(&self) -> bool;
}

impl IsSystem for Id {
	fn is_system(&self) -> bool {
		self.0 < USER_INDEX_START
	}
}
impl IsSystem for Sibling {
	fn is_system(&self) -> bool {
		IsSystem::is_system(&self.0)
	}
}

impl sp_std::ops::Add<u32> for Id {
	type Output = Self;

	fn add(self, other: u32) -> Self {
		Self(self.0 + other)
	}
}

#[derive(Clone, Copy, Default, Encode, Decode, Eq, PartialEq, Ord, PartialOrd, RuntimeDebug)]
pub struct Sibling(pub Id);

impl From<Id> for Sibling {
	fn from(i: Id) -> Self {
		Self(i)
	}
}
impl From<Sibling> for Id {
	fn from(i: Sibling) -> Self {
		i.0
	}
}
impl AsRef<Id> for Sibling {
	fn as_ref(&self) -> &Id {
		&self.0
	}
}
impl TypeId for Sibling {
	const TYPE_ID: [u8; 4] = *b"sibl";
}
impl From<Sibling> for u32 {
	fn from(x: Sibling) -> Self { x.0.into() }
}

impl From<u32> for Sibling {
	fn from(x: u32) -> Self { Sibling(x.into()) }
}

/// Which origin a parachain's message to the relay chain should be dispatched from.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
#[repr(u8)]
pub enum ParachainDispatchOrigin {
	/// As the special `Origin::Parachain(ParaId)`. This is good when interacting with parachain-
	/// aware modules which need to succinctly verify that the origin is a parachain.
	Parachain,
	/// As a simple `Origin::Signed`, using `ParaId::account_id` as its value. This is good when
	/// interacting with standard modules such as `balances`.
	Signed,
	/// As the simple, superuser `Origin::Root`. This can only be done on specially permissioned
	/// parachains.
	Root,
}

impl sp_std::convert::TryFrom<u8> for ParachainDispatchOrigin {
	type Error = ();
	fn try_from(x: u8) -> core::result::Result<ParachainDispatchOrigin, ()> {
		const SIGNED: u8 = ParachainDispatchOrigin::Signed as u8;
		const PARACHAIN: u8 = ParachainDispatchOrigin::Parachain as u8;
		Ok(match x {
			SIGNED => ParachainDispatchOrigin::Signed,
			PARACHAIN => ParachainDispatchOrigin::Parachain,
			_ => return Err(()),
		})
	}
}

impl From<ParachainDispatchOrigin> for xcm::v0::OriginKind {
	fn from(o: ParachainDispatchOrigin) -> Self {
		match o {
			ParachainDispatchOrigin::Parachain => xcm::v0::OriginKind::Native,
			ParachainDispatchOrigin::Signed => xcm::v0::OriginKind::SovereignAccount,
			ParachainDispatchOrigin::Root => xcm::v0::OriginKind::Superuser,
		}
	}
}

impl From<xcm::v0::OriginKind> for ParachainDispatchOrigin {
	fn from(o: xcm::v0::OriginKind) -> Self {
		match o {
			xcm::v0::OriginKind::Native => ParachainDispatchOrigin::Parachain,
			xcm::v0::OriginKind::SovereignAccount => ParachainDispatchOrigin::Signed,
			xcm::v0::OriginKind::Superuser => ParachainDispatchOrigin::Root,
		}
	}
}

/// Validation parameters for evaluating the parachain validity function.
// TODO: balance downloads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Encode))]
pub struct ValidationParams {
	/// The collation body.
	pub block_data: BlockData,
	/// Previous head-data.
	pub parent_head: HeadData,
	/// The maximum code size permitted, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size permitted, in bytes.
	pub max_head_data_size: u32,
	/// The current relay-chain block number.
	pub relay_chain_height: polkadot_core_primitives::BlockNumber,
	/// Whether a code upgrade is allowed or not, and at which height the upgrade
	/// would be applied after, if so. The parachain logic should apply any upgrade
	/// issued in this block after the first block
	/// with `relay_chain_height` at least this value, if `Some`. if `None`, issue
	/// no upgrade.
	pub code_upgrade_allowed: Option<polkadot_core_primitives::BlockNumber>,
}

/// The result of parachain validation.
// TODO: egress and balance uploads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Debug, Decode))]
pub struct ValidationResult {
	/// New head data that should be included in the relay chain state.
	pub head_data: HeadData,
	/// An update to the validation code that should be scheduled in the relay chain.
	pub new_validation_code: Option<ValidationCode>,
	/// Upward messages send by the Parachain.
	pub upward_messages: Vec<Vec<u8>>,
	/// Number of downward messages that were processed by the Parachain.
	///
	/// It is expected that the Parachain processes them from first to last.
	pub processed_downward_messages: u32,
}
