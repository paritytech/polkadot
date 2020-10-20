// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The provisioner is responsible for assembling a relay chain block
//! from a set of available parachain candidates of its choice.

#![deny(missing_docs)]

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::{
		AllMessages, ChainApiMessage, ProvisionableData, ProvisionerInherentData,
		ProvisionerMessage, RuntimeApiMessage,
	},
};
use polkadot_node_subsystem_util::{
	self as util,
	delegated_subsystem,
	request_availability_cores, request_persisted_validation_data, JobTrait, ToJobTrait,
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{
	BackedCandidate, BlockNumber, CoreState, Hash, OccupiedCoreAssumption,
	SignedAvailabilityBitfield,
};
use std::{collections::HashMap, convert::TryFrom, pin::Pin};

struct ProvisioningJob {
	relay_parent: Hash,
	sender: mpsc::Sender<FromJob>,
	receiver: mpsc::Receiver<ToJob>,
	provisionable_data_channels: Vec<mpsc::Sender<ProvisionableData>>,
	backed_candidates: Vec<BackedCandidate>,
	signed_bitfields: Vec<SignedAvailabilityBitfield>,
	metrics: Metrics,
}

/// This enum defines the messages that the provisioner is prepared to receive.
pub enum ToJob {
	/// The provisioner message is the main input to the provisioner.
	Provisioner(ProvisionerMessage),
	/// This message indicates that the provisioner should shut itself down.
	Stop,
}

impl ToJobTrait for ToJob {
	const STOP: Self = Self::Stop;

	fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::Provisioner(pm) => pm.relay_parent(),
			Self::Stop => None,
		}
	}
}

impl TryFrom<AllMessages> for ToJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::Provisioner(pm) => Ok(Self::Provisioner(pm)),
			_ => Err(()),
		}
	}
}

impl From<ProvisionerMessage> for ToJob {
	fn from(pm: ProvisionerMessage) -> Self {
		Self::Provisioner(pm)
	}
}

enum FromJob {
	ChainApi(ChainApiMessage),
	Runtime(RuntimeApiMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::ChainApi(cam) => AllMessages::ChainApi(cam),
			FromJob::Runtime(ram) => AllMessages::RuntimeApi(ram),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::ChainApi(chain) => Ok(FromJob::ChainApi(chain)),
			AllMessages::RuntimeApi(runtime) => Ok(FromJob::Runtime(runtime)),
			_ => Err(()),
		}
	}
}

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Sending(mpsc::SendError),
	#[from]
	Util(util::Error),
	#[from]
	OneshotRecv(oneshot::Canceled),
	#[from]
	ChainApi(ChainApiError),
	#[from]
	Runtime(RuntimeApiError),
	OneshotSend,
}

impl JobTrait for ProvisioningJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = ();
	type Metrics = Metrics;

	const NAME: &'static str = "ProvisioningJob";

	/// Run a job for the parent block indicated
	//
	// this function is in charge of creating and executing the job's main loop
	fn run(
		relay_parent: Hash,
		_run_args: Self::RunArgs,
		metrics: Self::Metrics,
		receiver: mpsc::Receiver<ToJob>,
		sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = ProvisioningJob::new(relay_parent, metrics, sender, receiver);

			// it isn't necessary to break run_loop into its own function,
			// but it's convenient to separate the concerns in this way
			job.run_loop().await
		}
		.boxed()
	}
}

impl ProvisioningJob {
	pub fn new(
		relay_parent: Hash,
		metrics: Metrics,
		sender: mpsc::Sender<FromJob>,
		receiver: mpsc::Receiver<ToJob>,
	) -> Self {
		Self {
			relay_parent,
			sender,
			receiver,
			provisionable_data_channels: Vec::new(),
			backed_candidates: Vec::new(),
			signed_bitfields: Vec::new(),
			metrics,
		}
	}

