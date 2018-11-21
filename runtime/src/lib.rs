// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! The Polkadot runtime. This can be compiled with ``#[no_std]`, ready for Wasm.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit="256"]

#[macro_use]
extern crate parity_codec_derive;
extern crate parity_codec as codec;

extern crate substrate_primitives;
#[macro_use]
extern crate substrate_client as client;

#[macro_use]
extern crate sr_std as rstd;
#[cfg(test)]
extern crate sr_io;
#[macro_use]
extern crate sr_version as version;
#[macro_use]
extern crate sr_primitives;

#[macro_use]
extern crate srml_support;
extern crate srml_balances as balances;
extern crate srml_consensus as consensus;
extern crate srml_council as council;
extern crate srml_democracy as democracy;
extern crate srml_executive as executive;
extern crate srml_session as session;
extern crate srml_staking as staking;
extern crate srml_system as system;
extern crate srml_timestamp as timestamp;
extern crate srml_treasury as treasury;

extern crate polkadot_primitives as primitives;

mod parachains;

#[cfg(feature = "std")]
use codec::{Encode, Decode};
use rstd::prelude::*;
use substrate_primitives::u32_trait::{_2, _4};
use primitives::{
	AccountId, AccountIndex, Balance, BlockNumber, Hash, Index, SessionKey, Signature,
	parachain, parachain::runtime::ParachainHost, parachain::id::PARACHAIN_HOST,
};
#[cfg(feature = "std")]
use primitives::Block as GBlock;
use client::{block_builder::api::runtime::*, runtime_api::{runtime::*, id::*}};
#[cfg(feature = "std")]
use client::runtime_api::ApiExt;
use sr_primitives::ApplyResult;
use sr_primitives::transaction_validity::TransactionValidity;
use sr_primitives::generic;
use sr_primitives::traits::{Convert, BlakeTwo256, Block as BlockT};
#[cfg(feature = "std")]
use sr_primitives::traits::ApiRef;
#[cfg(feature = "std")]
use substrate_primitives::AuthorityId;
use version::RuntimeVersion;
use council::{motions as council_motions, voting as council_voting};
#[cfg(feature = "std")]
use council::seats as council_seats;
#[cfg(any(feature = "std", test))]
use version::NativeVersion;
use substrate_primitives::OpaqueMetadata;

#[cfg(any(feature = "std", test))]
pub use sr_primitives::BuildStorage;
pub use consensus::Call as ConsensusCall;
pub use timestamp::Call as TimestampCall;
pub use balances::Call as BalancesCall;
pub use parachains::Call as ParachainsCall;
pub use sr_primitives::{Permill, Perbill};
pub use timestamp::BlockPeriod;
pub use srml_support::{StorageValue, RuntimeMetadata};

const TIMESTAMP_SET_POSITION: u32 = 0;
const NOTE_OFFLINE_POSITION: u32 = 1;
const PARACHAINS_SET_POSITION: u32 = 2;

/// Runtime version.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: ver_str!("polkadot"),
	impl_name: ver_str!("parity-polkadot"),
	authoring_version: 1,
	spec_version: 101,
	impl_version: 0,
	apis: apis_vec!([
		(BLOCK_BUILDER, 1),
		(TAGGED_TRANSACTION_QUEUE, 1),
		(METADATA, 1),
		(PARACHAIN_HOST, 1),
	]),
};

/// Native version.
#[cfg(any(feature = "std", test))]
pub fn native_version() -> NativeVersion {
	NativeVersion {
		runtime_version: VERSION,
		can_author_with: Default::default(),
	}
}

impl system::Trait for Runtime {
	type Origin = Origin;
	type Index = Index;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type Digest = generic::Digest<Log>;
	type AccountId = AccountId;
	type Header = generic::Header<BlockNumber, BlakeTwo256, Log>;
	type Event = Event;
	type Log = Log;
}

impl balances::Trait for Runtime {
	type Balance = Balance;
	type AccountIndex = AccountIndex;
	type OnFreeBalanceZero = Staking;
	type EnsureAccountLiquid = Staking;
	type Event = Event;
}

impl consensus::Trait for Runtime {
	const NOTE_OFFLINE_POSITION: u32 = NOTE_OFFLINE_POSITION;
	type Log = Log;
	type SessionKey = SessionKey;
	type OnOfflineValidator = Staking;
}

impl timestamp::Trait for Runtime {
	const TIMESTAMP_SET_POSITION: u32 = TIMESTAMP_SET_POSITION;
	type Moment = u64;
}

/// Session key conversion.
pub struct SessionKeyConversion;
impl Convert<AccountId, SessionKey> for SessionKeyConversion {
	fn convert(a: AccountId) -> SessionKey {
		a.to_fixed_bytes().into()
	}
}

impl session::Trait for Runtime {
	type ConvertAccountIdToSessionKey = SessionKeyConversion;
	type OnSessionChange = Staking;
	type Event = Event;
}

impl staking::Trait for Runtime {
	type OnRewardMinted = Treasury;
	type Event = Event;
}

