use polkadot_erasure_coding::*;
use primitives::AvailableData;
use honggfuzz::fuzz;

fn main() {
	loop {
		fuzz!(|data: (usize, Vec<(Vec<u8>, usize)>)| {
			let (num_validators, chunk_input) = data;
			let reconstructed: Result<AvailableData, _> = reconstruct_v1(
				num_validators,
				chunk_input.iter().map(|t| (&*t.0, t.1)).collect::<Vec<(&[u8], usize)>>()
			);
			println!("reconstructed {:?}", reconstructed);
		});
	}
}