	async fn run_loop(mut self) -> Result<(), Error> {
		while let Some(msg) = self.receiver.next().await {
			use ProvisionerMessage::{
				ProvisionableData, RequestBlockAuthorshipData, RequestInherentData,
			};

			match msg {
				ToJob::Provisioner(RequestInherentData(_, return_sender)) => {
					if let Err(err) = send_inherent_data(
						self.relay_parent,
						&self.signed_bitfields,
						&self.backed_candidates,
						return_sender,
						self.sender.clone(),
					)
					.await
					{
						log::warn!(target: "provisioner", "failed to assemble or send inherent data: {:?}", err);
						self.metrics.on_inherent_data_request(Err(()));
					} else {
						self.metrics.on_inherent_data_request(Ok(()));
					}
				}
				ToJob::Provisioner(RequestBlockAuthorshipData(_, sender)) => {
					self.provisionable_data_channels.push(sender)
				}
				ToJob::Provisioner(ProvisionableData(data)) => {
					let mut bad_indices = Vec::new();
					for (idx, channel) in self.provisionable_data_channels.iter_mut().enumerate() {
						match channel.send(data.clone()).await {
							Ok(_) => {}
							Err(_) => bad_indices.push(idx),
						}
					}
					self.note_provisionable_data(data);

					// clean up our list of channels by removing the bad indices
					// start by reversing it for efficient pop
					bad_indices.reverse();
					// Vec::retain would be nicer here, but it doesn't provide
					// an easy API for retaining by index, so we re-collect instead.
					self.provisionable_data_channels = self
						.provisionable_data_channels
						.into_iter()
						.enumerate()
						.filter(|(idx, _)| {
							if bad_indices.is_empty() {
								return true;
							}
							let tail = bad_indices[bad_indices.len() - 1];
							let retain = *idx != tail;
							if *idx >= tail {
								bad_indices.pop();
							}
							retain
						})
						.map(|(_, item)| item)
						.collect();
				}
				ToJob::Stop => break,
			}
		}

		Ok(())
	}

	fn note_provisionable_data(&mut self, provisionable_data: ProvisionableData) {
		match provisionable_data {
			ProvisionableData::Bitfield(_, signed_bitfield) => {
				self.signed_bitfields.push(signed_bitfield)
			}
			ProvisionableData::BackedCandidate(backed_candidate) => {
				self.backed_candidates.push(backed_candidate)
			}
			_ => {}
		}
	}
}

type CoreAvailability = BitVec<bitvec::order::Lsb0, u8>;

// The provisioner is the subsystem best suited to choosing which specific
// backed candidates and availability bitfields should be assembled into the
// block. To engage this functionality, a
// `ProvisionerMessage::RequestInherentData` is sent; the response is a set of
// non-conflicting candidates and the appropriate bitfields. Non-conflicting
// means that there are never two distinct parachain candidates included for
// the same parachain and that new parachain candidates cannot be included
// until the previous one either gets declared available or expired.
//
// The main complication here is going to be around handling
// occupied-core-assumptions. We might have candidates that are only
// includable when some bitfields are included. And we might have candidates
// that are not includable when certain bitfields are included.
//
// When we're choosing bitfields to include, the rule should be simple:
// maximize availability. So basically, include all bitfields. And then
// choose a coherent set of candidates along with that.
async fn send_inherent_data(
	relay_parent: Hash,
	bitfields: &[SignedAvailabilityBitfield],
	candidates: &[BackedCandidate],
	return_sender: oneshot::Sender<ProvisionerInherentData>,
	mut from_job: mpsc::Sender<FromJob>,
) -> Result<(), Error> {
	let availability_cores = request_availability_cores(relay_parent, &mut from_job)
		.await?
		.await??;

	let bitfields = select_availability_bitfields(&availability_cores, bitfields);
	let candidates = select_candidates(
		&availability_cores,
		&bitfields,
		candidates,
		relay_parent,
		&mut from_job,
	)
	.await?;

	return_sender
		.send((bitfields, candidates))
		.map_err(|_| Error::OneshotSend)?;
	Ok(())
}

