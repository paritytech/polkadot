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

#[cfg(feature = "std")]
#[macro_use]
extern crate serde_derive;

#[cfg(feature = "std")]
extern crate serde;

#[macro_use]
extern crate sr_io as runtime_io;

#[macro_use]
extern crate srml_support;

#[macro_use]
extern crate sr_primitives as runtime_primitives;

#[cfg(test)]
#[macro_use]
extern crate hex_literal;

#[cfg(test)]
extern crate substrate_serializer;

extern crate substrate_primitives;

#[macro_use]
extern crate sr_std as rstd;
#[macro_use]
extern crate parity_codec_derive;

extern crate polkadot_primitives as primitives;
extern crate parity_codec as codec;
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
#[macro_use]
extern crate sr_version as version;

#[cfg(feature = "std")]
mod checked_block;
mod parachains;
mod utils;

#[cfg(feature = "std")]
pub use checked_block::CheckedBlock;
pub use utils::{inherent_extrinsics, check_extrinsic};
pub use balances::address::Address as RawAddress;

use rstd::prelude::*;
use codec::{Encode, Decode, Input};
use substrate_primitives::u32_trait::{_2, _4};
use primitives::{AccountId, AccountIndex, Balance, BlockNumber, Hash, Index, SessionKey, Signature};
use runtime_primitives::{generic, traits::{Convert, BlakeTwo256, DigestItem}};
use version::RuntimeVersion;
use council::{motions as council_motions, voting as council_voting};

#[cfg(feature = "std")]
pub use runtime_primitives::BuildStorage;

pub use consensus::Call as ConsensusCall;
pub use timestamp::Call as TimestampCall;
pub use parachains::Call as ParachainsCall;

/// The position of the timestamp set extrinsic.
pub const TIMESTAMP_SET_POSITION: u32 = 0;
/// The position of the parachains set extrinsic.
pub const PARACHAINS_SET_POSITION: u32 = 1;
/// The position of the note_offline in the block, if it exists.
pub const NOTE_OFFLINE_POSITION: u32 = 2;

/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256, Log>;
/// The address format for describing accounts.
pub type Address = balances::Address<Runtime>;
/// Block Id type for this block.
pub type BlockId = generic::BlockId<Block>;
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic = generic::UncheckedExtrinsic<Address, Index, Call, Signature>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, Index, Call>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// Runtime runtime type used to parameterize the various modules.
// Workaround for https://github.com/rust-lang/rust/issues/26925 . Remove when sorted.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Debug, Serialize, Deserialize))]
pub struct Runtime;

/// Polkadot runtime version.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: ver_str!("polkadot"),
	impl_name: ver_str!("parity-polkadot"),
	authoring_version: 1,
	spec_version: 101,
	impl_version: 0,
};

impl system::Trait for Runtime {
	type Origin = Origin;
	type Index = Index;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type Digest = generic::Digest<Log>;
	type AccountId = AccountId;
	type Header = Header;
	type Event = Event;
}
/// System module for this concrete runtime.
pub type System = system::Module<Runtime>;

impl balances::Trait for Runtime {
	type Balance = Balance;
	type AccountIndex = AccountIndex;
	type OnFreeBalanceZero = Staking;
	type EnsureAccountLiquid = Staking;
	type Event = Event;
}
/// Staking module for this concrete runtime.
pub type Balances = balances::Module<Runtime>;

impl consensus::Trait for Runtime {
	const NOTE_OFFLINE_POSITION: u32 = NOTE_OFFLINE_POSITION;
	type Log = Log;
	type SessionKey = SessionKey;
	type OnOfflineValidator = Staking;
}
/// Consensus module for this concrete runtime.
pub type Consensus = consensus::Module<Runtime>;

impl timestamp::Trait for Runtime {
	const TIMESTAMP_SET_POSITION: u32 = TIMESTAMP_SET_POSITION;
	type Moment = u64;
}
/// Timestamp module for this concrete runtime.
pub type Timestamp = timestamp::Module<Runtime>;

/// Session key conversion.
pub struct SessionKeyConversion;
impl Convert<AccountId, SessionKey> for SessionKeyConversion {
	fn convert(a: AccountId) -> SessionKey {
		a.0.into()
	}
}

