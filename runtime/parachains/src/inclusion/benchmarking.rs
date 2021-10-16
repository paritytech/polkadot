use super::*;

use frame_benchmarking::benchmarks;
use futures::executor::block_on;
use keyring::Sr25519Keyring;
use sc_keystore::LocalKeystore;
use sp_keystore::SyncCryptoStorePtr;
use std::sync::Arc;

use primitives::v1::{
	BlockNumber, CoreIndex, Id as ParaId, SignedAvailabilityBitfield, SigningContext,
	UncheckedSignedAvailabilityBitfield, ValidationCode, ValidatorId, ValidatorIndex,
};

use super::mock::{Configuration, Paras, System};

#[derive(Default)]
struct TestCandidateBuilder {
	para_id: ParaId,
	head_data: HeadData,
	para_head_hash: Option<Hash>,
	pov_hash: Hash,
	relay_parent: Hash,
	persisted_validation_data_hash: Hash,
	new_validation_code: Option<ValidationCode>,
	validation_code: ValidationCode,
	hrmp_watermark: BlockNumber,
}

impl TestCandidateBuilder {
	fn build(self) -> CommittedCandidateReceipt {
		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				persisted_validation_data_hash: self.persisted_validation_data_hash,
				validation_code_hash: self.validation_code.hash(),
				para_head: self.para_head_hash.unwrap_or_else(|| self.head_data.hash()),
				..Default::default()
			},
			commitments: CandidateCommitments {
				head_data: self.head_data,
				new_validation_code: self.new_validation_code,
				hrmp_watermark: self.hrmp_watermark,
				..Default::default()
			},
		}
	}
}

fn expected_bits() -> usize {
	Paras::parachains().len() + Configuration::config().parathread_cores as usize
}

fn default_backing_bitfield() -> BitVec<BitOrderLsb0, u8> {
	bitvec::bitvec![BitOrderLsb0, u8; 0; ParasShared::active_validator_keys().len()]
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

		let candidate = TestCandidateBuilder::default().build();
	}: {
		<PendingAvailability<T>>::insert(
			&chain_b,
			CandidatePendingAvailability {
				core: CoreIndex::from(1),
				hash: candidate.hash(),
				descriptor: candidate.descriptor,
				availability_votes: default_availability_votes(),
				relay_parent_number: 6,
				backed_in_number: 7,
				backers: default_backing_bitfield(),
				backing_group: GroupIndex::from(1),
			},
		);
		Pallet::<T>::process_bitfields(expected_bits(), vec![signed.into()], core_lookup).unwrap() }

   impl_benchmark_test_suite!(
	   Pallet,
	   super::mock::new_test_ext(Default::default()),
	   super::mock::Test
   );
}