// in general, we want to pick all the bitfields. However, we have the following constraints:
//
// - not more than one per validator
// - each must correspond to an occupied core
//
// If we have too many, an arbitrary selection policy is fine. For purposes of maximizing availability,
// we pick the one with the greatest number of 1 bits.
//
// note: this does not enforce any sorting precondition on the output; the ordering there will be unrelated
// to the sorting of the input.
fn select_availability_bitfields(
	cores: &[CoreState],
	bitfields: &[SignedAvailabilityBitfield],
) -> Vec<SignedAvailabilityBitfield> {
	let mut fields_by_core: HashMap<_, Vec<_>> = HashMap::new();
	for bitfield in bitfields.iter() {
		let core_idx = bitfield.validator_index() as usize;
		if let CoreState::Occupied(_) = cores[core_idx] {
			fields_by_core
				.entry(core_idx)
				// there cannot be a value list in field_by_core with len < 1
				.or_default()
				.push(bitfield.clone());
		}
	}

	let mut out = Vec::with_capacity(fields_by_core.len());
	for (_, core_bitfields) in fields_by_core.iter_mut() {
		core_bitfields.sort_by_key(|bitfield| bitfield.payload().0.count_ones());
		out.push(
			core_bitfields
				.pop()
				.expect("every core bitfield has at least 1 member; qed"),
		);
	}

	out
}

// determine which cores are free, and then to the degree possible, pick a candidate appropriate to each free core.
//
// follow the candidate selection algorithm from the guide
async fn select_candidates(
	availability_cores: &[CoreState],
	bitfields: &[SignedAvailabilityBitfield],
	candidates: &[BackedCandidate],
	relay_parent: Hash,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<Vec<BackedCandidate>, Error> {
	let block_number = get_block_number_under_construction(relay_parent, sender).await?;

	let mut selected_candidates =
		Vec::with_capacity(candidates.len().min(availability_cores.len()));

	for (core_idx, core) in availability_cores.iter().enumerate() {
		let (scheduled_core, assumption) = match core {
			CoreState::Scheduled(scheduled_core) => (scheduled_core, OccupiedCoreAssumption::Free),
			CoreState::Occupied(occupied_core) => {
				if bitfields_indicate_availability(core_idx, bitfields, &occupied_core.availability)
				{
					if let Some(ref scheduled_core) = occupied_core.next_up_on_available {
						(scheduled_core, OccupiedCoreAssumption::Included)
					} else {
						continue;
					}
				} else {
					if occupied_core.time_out_at != block_number {
						continue;
					}
					if let Some(ref scheduled_core) = occupied_core.next_up_on_time_out {
						(scheduled_core, OccupiedCoreAssumption::TimedOut)
					} else {
						continue;
					}
				}
			}
			_ => continue,
		};

		let validation_data = match request_persisted_validation_data(
			relay_parent,
			scheduled_core.para_id,
			assumption,
			sender,
		)
		.await?
		.await??
		{
			Some(v) => v,
			None => continue,
		};

		let computed_validation_data_hash = validation_data.hash();

		// we arbitrarily pick the first of the backed candidates which match the appropriate selection criteria
		if let Some(candidate) = candidates.iter().find(|backed_candidate| {
			let descriptor = &backed_candidate.candidate.descriptor;
			descriptor.para_id == scheduled_core.para_id
				&& descriptor.persisted_validation_data_hash == computed_validation_data_hash
		}) {
			selected_candidates.push(candidate.clone());
		}
	}

	Ok(selected_candidates)
}

// produces a block number 1 higher than that of the relay parent
// in the event of an invalid `relay_parent`, returns `Ok(0)`
async fn get_block_number_under_construction(
	relay_parent: Hash,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<BlockNumber, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send(FromJob::ChainApi(ChainApiMessage::BlockNumber(
			relay_parent,
			tx,
		)))
		.await
		.map_err(|_| Error::OneshotSend)?;
	match rx.await? {
		Ok(Some(n)) => Ok(n + 1),
		Ok(None) => Ok(0),
		Err(err) => Err(err.into()),
	}
}

// the availability bitfield for a given core is the transpose
// of a set of signed availability bitfields. It goes like this:
//
//   - construct a transverse slice along `core_idx`
//   - bitwise-or it with the availability slice
//   - count the 1 bits, compare to the total length; true on 2/3+
fn bitfields_indicate_availability(
	core_idx: usize,
	bitfields: &[SignedAvailabilityBitfield],
	availability: &CoreAvailability,
) -> bool {
	let mut availability = availability.clone();
	// we need to pre-compute this to avoid a borrow-immutable-while-borrowing-mutable error in the error message
	let availability_len = availability.len();

	for bitfield in bitfields {
		let validator_idx = bitfield.validator_index() as usize;
		match availability.get_mut(validator_idx) {
			None => {
				// in principle, this function might return a `Result<bool, Error>` so that we can more clearly express this error condition
				// however, in practice, that would just push off an error-handling routine which would look a whole lot like this one.
				// simpler to just handle the error internally here.
				log::warn!(target: "provisioner", "attempted to set a transverse bit at idx {} which is greater than bitfield size {}", validator_idx, availability_len);
				return false;
			}
			Some(mut bit_mut) => *bit_mut |= bitfield.payload().0[core_idx],
		}
	}
	3 * availability.count_ones() >= 2 * availability.len()
}

#[derive(Clone)]
struct MetricsInner {
	inherent_data_requests: prometheus::CounterVec<prometheus::U64>,
}

/// Provisioner metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_inherent_data_request(&self, response: Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			match response {
				Ok(()) => metrics.inherent_data_requests.with_label_values(&["succeeded"]).inc(),
				Err(()) => metrics.inherent_data_requests.with_label_values(&["failed"]).inc(),
			}
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			inherent_data_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_inherent_data_requests_total",
						"Number of InherentData requests served by provisioner.",
					),
					&["success"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}


