// Copyright (C) Parity Technologies (UK) Ltd.
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

use honggfuzz::fuzz;
use polkadot_erasure_coding::*;
use polkadot_primitives::PersistedValidationData;
use primitives::{AvailableData, BlockData, PoV};
use std::sync::Arc;

fn main() {
	loop {
		fuzz!(|data: &[u8]| {
			let pov_block = PoV { block_data: BlockData(data.iter().cloned().collect()) };

			let available_data = AvailableData {
				pov: Arc::new(pov_block),
				validation_data: PersistedValidationData::default(),
			};
			let chunks = obtain_chunks_v1(10, &available_data).unwrap();

			assert_eq!(chunks.len(), 10);

			// any 4 chunks should work.
			let reconstructed: AvailableData = reconstruct_v1(
				10,
				[(&*chunks[1], 1), (&*chunks[4], 4), (&*chunks[6], 6), (&*chunks[9], 9)]
					.iter()
					.cloned(),
			)
			.unwrap();

			assert_eq!(reconstructed, available_data);
			println!("{:?}", reconstructed);
		});
	}
}
