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
use sp_runtime::traits::{Hash as _, BlakeTwo256};

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

#[cfg(feature = "std")]
use sp_core::bytes;

use polkadot_core_primitives::Hash;

/// Block number type used by the relay chain.
pub use polkadot_core_primitives::BlockNumber as RelayChainBlockNumber;

/// Parachain head data included in the chain.
#[derive(PartialEq, Eq, Clone, PartialOrd, Ord, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Default, Hash))]
pub struct HeadData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

impl From<Vec<u8>> for HeadData {
	fn from(head: Vec<u8>) -> Self {
		HeadData(head)
	}
}

/// Parachain validation code.
#[derive(Default, PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash))]
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
	pub fn is_system(&self) -> bool { self.0 < USER_INDEX_START }
}

impl sp_std::ops::Add<u32> for Id {
	type Output = Self;

	fn add(self, other: u32) -> Self {
		Self(self.0 + other)
	}
}

/// This type can be converted into and possibly from an AccountId (which itself is generic).
pub trait AccountIdConversion<AccountId>: Sized {
	/// Convert into an account ID. This is infallible.
	fn into_account(&self) -> AccountId;

 	/// Try to convert an account ID into this type. Might not succeed.
	fn try_from_account(a: &AccountId) -> Option<Self>;
}

// TODO: Remove all of this, move sp-runtime::AccountIdConversion to own crate and and use that.
// #360
struct TrailingZeroInput<'a>(&'a [u8]);
impl<'a> codec::Input for TrailingZeroInput<'a> {
	fn remaining_len(&mut self) -> Result<Option<usize>, codec::Error> {
		Ok(None)
	}

	fn read(&mut self, into: &mut [u8]) -> Result<(), codec::Error> {
		let len = into.len().min(self.0.len());
		into[..len].copy_from_slice(&self.0[..len]);
		for i in &mut into[len..] {
			*i = 0;
		}
		self.0 = &self.0[len..];
		Ok(())
	}
}

/// Format is b"para" ++ encode(parachain ID) ++ 00.... where 00... is indefinite trailing
/// zeroes to fill AccountId.
impl<T: Encode + Decode + Default> AccountIdConversion<T> for Id {
	fn into_account(&self) -> T {
		(b"para", self).using_encoded(|b|
			T::decode(&mut TrailingZeroInput(b))
		).unwrap_or_default()
	}

 	fn try_from_account(x: &T) -> Option<Self> {
		x.using_encoded(|d| {
			if &d[0..4] != b"para" { return None }
			let mut cursor = &d[4..];
			let result = Decode::decode(&mut cursor).ok()?;
			if cursor.iter().all(|x| *x == 0) {
				Some(result)
			} else {
				None
			}
		})
	}
}

/// Which origin a parachain's message to the relay chain should be dispatched from.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Hash))]
#[repr(u8)]
pub enum ParachainDispatchOrigin {
	/// As a simple `Origin::Signed`, using `ParaId::account_id` as its value. This is good when
	/// interacting with standard modules such as `balances`.
	Signed,
	/// As the special `Origin::Parachain(ParaId)`. This is good when interacting with parachain-
	/// aware modules which need to succinctly verify that the origin is a parachain.
	Parachain,
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

/// A message from a parachain to its Relay Chain.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Hash))]
pub struct UpwardMessage {
	/// The origin for the message to be sent from.
	pub origin: ParachainDispatchOrigin,
	/// The message data.
	pub data: Vec<u8>,
}

/// The validation data provide information about how to validate both the inputs and
/// outputs of a candidate.
///
/// There are two types of validation data: persisted and transient.
/// Their respective sections of the guide elaborate on their functionality in more detail.
///
/// This information is derived from the chain state and will vary from para to para,
/// although some of the fields may be the same for every para.
///
/// Persisted validation data are generally derived from some relay-chain state to form inputs
/// to the validation function, and as such need to be persisted by the availability system to
/// avoid dependence on availability of the relay-chain state. The backing phase of the
/// inclusion pipeline ensures that everything that is included in a valid fork of the
/// relay-chain already adheres to the transient constraints.
///
/// The validation data also serve the purpose of giving collators a means of ensuring that
/// their produced candidate and the commitments submitted to the relay-chain alongside it
/// will pass the checks done by the relay-chain when backing, and give validators
/// the same understanding when determining whether to second or attest to a candidate.
///
/// Since the commitments of the validation function are checked by the
/// relay-chain, secondary checkers can rely on the invariant that the relay-chain
/// only includes para-blocks for which these checks have already been done. As such,
/// there is no need for the validation data used to inform validators and collators about
/// the checks the relay-chain will perform to be persisted by the availability system.
/// Nevertheless, we expose it so the backing validators can validate the outputs of a
/// candidate before voting to submit it to the relay-chain and so collators can
/// collate candidates that satisfy the criteria implied these transient validation data.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct ValidationData<N = RelayChainBlockNumber> {
	/// The persisted validation data.
	pub persisted: PersistedValidationData<N>,
	/// The transient validation data.
	pub transient: TransientValidationData<N>,
}

/// Validation data that needs to be persisted for secondary checkers.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct PersistedValidationData<N = RelayChainBlockNumber> {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The relay-chain block number this is in the context of.
	pub block_number: N,
	/// The list of MQC heads for the inbound channels paired with the sender para ids. This
	/// vector is sorted ascending by the para id and doesn't contain multiple entries with the same
	/// sender.
	pub hrmp_mqc_heads: Vec<(Id, Hash)>,
}

impl<N: Encode> PersistedValidationData<N> {
	/// Compute the blake2-256 hash of the persisted validation data.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// Validation data for checking outputs of the validation-function.
/// As such, they also inform the collator about how to construct the candidate.
///
/// These are transient because they are not necessary beyond the point where the
/// candidate is backed.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct TransientValidationData<N = RelayChainBlockNumber> {
	/// The maximum code size permitted, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size permitted, in bytes.
	pub max_head_data_size: u32,
	/// The balance of the parachain at the moment of validation.
	pub balance: polkadot_core_primitives::Balance,
	/// Whether the parachain is allowed to upgrade its validation code.
	///
	/// This is `Some` if so, and contains the number of the minimum relay-chain
	/// height at which the upgrade will be applied, if an upgrade is signaled
	/// now.
	///
	/// A parachain should enact its side of the upgrade at the end of the first
	/// parablock executing in the context of a relay-chain block with at least this
	/// height. This may be equal to the current perceived relay-chain block height, in
	/// which case the code upgrade should be applied at the end of the signaling
	/// block.
	pub code_upgrade_allowed: Option<N>,
}

/// Validation parameters for evaluating the parachain validity function.
// TODO: balance downloads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Encode))]
pub struct ValidationParams {
	/// The collation body.
	pub block_data: BlockData,
	/// The validation data that was also available to the collator when
	/// building the PoVBlock.
	pub validation_data: ValidationData,
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
	pub upward_messages: Vec<UpwardMessage>,
	/// Number of downward messages that were processed by the Parachain.
	///
	/// It is expected that the Parachain processes them from first to last.
	pub processed_downward_messages: u32,
}