delegated_subsystem!(ProvisioningJob((), Metrics) <- ToJob as ProvisioningSubsystem);

#[cfg(test)]
mod tests {
	use super::*;
	use bitvec::bitvec;
	use polkadot_primitives::v1::{OccupiedCore, ScheduledCore};

	pub fn occupied_core(para_id: u32) -> CoreState {
		CoreState::Occupied(OccupiedCore {
			para_id: para_id.into(),
			group_responsible: para_id.into(),
			next_up_on_available: None,
			occupied_since: 100_u32,
			time_out_at: 200_u32,
			next_up_on_time_out: None,
			availability: default_bitvec(),
		})
	}

	pub fn build_occupied_core<Builder>(para_id: u32, builder: Builder) -> CoreState
	where
		Builder: FnOnce(&mut OccupiedCore),
	{
		let mut core = match occupied_core(para_id) {
			CoreState::Occupied(core) => core,
			_ => unreachable!(),
		};

		builder(&mut core);

		CoreState::Occupied(core)
	}

	pub fn default_bitvec() -> CoreAvailability {
		bitvec![bitvec::order::Lsb0, u8; 0; 32]
	}

	pub fn scheduled_core(id: u32) -> ScheduledCore {
		ScheduledCore {
			para_id: id.into(),
			..Default::default()
		}
	}

	mod select_availability_bitfields {
		use super::super::*;
		use super::{default_bitvec, occupied_core};
		use futures::executor::block_on;
		use std::sync::Arc;
		use polkadot_primitives::v1::{SigningContext, ValidatorIndex, ValidatorId};
		use sp_application_crypto::AppKey;
		use sp_keystore::{CryptoStore, SyncCryptoStorePtr};
		use sc_keystore::LocalKeystore;

		async fn signed_bitfield(
			keystore: &SyncCryptoStorePtr,
			field: CoreAvailability,
			validator_idx: ValidatorIndex,
		) -> SignedAvailabilityBitfield {
			let public = CryptoStore::sr25519_generate_new(&**keystore, ValidatorId::ID, None)
				.await
				.expect("generated sr25519 key");
			SignedAvailabilityBitfield::sign(
				&keystore,
				field.into(),
				&<SigningContext<Hash>>::default(),
				validator_idx,
				&public.into(),
			).await.expect("Should be signed")
		}