impl democracy::Trait for Runtime {
	type Proposal = Call;
	type Event = Event;
}

impl council::Trait for Runtime {
	type Event = Event;
}

impl council::voting::Trait for Runtime {
	type Event = Event;
}

impl council::motions::Trait for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
}

impl treasury::Trait for Runtime {
	type ApproveOrigin = council_motions::EnsureMembers<_4>;
	type RejectOrigin = council_motions::EnsureMembers<_2>;
	type Event = Event;
}

impl parachains::Trait for Runtime {
	const SET_POSITION: u32 = PARACHAINS_SET_POSITION;
}

construct_runtime!(
	pub enum Runtime with Log(InternalLog: DigestItem<Hash, SessionKey>) where
		Block = Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: system::{default, Log(ChangesTrieRoot)},
		Timestamp: timestamp::{Module, Call, Storage, Config<T>, Inherent},
		Consensus: consensus::{Module, Call, Storage, Config<T>, Log(AuthoritiesChange), Inherent},
		Balances: balances,
		Session: session,
		Staking: staking,
		Democracy: democracy,
		Council: council::{Module, Call, Storage, Event<T>},
		CouncilVoting: council_voting,
		CouncilMotions: council_motions::{Module, Call, Storage, Event<T>, Origin},
		CouncilSeats: council_seats::{Config<T>},
		Treasury: treasury,
		Parachains: parachains::{Module, Call, Storage, Config<T>, Inherent},
	}
);

/// The address format for describing accounts.
pub use balances::address::Address as RawAddress;
/// The address format for describing accounts.
pub type Address = balances::Address<Runtime>;
/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256, Log>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedMortalExtrinsic<Address, Index, Call, Signature>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Index, Call>;
/// Executive: handles dispatch to the various modules.
pub type Executive = executive::Executive<Runtime, Block, balances::ChainContext<Runtime>, Balances, AllModules>;

#[cfg(feature = "std")]
pub struct ClientWithApi {
	call: ::std::ptr::NonNull<client::runtime_api::CallApiAt<GBlock>>,
	commit_on_success: ::std::cell::RefCell<bool>,
	initialised_block: ::std::cell::RefCell<Option<GBlockId>>,
	changes: ::std::cell::RefCell<client::runtime_api::OverlayedChanges>,
}

#[cfg(feature = "std")]
unsafe impl Send for ClientWithApi {}
#[cfg(feature = "std")]
unsafe impl Sync for ClientWithApi {}

#[cfg(feature = "std")]
impl ApiExt for ClientWithApi {
	fn map_api_result<F: FnOnce(&Self) -> Result<R, E>, R, E>(&self, map_call: F) -> Result<R, E> {
		*self.commit_on_success.borrow_mut() = false;
		let res = map_call(self);
		*self.commit_on_success.borrow_mut() = true;

		self.commit_on_ok(&res);

		res
	}
}

#[cfg(feature = "std")]
impl client::runtime_api::ConstructRuntimeApi<GBlock> for ClientWithApi {
	fn construct_runtime_api<'a, T: client::runtime_api::CallApiAt<GBlock>>(call: &'a T) -> ApiRef<'a, Self> {
		ClientWithApi {
			call: unsafe {
				::std::ptr::NonNull::new_unchecked(
					::std::mem::transmute(
						call as &client::runtime_api::CallApiAt<GBlock>
					)
				)
			},
			commit_on_success: true.into(),
			initialised_block: None.into(),
			changes: Default::default(),
		}.into()
	}
}

#[cfg(feature = "std")]
impl ClientWithApi {
	fn call_api_at<A: Encode, R: Decode>(
		&self,
		at: &GBlockId,
		function: &'static str,
		args: &A
	) -> client::error::Result<R> {
		let res = unsafe {
			self.call.as_ref().call_api_at(
				at,
				function,
				args.encode(),
				&mut *self.changes.borrow_mut(),
				&mut *self.initialised_block.borrow_mut()
			).and_then(|r|
				R::decode(&mut &r[..])
					.ok_or_else(||
						client::error::ErrorKind::CallResultDecode(function).into()
					)
			)
		};

		self.commit_on_ok(&res);
		res
	}

	fn commit_on_ok<R, E>(&self, res: &Result<R, E>) {
		if *self.commit_on_success.borrow() {
			if res.is_err() {
				self.changes.borrow_mut().discard_prospective();
			} else {
				self.changes.borrow_mut().commit_prospective();
			}
		}
	}
}

#[cfg(feature = "std")]
type GBlockId = generic::BlockId<GBlock>;

#[cfg(feature = "std")]
impl client::runtime_api::Core<GBlock> for ClientWithApi {
	fn version(&self, at: &GBlockId) -> Result<RuntimeVersion, client::error::Error> {
		self.call_api_at(at, "version", &())
	}

	fn authorities(&self, at: &GBlockId) -> Result<Vec<AuthorityId>, client::error::Error> {
		self.call_api_at(at, "authorities", &())
	}

	fn execute_block(&self, at: &GBlockId, block: &GBlock) -> Result<(), client::error::Error> {
		self.call_api_at(at, "execute_block", block)
	}

