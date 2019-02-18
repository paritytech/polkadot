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

#[cfg(test)]
#[macro_use]
extern crate hex_literal;
#[cfg(test)]
extern crate secp256k1;
#[cfg(test)]
extern crate tiny_keccak;

#[macro_use]
extern crate bitvec;

extern crate parity_codec_derive;
extern crate parity_codec as codec;

extern crate substrate_consensus_aura_primitives as consensus_aura;
extern crate substrate_primitives;
extern crate substrate_inherents as inherents;
#[macro_use]
extern crate substrate_client as client;

extern crate sr_std as rstd;
extern crate sr_io;
extern crate sr_version as version;
#[macro_use]
extern crate sr_primitives;

#[macro_use]
extern crate srml_support;
extern crate srml_aura as aura;
extern crate srml_balances as balances;
extern crate srml_consensus as consensus;
extern crate srml_council as council;
extern crate srml_democracy as democracy;
extern crate srml_executive as executive;
extern crate srml_grandpa as grandpa;
extern crate srml_indices as indices;
extern crate srml_session as session;
extern crate srml_staking as staking;
extern crate srml_sudo as sudo;
extern crate srml_system as system;
extern crate srml_timestamp as timestamp;
extern crate srml_treasury as treasury;
extern crate srml_upgrade_key as upgrade_key;
extern crate srml_fees as fees;

extern crate polkadot_primitives as primitives;

#[cfg(test)]
extern crate substrate_keyring as keyring;

mod parachains;
mod claims;

use rstd::prelude::*;
use substrate_primitives::u32_trait::{_2, _4};
use primitives::{
	AccountId, AccountIndex, Balance, BlockNumber, Hash, Nonce, SessionKey, Signature,
	parachain,
};
use client::{
	block_builder::api::{self as block_builder_api, InherentData, CheckInherentsResult},
	runtime_api as client_api,
};
use sr_primitives::{
	ApplyResult, generic, transaction_validity::TransactionValidity,
	traits::{Convert, BlakeTwo256, Block as BlockT, DigestFor, StaticLookup}
};
use version::RuntimeVersion;
use grandpa::fg_primitives::{self, ScheduledChange};
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
pub use parachains::{Call as ParachainsCall, INHERENT_IDENTIFIER as PARACHAIN_INHERENT_IDENTIFIER};
pub use sr_primitives::{Permill, Perbill};
pub use timestamp::BlockPeriod;
pub use srml_support::StorageValue;

/// Runtime version.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("polkadot"),
	impl_name: create_runtime_str!("parity-polkadot"),
	authoring_version: 1,
	spec_version: 107,
	impl_version: 1,
	apis: RUNTIME_API_VERSIONS,
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
	type Index = Nonce;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type Digest = generic::Digest<Log>;
	type AccountId = AccountId;
	type Lookup = Indices;
	type Header = generic::Header<BlockNumber, BlakeTwo256, Log>;
	type Event = Event;
	type Log = Log;
}

impl aura::Trait for Runtime {
	type HandleReport = aura::StakingSlasher<Runtime>;
}

impl indices::Trait for Runtime {
	type IsDeadAccount = Balances;
	type AccountIndex = AccountIndex;
	type ResolveHint = indices::SimpleResolveHint<Self::AccountId, Self::AccountIndex>;
	type Event = Event;
}

impl balances::Trait for Runtime {
	type Balance = Balance;
	type OnFreeBalanceZero = Staking;
	type OnNewAccount = Indices;
	type EnsureAccountLiquid = Staking;
	type Event = Event;
}

impl consensus::Trait for Runtime {
	type Log = Log;
	type SessionKey = SessionKey;

	// the aura module handles offline-reports internally
	// rather than using an explicit report system.
	type InherentOfflineReport = ();
}

impl timestamp::Trait for Runtime {
	type Moment = u64;
	type OnTimestampSet = Aura;
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
	type OnSessionChange = (Staking, grandpa::SyncedAuthorities<Runtime>);
	type Event = Event;
}

impl staking::Trait for Runtime {
	type Currency = Balances;
	type OnRewardMinted = Treasury;
	type Event = Event;
}