		#[test]
		fn not_more_than_one_per_validator() {
			// Configure filesystem-based keystore as generating keys without seed
			// would trigger the key to be generated on the filesystem.
			let keystore_path = tempfile::tempdir().expect("Creates keystore path");
			let keystore : SyncCryptoStorePtr = Arc::new(LocalKeystore::open(keystore_path.path(), None)
				.expect("Creates keystore"));
			let bitvec = default_bitvec();

			let cores = vec![occupied_core(0), occupied_core(1)];

			// we pass in three bitfields with two validators
			// this helps us check the postcondition that we get two bitfields back, for which the validators differ
			let bitfields = vec![
				block_on(signed_bitfield(&keystore, bitvec.clone(), 0)),
				block_on(signed_bitfield(&keystore, bitvec.clone(), 1)),
				block_on(signed_bitfield(&keystore, bitvec, 1)),
			];

			let mut selected_bitfields = select_availability_bitfields(&cores, &bitfields);
			selected_bitfields.sort_by_key(|bitfield| bitfield.validator_index());

			assert_eq!(selected_bitfields.len(), 2);
			assert_eq!(selected_bitfields[0], bitfields[0]);
			// we don't know which of the (otherwise equal) bitfields will be selected
			assert!(selected_bitfields[1] == bitfields[1] || selected_bitfields[1] == bitfields[2]);
		}

		#[test]
		fn each_corresponds_to_an_occupied_core() {
			// Configure filesystem-based keystore as generating keys without seed
			// would trigger the key to be generated on the filesystem.
			let keystore_path = tempfile::tempdir().expect("Creates keystore path");
			let keystore : SyncCryptoStorePtr = Arc::new(LocalKeystore::open(keystore_path.path(), None)
				.expect("Creates keystore"));
			let bitvec = default_bitvec();

			let cores = vec![CoreState::Free, CoreState::Scheduled(Default::default())];

			let bitfields = vec![
				block_on(signed_bitfield(&keystore, bitvec.clone(), 0)),
				block_on(signed_bitfield(&keystore, bitvec.clone(), 1)),
				block_on(signed_bitfield(&keystore, bitvec, 1)),
			];

			let mut selected_bitfields = select_availability_bitfields(&cores, &bitfields);
			selected_bitfields.sort_by_key(|bitfield| bitfield.validator_index());

			// bitfields not corresponding to occupied cores are not selected
			assert!(selected_bitfields.is_empty());
		}

		#[test]
		fn more_set_bits_win_conflicts() {
			// Configure filesystem-based keystore as generating keys without seed
			// would trigger the key to be generated on the filesystem.
			let keystore_path = tempfile::tempdir().expect("Creates keystore path");
			let keystore : SyncCryptoStorePtr = Arc::new(LocalKeystore::open(keystore_path.path(), None)
				.expect("Creates keystore"));
			let bitvec_zero = default_bitvec();
			let bitvec_one = {
				let mut bitvec = bitvec_zero.clone();
				bitvec.set(0, true);
				bitvec
			};

			let cores = vec![occupied_core(0)];

			let bitfields = vec![
				block_on(signed_bitfield(&keystore, bitvec_zero, 0)),
				block_on(signed_bitfield(&keystore, bitvec_one.clone(), 0)),
			];

			// this test is probablistic: chances are excellent that it does what it claims to.
			// it cannot fail unless things are broken.
			// however, there is a (very small) chance that it passes when things are broken.
			for _ in 0..64 {
				let selected_bitfields = select_availability_bitfields(&cores, &bitfields);
				assert_eq!(selected_bitfields.len(), 1);
				assert_eq!(selected_bitfields[0].payload().0, bitvec_one);
			}
		}
	}

	mod select_candidates {
		use futures_timer::Delay;
		use super::super::*;
		use super::{build_occupied_core, default_bitvec, occupied_core, scheduled_core};
		use polkadot_node_subsystem::messages::RuntimeApiRequest::{
			AvailabilityCores, PersistedValidationData as PersistedValidationDataReq,
		};
		use polkadot_primitives::v1::{
			BlockNumber, CandidateDescriptor, CommittedCandidateReceipt, PersistedValidationData,
		};
		use FromJob::{ChainApi, Runtime};

