// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! WASM validation for adder parachain.

use crate::{HeadData, BlockData};
use core::{intrinsics, panic};
use parachain::primitives::{ValidationResult, HeadData as GenericHeadData};
use codec::{Encode, Decode};

#[no_mangle]
pub extern fn validate_block(params: *const u8, len: usize) -> u64 {
	let params = unsafe { parachain::load_params(params, len) };
	let parent_head = HeadData::decode(&mut &params.parent_head.0[..])
		.expect("invalid parent head format.");

	let block_data = BlockData::decode(&mut &params.block_data.0[..])
		.expect("invalid block data format.");

	let parent_hash = tiny_keccak::keccak256(&params.parent_head.0[..]);

	match crate::execute(parent_hash, parent_head, &block_data) {
		Ok(new_head) => parachain::write_result(
			&ValidationResult {
				head_data: GenericHeadData(new_head.encode()),
				new_validation_code: None,
			}
		),
		Err(_) => panic!("execution failure"),
	}
}
