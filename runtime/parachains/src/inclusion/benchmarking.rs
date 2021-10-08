use super::*;

use frame_benchmarking::benchmarks;
use futures::executor::block_on;
use keyring::Sr25519Keyring;
use sc_keystore::LocalKeystore;
use sp_keystore::SyncCryptoStorePtr;
use std::sync::Arc;

use primitives::v1::{
	CoreIndex, Id as ParaId, SignedAvailabilityBitfield, SigningContext,
	UncheckedSignedAvailabilityBitfield, ValidatorId, ValidatorIndex,
};

use crate::mock::{Configuration, Paras, System};

fn expected_bits() -> usize {
	Paras::parachains().len() + Configuration::config().parathread_cores as usize
}

fn default_bitfield() -> AvailabilityBitfield {
	AvailabilityBitfield(bitvec::bitvec![BitOrderLsb0, u8; 8; expected_bits()])
}

async fn sign_bitfield(
	keystore: &SyncCryptoStorePtr,
	key: &Sr25519Keyring,
	validator_index: ValidatorIndex,
	bitfield: AvailabilityBitfield,
	signing_context: &SigningContext,
) -> SignedAvailabilityBitfield {
	SignedAvailabilityBitfield::sign(
		keystore,
		bitfield,
		signing_context,
		validator_index,
		&key.public().into(),
	)
	.await
	.unwrap()
	.unwrap()
}

fn validator_pubkeys(validators: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	validators.iter().map(|v| v.public().into()).collect()
}

benchmarks! {
	process_bitfields {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);
		let thread_a = ParaId::from(3);


		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];
		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
		let validator_publics = validator_pubkeys(&validators);

		let signing_context = SigningContext { parent_hash: System::parent_hash(), session_index: 5 };

		let core_lookup = move |core| match core {
			core if core == CoreIndex::from(0) => Some(chain_a),
			core if core == CoreIndex::from(1) => Some(chain_b),
			core if core == CoreIndex::from(2) => Some(thread_a),
			core if core == CoreIndex::from(3) => None,
			_ => panic!("out of bounds for testing"),
		};

		let signed = block_on(sign_bitfield(
			&keystore,
			&validators[0],
			ValidatorIndex(0),
			default_bitfield(),
			&signing_context,
		));

	}: { Pallet::<T>::process_bitfields(expected_bits(), vec![signed.into()], core_lookup).unwrap() }

   impl_benchmark_test_suite!(
	   Pallet,
	   crate::mock::new_test_ext(Default::default()),
	   crate::mock::Test
   );
}