		const BLOCK_UNDER_PRODUCTION: BlockNumber = 128;

		fn test_harness<OverseerFactory, Overseer, TestFactory, Test>(
			overseer_factory: OverseerFactory,
			test_factory: TestFactory,
		) where
			OverseerFactory: FnOnce(mpsc::Receiver<FromJob>) -> Overseer,
			Overseer: Future<Output = ()>,
			TestFactory: FnOnce(mpsc::Sender<FromJob>) -> Test,
			Test: Future<Output = ()>,
		{
			let (tx, rx) = mpsc::channel(64);
			let overseer = overseer_factory(rx);
			let test = test_factory(tx);

			futures::pin_mut!(overseer, test);

			futures::executor::block_on(future::select(overseer, test));
		}

		// For test purposes, we always return this set of availability cores:
		//
		//   [
		//      0: Free,
		//      1: Scheduled(default),
		//      2: Occupied(no next_up set),
		//      3: Occupied(next_up_on_available set but not available),
		//      4: Occupied(next_up_on_available set and available),
		//      5: Occupied(next_up_on_time_out set but not timeout),
		//      6: Occupied(next_up_on_time_out set and timeout but available),
		//      7: Occupied(next_up_on_time_out set and timeout and not available),
		//      8: Occupied(both next_up set, available),
		//      9: Occupied(both next_up set, not available, no timeout),
		//     10: Occupied(both next_up set, not available, timeout),
		//     11: Occupied(next_up_on_available and available, but different successor para_id)
		//   ]
		fn mock_availability_cores() -> Vec<CoreState> {
			use std::ops::Not;
			use CoreState::{Free, Scheduled};

			vec![
				// 0: Free,
				Free,
				// 1: Scheduled(default),
				Scheduled(scheduled_core(1)),
				// 2: Occupied(no next_up set),
				occupied_core(2),
				// 3: Occupied(next_up_on_available set but not available),
				build_occupied_core(3, |core| {
					core.next_up_on_available = Some(scheduled_core(3));
				}),
				// 4: Occupied(next_up_on_available set and available),
				build_occupied_core(4, |core| {
					core.next_up_on_available = Some(scheduled_core(4));
					core.availability = core.availability.clone().not();
				}),
				// 5: Occupied(next_up_on_time_out set but not timeout),
				build_occupied_core(5, |core| {
					core.next_up_on_time_out = Some(scheduled_core(5));
				}),
				// 6: Occupied(next_up_on_time_out set and timeout but available),
				build_occupied_core(6, |core| {
					core.next_up_on_time_out = Some(scheduled_core(6));
					core.time_out_at = BLOCK_UNDER_PRODUCTION;
					core.availability = core.availability.clone().not();
				}),
				// 7: Occupied(next_up_on_time_out set and timeout and not available),
				build_occupied_core(7, |core| {
					core.next_up_on_time_out = Some(scheduled_core(7));
					core.time_out_at = BLOCK_UNDER_PRODUCTION;
				}),
				// 8: Occupied(both next_up set, available),
				build_occupied_core(8, |core| {
					core.next_up_on_available = Some(scheduled_core(8));
					core.next_up_on_time_out = Some(scheduled_core(8));
					core.availability = core.availability.clone().not();
				}),
				// 9: Occupied(both next_up set, not available, no timeout),
				build_occupied_core(9, |core| {
					core.next_up_on_available = Some(scheduled_core(9));
					core.next_up_on_time_out = Some(scheduled_core(9));
				}),
				// 10: Occupied(both next_up set, not available, timeout),
				build_occupied_core(10, |core| {
					core.next_up_on_available = Some(scheduled_core(10));
					core.next_up_on_time_out = Some(scheduled_core(10));
					core.time_out_at = BLOCK_UNDER_PRODUCTION;
				}),
				// 11: Occupied(next_up_on_available and available, but different successor para_id)
				build_occupied_core(11, |core| {
					core.next_up_on_available = Some(scheduled_core(12));
					core.availability = core.availability.clone().not();
				}),
			]
		}