	fn initialise_block(&self, at: &GBlockId, header: &<GBlock as BlockT>::Header) -> Result<(), client::error::Error> {
		self.call_api_at(at, "initialise_block", header)
	}
}

#[cfg(feature = "std")]
impl client::block_builder::api::BlockBuilder<GBlock> for ClientWithApi {
	fn apply_extrinsic(&self, at: &GBlockId, extrinsic: &<GBlock as BlockT>::Extrinsic) -> Result<ApplyResult, client::error::Error> {
		self.call_api_at(at, "apply_extrinsic", extrinsic)
	}

	fn finalise_block(&self, at: &GBlockId) -> Result<<GBlock as BlockT>::Header, client::error::Error> {
		self.call_api_at(at, "finalise_block", &())
	}

	fn inherent_extrinsics<Inherent: Decode + Encode, Unchecked: Decode + Encode>(
		&self, at: &GBlockId, inherent: &Inherent
	) -> Result<Vec<Unchecked>, client::error::Error> {
		self.call_api_at(at, "inherent_extrinsics", inherent)
	}

	fn check_inherents<Inherent: Decode + Encode, Error: Decode + Encode>(&self, at: &GBlockId, block: &GBlock, inherent: &Inherent) -> Result<Result<(), Error>, client::error::Error> {
		self.call_api_at(at, "check_inherents", &(block, inherent))
	}

	fn random_seed(&self, at: &GBlockId) -> Result<<GBlock as BlockT>::Hash, client::error::Error> {
		self.call_api_at(at, "random_seed", &())
	}
}

#[cfg(feature = "std")]
impl client::runtime_api::TaggedTransactionQueue<GBlock> for ClientWithApi {
	fn validate_transaction(
		&self,
		at: &GBlockId,
		utx: &<GBlock as BlockT>::Extrinsic
	) -> Result<TransactionValidity, client::error::Error> {
		self.call_api_at(at, "validate_transaction", utx)
	}
}

#[cfg(feature = "std")]
impl client::runtime_api::Metadata<GBlock> for ClientWithApi {
	fn metadata(&self, at: &GBlockId) -> Result<OpaqueMetadata, client::error::Error> {
		self.call_api_at(at, "metadata", &())
	}
}

#[cfg(feature = "std")]
impl ::primitives::parachain::ParachainHost<GBlock> for ClientWithApi {
	fn validators(&self, at: &GBlockId) -> Result<Vec<primitives::AccountId>, client::error::Error> {
		self.call_api_at(at, "validators", &())
	}
	fn duty_roster(&self, at: &GBlockId) -> Result<primitives::parachain::DutyRoster, client::error::Error> {
		self.call_api_at(at, "calculate_duty_roster", &())
	}
	fn active_parachains(&self, at: &GBlockId) -> Result<Vec<parachain::Id>, client::error::Error> {
		self.call_api_at(at, "active_parachains", &())
	}
	fn parachain_head(&self, at: &GBlockId, id: &parachain::Id) -> Result<Option<Vec<u8>>, client::error::Error> {
		self.call_api_at(at, "parachain_head", &id)
	}
	fn parachain_code(&self, at: &GBlockId, id: &parachain::Id) -> Result<Option<Vec<u8>>, client::error::Error> {
		self.call_api_at(at, "parachain_code", &id)
	}
}

impl_runtime_apis! {
	impl Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn authorities() -> Vec<SessionKey> {
			Consensus::authorities()
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialise_block(header: <Block as BlockT>::Header) {
			Executive::initialise_block(&header)
		}
	}

	impl Metadata for Runtime {
		fn metadata() -> OpaqueMetadata {
			Runtime::metadata().into()
		}
	}

	impl BlockBuilder<Block, InherentData, UncheckedExtrinsic, InherentData, InherentError> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalise_block() -> <Block as BlockT>::Header {
			Executive::finalise_block()
		}

		fn inherent_extrinsics(data: InherentData) -> Vec<UncheckedExtrinsic> {
			data.create_inherent_extrinsics()
		}

		fn check_inherents(block: Block, data: InherentData) -> Result<(), InherentError> {
			data.check_inherents(block)
		}

		fn random_seed() -> <Block as BlockT>::Hash {
			System::random_seed()
		}
	}

	impl TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(tx: <Block as BlockT>::Extrinsic) -> TransactionValidity {
			Executive::validate_transaction(tx)
		}
	}

	impl ParachainHost for Runtime {
		fn validators() -> Vec<AccountId> {
			Session::validators()
		}
		fn duty_roster() -> parachain::DutyRoster {
			Parachains::calculate_duty_roster()
		}
		fn active_parachains() -> Vec<parachain::Id> {
			Parachains::active_parachains()
		}
		fn parachain_head(id: parachain::Id) -> Option<Vec<u8>> {
			Parachains::parachain_head(&id)
		}
		fn parachain_code(id: parachain::Id) -> Option<Vec<u8>> {
			Parachains::parachain_code(&id)
		}
	}
}