impl session::Trait for Runtime {
	type ConvertAccountIdToSessionKey = SessionKeyConversion;
	type OnSessionChange = Staking;
	type Event = Event;
}
/// Session module for this concrete runtime.
pub type Session = session::Module<Runtime>;

impl staking::Trait for Runtime {
	type OnRewardMinted = Treasury;
	type Event = Event;
}
/// Staking module for this concrete runtime.
pub type Staking = staking::Module<Runtime>;

impl democracy::Trait for Runtime {
	type Proposal = Call;
	type Event = Event;
}
/// Democracy module for this concrete runtime.
pub type Democracy = democracy::Module<Runtime>;

impl council::Trait for Runtime {
	type Event = Event;
}

/// Council module for this concrete runtime.
pub type Council = council::Module<Runtime>;

impl council::voting::Trait for Runtime {
	type Event = Event;
}

/// Council voting module for this concrete runtime.
pub type CouncilVoting = council::voting::Module<Runtime>;

impl council::motions::Trait for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
}

/// Council motions module for this concrete runtime.
pub type CouncilMotions = council_motions::Module<Runtime>;

impl treasury::Trait for Runtime {
	type ApproveOrigin = council_motions::EnsureMembers<_4>;
	type RejectOrigin = council_motions::EnsureMembers<_2>;
	type Event = Event;
}

/// Treasury module for this concrete runtime.
pub type Treasury = treasury::Module<Runtime>;

impl parachains::Trait for Runtime {
	const SET_POSITION: u32 = PARACHAINS_SET_POSITION;
}
pub type Parachains = parachains::Module<Runtime>;

impl_outer_event! {
	pub enum Event for Runtime {
		//consensus,
		balances,
		//timetstamp,
		session,
		staking,
		democracy,
		council,
		council_voting,
		council_motions,
		treasury
	}
}

impl_outer_log! {
	pub enum Log(InternalLog: DigestItem<SessionKey>) for Runtime {
		consensus(AuthoritiesChange)
	}
}

impl_outer_origin! {
	pub enum Origin for Runtime {
		council_motions
	}
}

impl_outer_dispatch! {
	/// Call type for polkadot transactions.
	pub enum Call where origin: <Runtime as system::Trait>::Origin {
		Consensus,
		Balances,
		Session,
		Staking,
		Timestamp,
		Democracy,
		Council,
		CouncilVoting,
		CouncilMotions,
		Parachains,
		Treasury,
	}
}

impl_outer_config! {
	pub struct GenesisConfig for Runtime {
		ConsensusConfig => consensus,
		SystemConfig => system,
		BalancesConfig => balances,
		SessionConfig => session,
		StakingConfig => staking,
		DemocracyConfig => democracy,
		CouncilConfig => council,
		TimestampConfig => timestamp,
		TreasuryConfig => treasury,
		ParachainsConfig => parachains,
	}
}

type AllModules = (
	Consensus,
	Balances,
	Session,
	Staking,
	Timestamp,
	Democracy,
	Council,
	CouncilVoting,
	CouncilMotions,
	Parachains,
	Treasury,
);

impl_json_metadata!(
	for Runtime with modules
		system::Module with Storage,
		consensus::Module with Storage,
		balances::Module with Storage,
		timestamp::Module with Storage,
		session::Module with Storage,
		staking::Module with Storage,
		democracy::Module with Storage,
		council::Module with Storage,
		council_voting::Module with Storage,
		council_motions::Module with Storage,
		treasury::Module with Storage,
		parachains::Module with Storage,
);

impl DigestItem for Log {
	type AuthorityId = SessionKey;

	fn as_authorities_change(&self) -> Option<&[Self::AuthorityId]> {
		match self.0 {
			InternalLog::consensus(ref item) => item.as_authorities_change(),
		}
	}
}

/// Executive: handles dispatch to the various modules.
pub type Executive = executive::Executive<Runtime, Block, Balances, Balances, AllModules>;