impl democracy::Trait for Runtime {
	type Currency = Balances;
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
	type Currency = Balances;
	type ApproveOrigin = council_motions::EnsureMembers<_4>;
	type RejectOrigin = council_motions::EnsureMembers<_2>;
	type Event = Event;
}

impl grandpa::Trait for Runtime {
	type SessionKey = SessionKey;
	type Log = Log;
	type Event = Event;
}

impl parachains::Trait for Runtime {}

impl upgrade_key::Trait for Runtime {
	type Event = Event;
}

impl sudo::Trait for Runtime {
	type Event = Event;
	type Proposal = Call;
}

impl claims::Trait for Runtime {
	type Event = Event;
}

impl fees::Trait for Runtime {
	type Event = Event;
	type Amount = Balance;
	type TransferAsset = Balances;
}

construct_runtime!(
	pub enum Runtime with Log(InternalLog: DigestItem<Hash, SessionKey>) where
		Block = Block,
		NodeBlock = primitives::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: system::{default, Log(ChangesTrieRoot)},
		Aura: aura::{Module},
		Timestamp: timestamp::{Module, Call, Storage, Config<T>, Inherent},
		// consensus' Inherent is not provided because it assumes instant-finality blocks.
		Consensus: consensus::{Module, Call, Storage, Config<T>, Log(AuthoritiesChange) },
		Indices: indices,
		Balances: balances,
		Session: session,
		Staking: staking,
		Democracy: democracy,
		Grandpa: grandpa::{Module, Call, Storage, Config<T>, Log(), Event<T>},
		Council: council::{Module, Call, Storage, Event<T>},
		CouncilVoting: council_voting,
		CouncilMotions: council_motions::{Module, Call, Storage, Event<T>, Origin},
		CouncilSeats: council_seats::{Config<T>},
		Treasury: treasury,
		Parachains: parachains::{Module, Call, Storage, Config<T>, Inherent},
		Sudo: sudo,
		UpgradeKey: upgrade_key,
		Claims: claims,
		Fees: fees::{Module, Storage, Config<T>, Event<T>},
	}
);

/// The address format for describing accounts.
pub type Address = <Indices as StaticLookup>::Source;
/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256, Log>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedMortalCompactExtrinsic<Address, Nonce, Call, Signature>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Nonce, Call>;
/// Executive: handles dispatch to the various modules.
pub type Executive = executive::Executive<Runtime, Block, system::ChainContext<Runtime>, Fees, AllModules>;

impl_runtime_apis! {
	impl client_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn authorities() -> Vec<SessionKey> {
			Consensus::authorities()
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialise_block(header: &<Block as BlockT>::Header) {
			Executive::initialise_block(header)
		}
	}

	impl client_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			Runtime::metadata().into()
		}
	}

	impl block_builder_api::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalise_block() -> <Block as BlockT>::Header {
			Executive::finalise_block()
		}

		fn inherent_extrinsics(data: InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(block: Block, data: InherentData) -> CheckInherentsResult {
			data.check_extrinsics(&block)
		}

		fn random_seed() -> <Block as BlockT>::Hash {
			System::random_seed()
		}
	}

	impl client_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(tx: <Block as BlockT>::Extrinsic) -> TransactionValidity {
			Executive::validate_transaction(tx)
		}
	}

	impl parachain::ParachainHost<Block> for Runtime {
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
		fn ingress(to: parachain::Id) -> Option<Vec<(parachain::Id, Hash)>> {
			Parachains::ingress(to)
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_pending_change(digest: &DigestFor<Block>)
			-> Option<ScheduledChange<BlockNumber>>
		{
			for log in digest.logs.iter().filter_map(|l| match l {
				Log(InternalLog::grandpa(grandpa_signal)) => Some(grandpa_signal),
				_=> None
			}) {
				if let Some(change) = Grandpa::scrape_digest_change(log) {
					return Some(change);
				}
			}
			None
		}

		fn grandpa_authorities() -> Vec<(SessionKey, u64)> {
			Grandpa::grandpa_authorities()
		}
	}

	impl consensus_aura::AuraApi<Block> for Runtime {
		fn slot_duration() -> u64 {
			Aura::slot_duration()
		}
	}
}
