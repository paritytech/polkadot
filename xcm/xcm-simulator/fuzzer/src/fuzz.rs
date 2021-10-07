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

mod parachain;
mod relay_chain;

use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::traits::AccountIdConversion;
use xcm_simulator::{decl_test_network, decl_test_parachain, decl_test_relay_chain};
use honggfuzz::fuzz;
use xcm_simulator::TestExt;
use codec::{Decode, DecodeLimit, Encode};

use frame_support::assert_ok;
use xcm::latest::prelude::*;
use xcm::MAX_XCM_DECODE_DEPTH;


pub const ALICE: sp_runtime::AccountId32 = sp_runtime::AccountId32::new([0u8; 32]);
pub const INITIAL_BALANCE: u128 = 1_000_000_000;

decl_test_parachain! {
	pub struct ParaA {
		Runtime = parachain::Runtime,
		XcmpMessageHandler = parachain::MsgQueue,
		DmpMessageHandler = parachain::MsgQueue,
		new_ext = para_ext(1),
	}
}

decl_test_parachain! {
	pub struct ParaB {
		Runtime = parachain::Runtime,
		XcmpMessageHandler = parachain::MsgQueue,
		DmpMessageHandler = parachain::MsgQueue,
		new_ext = para_ext(2),
	}
}

decl_test_relay_chain! {
	pub struct Relay {
		Runtime = relay_chain::Runtime,
		XcmConfig = relay_chain::XcmConfig,
		new_ext = relay_ext(),
	}
}

decl_test_network! {
	pub struct MockNet {
		relay_chain = Relay,
		parachains = vec![
			(1, ParaA),
			(2, ParaB),
		],
	}
}

pub fn para_account_id(id: u32) -> relay_chain::AccountId {
	ParaId::from(id).into_account()
}

pub fn para_ext(para_id: u32) -> sp_io::TestExternalities {
	use parachain::{MsgQueue, Runtime, System};

	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	pallet_balances::GenesisConfig::<Runtime> { balances: vec![(ALICE, INITIAL_BALANCE)] }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| {
		System::set_block_number(1);
		MsgQueue::set_para_id(para_id.into());
	});
	ext
}

pub fn relay_ext() -> sp_io::TestExternalities {
	use relay_chain::{Runtime, System};

	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	pallet_balances::GenesisConfig::<Runtime> {
		balances: vec![(ALICE, INITIAL_BALANCE), (para_account_id(1), INITIAL_BALANCE)],
	}
	.assimilate_storage(&mut t)
	.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

pub type RelayChainPalletXcm = pallet_xcm::Pallet<relay_chain::Runtime>;
pub type ParachainPalletXcm = pallet_xcm::Pallet<parachain::Runtime>;

fn run_one_input(data: &[u8]) {
	MockNet::reset();
	if let Ok(m) = Xcm::decode_all_with_depth_limit(MAX_XCM_DECODE_DEPTH, data){
		#[cfg(not(fuzzing))]
		{
			println!("Executing message {:?}", m);
		}
		ParaA::execute_with(|| {
			ParachainPalletXcm::send_xcm(
				Here,
				Parent.into(),
				m,
			);
		});
		Relay::execute_with(|| {
			use relay_chain::{Event, System};
		});
	}
}

fn main() {
    #[cfg(fuzzing)]
    {
        loop {
            fuzz!(|data: &[u8]| {
                run_one_input(data);
            });
        }
    }
    #[cfg(not(fuzzing))]
    {
        use std::env;
        use std::fs;
        use std::fs::File;
        use std::io::Read;
        let args: Vec<_> = env::args().collect();
        let md = fs::metadata(&args[1]).unwrap();
        let all_files = match md.is_dir() {
            true => fs::read_dir(&args[1])
                .unwrap()
                .map(|x| x.unwrap().path().to_str().unwrap().to_string())
                .collect::<Vec<String>>(),
            false => (&args[1..]).to_vec(),
        };
        println!("All_files {:?}", all_files);
        for argument in all_files {
            println!("Now doing file {:?}", argument);
            let mut buffer: Vec<u8> = Vec::new();
            let mut f = File::open(argument).unwrap();
            f.read_to_end(&mut buffer).unwrap();
            run_one_input(&buffer.as_slice());
        }
    }
}
