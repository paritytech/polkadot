use polkadot_erasure_coding::*;
use primitives::v1::AvailableData;
use std::sync::Arc;
use honggfuzz::fuzz;

fn main(){
    loop {
        fuzz!(|data: (usize, Vec<(Vec<u8>, usize)>)| {
            let (num_validators, chunk_input) = data;
            if num_validators <= 1 || num_validators > 10_000 {
                return;
            }
            let reconstructed: Result<AvailableData, _> = reconstruct_v1(
                num_validators,
                chunk_input.iter().map(|t| (&*t.0, t.1)).collect::<Vec<(&[u8], usize)>>()
            );
            println!("reconstructed {:?}", reconstructed);
        });
    }
}
