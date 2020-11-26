use polkadot_erasure_coding::*;
use primitives::v1::{AvailableData, BlockData, PoV};
use std::sync::Arc;
use honggfuzz::fuzz;


fn main() {
	loop {
		fuzz!(|data: &[u8]| {
			let pov_block = PoV {
				block_data: BlockData(data.iter().cloned().collect()),
			};

			let available_data = AvailableData {
				pov: Arc::new(pov_block),
				validation_data: Default::default(),
			};
			let chunks = obtain_chunks_v1(
				10,
				&available_data,
			).unwrap();

			assert_eq!(chunks.len(), 10);

			// any 4 chunks should work.
			let reconstructed: AvailableData = reconstruct_v1(
				10,
				[
					(&*chunks[1], 1),
					(&*chunks[4], 4),
					(&*chunks[6], 6),
					(&*chunks[9], 9),
				].iter().cloned(),
			).unwrap();

			assert_eq!(reconstructed, available_data);
			println!("{:?}", reconstructed);
		});
	}
}
