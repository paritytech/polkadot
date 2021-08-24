use crate::{BlakeTwo256, BlockData};
use parity_scale_codec::{Decode, Encode};
use polkadot_erasure_coding::{branch_hash, branches, obtain_chunks_v0};
use polkadot_primitives::v0::{AvailableData, HashT, PoVBlock};

/// In order to adequately compute the number of entries in the Merkle
/// trie, we must account for the fixed 16-ary trie structure.
const KEY_INDEX_NIBBLE_SIZE: usize = 4;

fn generate_trie_and_generate_proofs(magnitude: u32) {
	let n_validators = 2_u32.pow(magnitude) as usize;
	let pov_block =
		PoVBlock { block_data: BlockData(vec![2; n_validators / KEY_INDEX_NIBBLE_SIZE]) };

	let available_data = AvailableData { pov_block, omitted_validation: Default::default() };

	let chunks = obtain_chunks_v0(magnitude as usize, &available_data).unwrap();

	assert_eq!(chunks.len() as u32, magnitude);

	let branches = branches(chunks.as_ref());
	let root = branches.root();

	let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();
	assert_eq!(proofs.len() as u32, magnitude);
	for (i, proof) in proofs.into_iter().enumerate() {
		let encode = Encode::encode(&proof);
		let decode = Decode::decode(&mut &encode[..]).unwrap();
		assert_eq!(proof, decode);
		assert_eq!(encode, Encode::encode(&decode));

		assert_eq!(branch_hash(&root, &proof, i).unwrap(), BlakeTwo256::hash(&chunks[i]));
	}
}

#[test]
fn roundtrip_proof_encoding() {
	for i in 2..16 {
		generate_trie_and_generate_proofs(i);
	}
}