pub mod api {
	impl_stubs!(
		version => |()| super::VERSION,
		json_metadata => |()| super::Runtime::json_metadata(),
		authorities => |()| super::Consensus::authorities(),
		initialise_block => |header| super::Executive::initialise_block(&header),
		apply_extrinsic => |extrinsic| super::Executive::apply_extrinsic(extrinsic),
		execute_block => |block| super::Executive::execute_block(block),
		finalise_block => |()| super::Executive::finalise_block(),
		inherent_extrinsics => |(inherent, spec_version)| super::inherent_extrinsics(inherent, spec_version),
		validator_count => |()| super::Session::validator_count(),
		validators => |()| super::Session::validators(),
		timestamp => |()| super::Timestamp::get(),
		random_seed => |()| super::System::random_seed(),
		account_nonce => |account| super::System::account_nonce(&account),
		lookup_address => |address| super::Balances::lookup_address(address),
		duty_roster => |()| super::Parachains::calculate_duty_roster(),
		active_parachains => |()| super::Parachains::active_parachains(),
		parachain_head => |id| super::Parachains::parachain_head(&id),
		parachain_code => |id| super::Parachains::parachain_code(&id)
	);
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_primitives as primitives;
	use codec::{Encode, Decode};
	use substrate_primitives::hexdisplay::HexDisplay;
	use substrate_serializer as ser;
	use runtime_primitives::traits::Header as HeaderT;
	type Digest = generic::Digest<Log>;

	#[test]
	fn test_header_serialization() {
		let header = Header {
			parent_hash: 5.into(),
			number: 67,
			state_root: 3.into(),
			extrinsics_root: 6.into(),
			digest: Digest::default(),
		};

		assert_eq!(ser::to_string_pretty(&header), r#"{
  "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000005",
  "number": 67,
  "stateRoot": "0x0000000000000000000000000000000000000000000000000000000000000003",
  "extrinsicsRoot": "0x0000000000000000000000000000000000000000000000000000000000000006",
  "digest": {
    "logs": []
  }
}"#);

		let v = header.encode();
		assert_eq!(Header::decode(&mut &v[..]).unwrap(), header);
	}

	#[test]
	fn block_encoding_round_trip() {
		let mut block = Block {
			header: Header::new(1, Default::default(), Default::default(), Default::default(), Default::default()),
			extrinsics: vec![
				UncheckedExtrinsic::new_unsigned(
					Default::default(),
					Call::Timestamp(timestamp::Call::set(100_000_000))
				)
			],
		};

		let raw = block.encode();
		let decoded = Block::decode(&mut &raw[..]).unwrap();

		assert_eq!(block, decoded);

		block.extrinsics.push(UncheckedExtrinsic::new_unsigned(
			10101,
			Call::Staking(staking::Call::stake())
		));

		let raw = block.encode();
		let decoded = Block::decode(&mut &raw[..]).unwrap();

		assert_eq!(block, decoded);
	}

	#[test]
	fn block_encoding_substrate_round_trip() {
		let mut block = Block {
			header: Header::new(1, Default::default(), Default::default(), Default::default(), Default::default()),
			extrinsics: vec![
				UncheckedExtrinsic::new_unsigned(
					Default::default(),
					Call::Timestamp(timestamp::Call::set(100_000_000))
				)
			],
		};

		block.extrinsics.push(UncheckedExtrinsic::new_unsigned(
			10101,
			Call::Staking(staking::Call::stake())
		));

		let raw = block.encode();
		let decoded_primitive = ::primitives::Block::decode(&mut &raw[..]).unwrap();
		let encoded_primitive = decoded_primitive.encode();
		let decoded = Block::decode(&mut &encoded_primitive[..]).unwrap();

		assert_eq!(block, decoded);
	}

	#[test]
	fn serialize_unchecked() {
		let tx = UncheckedExtrinsic::new_signed(
			999,
			Call::Timestamp(TimestampCall::set(135135)),
			AccountId::from([1; 32]).into(),
			runtime_primitives::Ed25519Signature(primitives::hash::H512([0; 64])).into()
		);

		// 6f000000
		// ff0101010101010101010101010101010101010101010101010101010101010101
		// e7030000
		// 0300
		// df0f0200
		// 0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000

		let v = Encode::encode(&tx);
		assert_eq!(&v[..], &hex!["7000000001ff010101010101010101010101010101010101010101010101010101010101010100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e70300000400df0f020000000000"][..]);
		println!("{}", HexDisplay::from(&v));
		assert_eq!(UncheckedExtrinsic::decode(&mut &v[..]).unwrap(), tx);
	}

	#[test]
	fn parachain_calls_are_privcall() {
		let _register = Call::Parachains(parachains::Call::register_parachain(0.into(), vec![1, 2, 3], vec![]));
		let _deregister = Call::Parachains(parachains::Call::deregister_parachain(0.into()));
	}
}