		async fn mock_overseer(mut receiver: mpsc::Receiver<FromJob>) {
			use ChainApiMessage::BlockNumber;
			use RuntimeApiMessage::Request;

			while let Some(from_job) = receiver.next().await {
				match from_job {
					ChainApi(BlockNumber(_relay_parent, tx)) => {
						tx.send(Ok(Some(BLOCK_UNDER_PRODUCTION - 1))).unwrap()
					}
					Runtime(Request(
						_parent_hash,
						PersistedValidationDataReq(_para_id, _assumption, tx),
					)) => tx.send(Ok(Some(Default::default()))).unwrap(),
					Runtime(Request(_parent_hash, AvailabilityCores(tx))) => {
						tx.send(Ok(mock_availability_cores())).unwrap()
					}
					// non-exhaustive matches are fine for testing
					_ => unimplemented!(),
				}
			}
		}

		#[test]
		fn handles_overseer_failure() {
			let overseer = |rx: mpsc::Receiver<FromJob>| async move {
				// drop the receiver so it closes and the sender can't send, then just sleep long enough that
				// this is almost certainly not the first of the two futures to complete
				std::mem::drop(rx);
				Delay::new(std::time::Duration::from_secs(1)).await;
			};

			let test = |mut tx: mpsc::Sender<FromJob>| async move {
				// wait so that the overseer can drop the rx before we attempt to send
				Delay::new(std::time::Duration::from_millis(50)).await;
				let result = select_candidates(&[], &[], &[], Default::default(), &mut tx).await;
				println!("{:?}", result);
				assert!(std::matches!(result, Err(Error::OneshotSend)));
			};

			test_harness(overseer, test);
		}

		#[test]
		fn can_succeed() {
			test_harness(mock_overseer, |mut tx: mpsc::Sender<FromJob>| async move {
				let result = select_candidates(&[], &[], &[], Default::default(), &mut tx).await;
				println!("{:?}", result);
				assert!(result.is_ok());
			})
		}

		// this tests that only the appropriate candidates get selected.
		// To accomplish this, we supply a candidate list containing one candidate per possible core;
		// the candidate selection algorithm must filter them to the appropriate set
		#[test]
		fn selects_correct_candidates() {
			let mock_cores = mock_availability_cores();

			let empty_hash = PersistedValidationData::<BlockNumber>::default().hash();

			let candidate_template = BackedCandidate {
				candidate: CommittedCandidateReceipt {
					descriptor: CandidateDescriptor {
						persisted_validation_data_hash: empty_hash,
						..Default::default()
					},
					..Default::default()
				},
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(),
			};

			let candidates: Vec<_> = std::iter::repeat(candidate_template)
				.take(mock_cores.len())
				.enumerate()
				.map(|(idx, mut candidate)| {
					candidate.candidate.descriptor.para_id = idx.into();
					candidate
				})
				.cycle()
				.take(mock_cores.len() * 3)
				.enumerate()
				.map(|(idx, mut candidate)| {
					if idx < mock_cores.len() {
						// first go-around: use candidates which should work
						candidate
					} else if idx < mock_cores.len() * 2 {
						// for the second repetition of the candidates, give them the wrong hash
						candidate.candidate.descriptor.persisted_validation_data_hash
							= Default::default();
						candidate
					} else {
						// third go-around: right hash, wrong para_id
						candidate.candidate.descriptor.para_id = idx.into();
						candidate
					}
				})
				.collect();

			// why those particular indices? see the comments on mock_availability_cores()
			let expected_candidates: Vec<_> = [1, 4, 7, 8, 10]
				.iter()
				.map(|&idx| candidates[idx].clone())
				.collect();

			test_harness(mock_overseer, |mut tx: mpsc::Sender<FromJob>| async move {
				let result =
					select_candidates(&mock_cores, &[], &candidates, Default::default(), &mut tx)
						.await;

				if result.is_err() {
					println!("{:?}", result);
				}
				assert_eq!(result.unwrap(), expected_candidates);
			})
		}
	}
}
