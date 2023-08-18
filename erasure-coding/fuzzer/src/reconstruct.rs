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
use primitives::AvailableData;

fn main() {
	loop {
		fuzz!(|data: (usize, Vec<(Vec<u8>, usize)>)| {
			let (num_validators, chunk_input) = data;
			let reconstructed: Result<AvailableData, _> = reconstruct_v1(
				num_validators,
				chunk_input.iter().map(|t| (&*t.0, t.1)).collect::<Vec<(&[u8], usize)>>(),
			);
			println!("reconstructed {:?}", reconstructed);
		});
	}
}
