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

//! Implements a `CandidateBackingSubsystem`.

#![recursion_limit="256"]

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use log;
use futures::{
	select, FutureExt, SinkExt, StreamExt,
	channel::{oneshot, mpsc},
	future::{self, Either},
	task::{Spawn, SpawnExt},
};
use futures_timer::Delay;
use streamunordered::{StreamUnordered, StreamYield};

use primitives::Pair;
use keystore::KeyStorePtr;
use polkadot_primitives::{
	Hash, BlockNumber,
	parachain::{
		AbridgedCandidateReceipt, Id as ParaId, ValidatorPair, ValidatorId, ValidatorIndex,
		HeadData, ValidationCode, SigningContext, PoVBlock, GroupIndex,
	},
};
use polkadot_overseer::{Subsystem, SubsystemContext, SpawnedSubsystem};
use polkadot_node_primitives::{Statement, SignedFullStatement};
use messages::{
	AllMessages, AvailabilityStoreMessage, FromOverseer, CandidateBackingMessage, SchedulerRoster,
	OverseerSignal, RuntimeApiMessage, RuntimeApiRequest, CandidateValidationMessage, ValidationFailed,
	StatementDistributionMessage, NewBackedCandidate,
};

#[derive(Debug, derive_more::From)]
enum Error {
	NotInValidatorSet,
	#[from]
	ValidationFailed(ValidationFailed),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Mpsc(mpsc::SendError),
}

/// Holds all data needed for candidate backing job operation.
struct CandidateBackingJob {
	/// The keystore which holds the signing keys.
	keystore: KeyStorePtr,
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
	/// Inbound message channel receiving part.
	rx_to: mpsc::Receiver<ToJob>,
	/// Inbound message sending part. Handed out to whom it may concern.
	tx_to: mpsc::Sender<ToJob>,
	/// Outbound message channel sending part.
	tx_from: mpsc::Sender<FromJob>,

	/// Validator set.
	validators: Vec<ValidatorId>,
	/// The validation codes of the `ParaId`s we are assigned as validators to.
	validation_code: HashMap<ParaId, ValidationCode>,
	/// `HeadData`s of the parachains that this validator is assigned to.
	head_data: HashMap<ParaId, HeadData>,
	/// `SigningContext` to use when signing.
	signing_context: SigningContext,
	/// This validator's signing key.
	signing_key: Arc<ValidatorPair>,
	/// Index of this validator.
	index: ValidatorIndex,
	/// The `ParaId`s assigned to this validator.
	assigned_ids: HashSet<ParaId>,
	/// We issued `Valid` statements on about these candidates.
	issued_validity: HashSet<Hash>,
	/// `Some(h)` if this job has already issues `Seconded` statemt for some candidate with `h` hash.
	seconded: Option<Hash>,
	/// Registered backing watchers.
	backing_watchers: HashMap<Hash, Vec<mpsc::Sender<NewBackedCandidate>>>,
	/// Valid votes by candidate hash.
	valid_votes: HashMap<Hash, Vec<ValidatorIndex>>,
}

const CHANNEL_CAPACITY: usize = 64;

/// A message type that is sent from `CandidateBackingSubsystem` to `CandidateBackingJob`.
enum ToJob {
	/// A `CandidateBackingMessage`.
	CandidateBacking(CandidateBackingMessage),
	/// Stop working.
	Stop,
}

/// A message type that is sent from `CandidateBackingJob` to `CandidateBackingSubsystem`.
enum FromJob {
	RuntimeApiMessage(RuntimeApiMessage),
	AvailabilityStoreMessage(AvailabilityStoreMessage),
	CandidateValidation(CandidateValidationMessage),
	StatementDistribution(StatementDistributionMessage),
}

impl From<FromJob> for AllMessages {
	fn from(f: FromJob) -> Self {
		match f {
			FromJob::RuntimeApiMessage(msg) => AllMessages::RuntimeApi(msg),
			FromJob::AvailabilityStoreMessage(msg) => AllMessages::AvailabilityStore(msg),
			FromJob::CandidateValidation(msg) => AllMessages::CandidateValidation(msg),
			FromJob::StatementDistribution(msg) => AllMessages::StatementDistribution(msg),
		}
	}
}

// finds the first key we are capable of signing with out of the given set of validators,
// if any.
fn signing_key(validators: &[ValidatorId], keystore: &KeyStorePtr) -> Option<Arc<ValidatorPair>> {
	let keystore = keystore.read();
	validators.iter()
		.find_map(|v| {
			keystore.key_pair::<ValidatorPair>(&v).ok()
		})
		.map(|pair| Arc::new(pair))
}

impl CandidateBackingJob {
	/// Create a new `CandidateBackingJob` working on top of some parent and with a
	/// keystore holding keys.
	fn new(parent: Hash, keystore: KeyStorePtr) -> (Self, mpsc::Receiver<FromJob>) {
		let (tx_to, rx_to) = mpsc::channel(CHANNEL_CAPACITY);
		let (tx_from, rx_from) = mpsc::channel(CHANNEL_CAPACITY);

		(
			Self {
				keystore,
				parent,
				rx_to,
				tx_to,
				tx_from,

				validators: Vec::new(),
				validation_code: HashMap::default(),
				head_data: HashMap::default(),
				signing_key: Arc::new(Pair::from_seed(&[1; 32])), // TODO: No no no
				signing_context: SigningContext::default(),
				assigned_ids: HashSet::default(),
				index: 0,

				issued_validity: HashSet::default(),
				seconded: None,
				backing_watchers: HashMap::default(),
				valid_votes: HashMap::default(),
			},
			rx_from,
		)
	}

	/// Hand out the sending part of the channel to communicate with this job.
	fn tx(&mut self) -> mpsc::Sender<ToJob> {
		self.tx_to.clone()
	}

	/// Request a validator set from the `RuntimeApi`.
	async fn request_validators(&mut self) -> Result<Vec<ValidatorId>, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::Validators(tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request the scheduler roster from `RuntimeApi`.
	async fn request_validator_groups(&mut self) -> Result<SchedulerRoster, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::ValidatorGroups(tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request the validation code for some `ParaId` at given block
	/// from `RuntimeApi`.
	async fn request_validation_code(
		&mut self,
		id: ParaId,
		parent: BlockNumber,
		parablock: Option<BlockNumber>,
	) -> Result<ValidationCode, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::ValidationCode(id, parent, parablock, tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request a `SigningContext` from the `RuntimeApi`.
	async fn request_signing_context(&mut self) -> Result<SigningContext, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::SigningContext(tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request `HeadData` for some `ParaId` from `RuntimeApi`.
	async fn request_head_data(&mut self, id: ParaId) -> Result<HeadData, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::HeadData(id, tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Logic that runs on startup.
	async fn on_startup(&mut self) -> Result<(), Error> {
		let validators = self.request_validators().await?;
	
		let roster = self.request_validator_groups().await?; 

		self.signing_key = signing_key(&validators[..], &self.keystore).unwrap();
		self.index = validators
			.iter()
			.position(|x| *x == self.signing_key.as_ref().public())
			.ok_or_else(|| Error::NotInValidatorSet)? as u32;

		let mut our_groups = HashSet::<GroupIndex>::default();

		self.validators = validators;

		for (i, group) in roster.validator_groups.iter().enumerate() {
			if group.iter().find(|id| **id == self.index).is_some() {
				our_groups.insert((i as u32).into());
			}
		}

		for assignment in roster.scheduled.iter() {
			if our_groups.contains(&assignment.group_idx) {
				self.assigned_ids.insert(assignment.para_id);

				let validation_code = self.request_validation_code(assignment.para_id, 0, None).await?;
				self.validation_code.insert(assignment.para_id, validation_code);

				let head_data = self.request_head_data(assignment.para_id).await?;

				self.head_data.insert(assignment.para_id, head_data);
			}
		}

		self.signing_context = self.request_signing_context().await?;

		Ok(())
	}

	/// Run asynchronously.
	async fn run(mut self) -> Result<(), Error> {
		self.on_startup().await?;

		while let Some(msg) = self.rx_to.next().await {
			match msg {
				ToJob::CandidateBacking(msg) => {
					self.process_msg(msg).await?;
				}
				_ => break,
			}
		}

		Ok(())
	}

	/// Validate the candidate that is requested to be `Second`ed and distribute validation result.
	async fn validate_and_second(&mut self, candidate: AbridgedCandidateReceipt) -> Result<(), Error> {
		let statement = match self.spawn_validation_work(candidate.clone()).await {
			Ok(()) => {
				// make PoV available for later distribution. Send data to the availability
				// store to keep. Sign and dispatch `valid` statement to network if we
				// have not seconded the given candidate.
				Statement::Seconded(candidate)
			}

			// sign and dispatch `invalid` statement to network.
			Err(_) => Statement::Invalid(candidate.hash()),
		};

		let signed_statement = SignedFullStatement::sign(
			statement,
			&self.signing_context,
			self.index,
			&self.signing_key,
		);

		self.distribute_signed_statement(signed_statement).await?;

		Ok(())
	}

	async fn process_msg(&mut self, msg: CandidateBackingMessage) -> Result<(), Error> {
		match msg {
			CandidateBackingMessage::Second(_, candidate) => {
				// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
				// Seconded statement only if we have not seconded any other candidate and
				// have not signed a Valid statement for the requested candidate.
				match self.seconded {
					// This job has not seconded a candidate yet.
					None => {
						let candidate_hash = candidate.hash();

						if !self.issued_validity.contains(&candidate_hash) {
							if let Ok(()) = self.validate_and_second(candidate).await {
								self.seconded = Some(candidate_hash);
							}
						}
					}
					// This job has already seconded a candidate.
					Some(_) => {
					},
				}
			}
			CandidateBackingMessage::Statement(hash, statement) => {
				let idx = statement.validator_index() as usize;

				if self.validators.len() < idx {
					match statement.check_signature(&self.signing_context, &self.validators[idx]) {
						Ok(()) => {
							// TODO: check if the validator issuing this statent is a part of the group for the
							// `ParaId` in question.
							// TODO: check if the validator issuing this statement is not equivocating.
							match statement.payload() {
								Statement::Seconded(candidate) => {
									let hash = candidate.hash();
									self.valid_votes.entry(hash).or_default().push(statement.validator_index());
								}
								Statement::Valid(hash) => {
									self.valid_votes.entry(*hash).or_default().push(statement.validator_index());
								}
								Statement::Invalid(_) => {},
							}

							if let Some(_) = self.valid_votes.get(&hash) {
								// TODO: check the quorum.
								// How do we go from `ParaId` in the 
							}
						}
						Err(()) => {
						}
					}
				}
			}
			CandidateBackingMessage::RegisterBackingWatcher(hash, tx) => {
				match self.backing_watchers.get_mut(&hash) {
					Some(watchers) => watchers.push(tx),
					None => { self.backing_watchers.insert(hash, vec![tx]); }
				}
			}
		}

		Ok(())
	}

	async fn request_candidate_validation(
		&mut self,
		candidate: AbridgedCandidateReceipt,
		pov: PoVBlock,
	) -> Result<(), Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::CandidateValidation(
				CandidateValidationMessage::Validate(
					self.parent,
					candidate,
					pov,
					tx,
				)
			)
		).await?;

		rx.await??;

		Ok(())
	}

	async fn distribute_signed_statement(&mut self, s: SignedFullStatement) -> Result<(), Error> {
		let smsg = StatementDistributionMessage::Share(self.parent, s);

		self.tx_from.send(FromJob::StatementDistribution(smsg)).await?;

		Ok(())
	}

	async fn spawn_validation_work(
		&mut self,
		candidate: AbridgedCandidateReceipt,
	) -> Result<(), Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::AvailabilityStoreMessage(
				AvailabilityStoreMessage::QueryPoV(
					candidate.pov_block_hash,
					tx,
				)
			)
		).await?;

		let pov = rx.await?.unwrap();

		self.request_candidate_validation(candidate, pov).await
	}
}

struct JobHandle {
	abort_handle: future::AbortHandle,
	to_job: mpsc::Sender<ToJob>,
	finished: oneshot::Receiver<()>,
	su_handle: usize,
}

impl JobHandle {
	async fn stop(mut self) {
		let _ = self.to_job.send(ToJob::Stop).await;
		let stop_timer = Delay::new(Duration::from_secs(1));

		match future::select(stop_timer, self.finished).await {
			Either::Left((_, _)) => {
			},
			Either::Right((_, _)) => {
				self.abort_handle.abort();
			},
		}
	}

	async fn send_msg(&mut self, msg: ToJob) -> Result<(), Error> {
		Ok(self.to_job.send(msg).await?)
	}
}

struct Jobs<S> {
	spawner: S,
	running: HashMap<Hash, JobHandle>,
	outgoing_msgs: StreamUnordered<mpsc::Receiver<FromJob>>,
}

impl<S: Spawn> Jobs<S> {
	fn new(spawner: S) -> Self {
		Self {
			spawner,
			running: HashMap::default(),
			outgoing_msgs: StreamUnordered::new(),
		}
	}

	fn spawn_job(&mut self, parent_hash: Hash, keystore: KeyStorePtr) -> Result<(), ()> {
		let (mut job, from_job) = CandidateBackingJob::new(parent_hash,keystore);

		let to_job = job.tx();

		let (future, abort_handle) = future::abortable(async move {
			if let Err(_) = job.run().await {
				log::error!("Job returned an error");
			}
		});
		let (finished_tx, finished) = oneshot::channel();

		let future = async move {
			let _ = future.await;
			let _ = finished_tx.send(());
		};
		self.spawner.spawn(future).map_err(|_| ())?;

		let su_handle = self.outgoing_msgs.push(from_job);

		let handle = JobHandle {
			abort_handle,
			to_job,
			finished,
			su_handle,
		};

		self.running.insert(parent_hash, handle);

		Ok(())
	}

	async fn stop_job(&mut self, parent_hash: Hash) -> Result<(), ()> {
		match self.running.remove(&parent_hash) {
			Some(handle) => {
				Pin::new(&mut self.outgoing_msgs).remove(handle.su_handle);
				handle.stop().await;
				Ok(())
			}
			None => Err(())
		}
	}

	async fn send_msg(&mut self, parent_hash: Hash, msg: ToJob) {
		if let Some(job) = self.running.get_mut(&parent_hash) {
			let _ = job.send_msg(msg).await;
		}
	}

	async fn next(&mut self) -> Option<FromJob> {
		self.outgoing_msgs.next().await.and_then(|(e, _)| match e {
			StreamYield::Item(e) => Some(e),
			_ => None,
		})
	}
}

pub struct CandidateBackingSubsystem<S> {
	spawner: S,
	keystore: KeyStorePtr,
}

impl<S: Spawn + Clone> CandidateBackingSubsystem<S> {
	pub fn new(keystore: KeyStorePtr, spawner: S) -> Self {
		Self {
			spawner,
			keystore,
		}
	}

	async fn run(
		mut ctx: SubsystemContext<CandidateBackingMessage>,
		keystore: KeyStorePtr,
		spawner: S,
	) {
		let mut jobs = Jobs::new(spawner.clone());

		loop {
			select! {
				incoming = ctx.recv().fuse() => {
					match incoming {
						Ok(msg) => match msg {
							FromOverseer::Signal(OverseerSignal::StartWork(hash)) => {
								let _ = jobs.spawn_job(hash, keystore.clone());
							}
							FromOverseer::Signal(OverseerSignal::StopWork(hash)) => {
								let _ = jobs.stop_job(hash);
							}
							FromOverseer::Communication { msg } => {
								match msg {
									CandidateBackingMessage::Second(hash, _) => {
										let _ = jobs.send_msg(hash.clone(), ToJob::CandidateBacking(msg)).await;
									}
									_ => (),
								}
							}
							_ => (),
						},
						Err(_) => break,
					}
				}
				outgoing = jobs.next().fuse() => {
					match outgoing {
						Some(msg) => {
							let _ = ctx.send_msg(msg.into()).await;
						}
						None => break,
					}
				}
				complete => break,
			}
		}
	}
}

impl<S: Spawn + Send + Clone + 'static> Subsystem<CandidateBackingMessage> for CandidateBackingSubsystem<S> {
	fn start(&mut self, ctx: SubsystemContext<CandidateBackingMessage>) -> SpawnedSubsystem {
		let keystore = self.keystore.clone();
		let spawner = self.spawner.clone();

		SpawnedSubsystem(Box::pin(async move {
			Self::run(ctx, keystore, spawner).await;
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{channel::mpsc, executor, pin_mut, StreamExt};
	use sp_keyring::sr25519::Keyring;
	use primitives::Pair;
	use polkadot_primitives::parachain::{
		AssignmentKind, CoreAssignment, BlockData, CoreIndex,
	};

	#[test]
	fn backing_works() {
		let spawner = executor::ThreadPool::new().unwrap();
		let keystore = keystore::Store::new_in_memory();

		let alice_key = Keyring::Alice.pair();
		let alice_id: ValidatorId = alice_key.public().into();
		let bob_key = Keyring::Bob.pair();
		let bob_id: ValidatorId = bob_key.public().into();

		// Make sure `Alice` key is in the keystore, so this mocked node will be a parachain validator.
		keystore.write().insert_ephemeral_from_seed::<ValidatorPair>(&Keyring::Alice.to_seed())
			.expect("Insert key into keystore");

		executor::block_on(async move {
			let parent_hash_1 = [1; 32].into();
			let mut cbs = CandidateBackingSubsystem::new(keystore, spawner.clone());
			let (mut tx, rx) = mpsc::channel(64); 
			let (ctx, outgoing) = SubsystemContext::<CandidateBackingMessage>::new_testing(rx);

			let roster = SchedulerRoster {
				validator_groups: vec![vec![0], vec![1]],
				scheduled: vec![
					CoreAssignment {
						core: CoreIndex(0),
						para_id: 0u32.into(),
						kind: AssignmentKind::Parachain,
						group_idx: GroupIndex(0),
					}
				],
				upcoming: vec![],
				availability_cores: vec![],
			};
			let validation_code = ValidationCode(vec![1, 2, 3]);
			let head_data = HeadData(vec![4, 5, 6]);
			let signing_context = SigningContext {
				session_index: 1,
				parent_hash: parent_hash_1,
			};

			let spawned = cbs.start(ctx).0;

			spawner.spawn(spawned).unwrap();
			pin_mut!(outgoing);

			// Start work on some new parent.
			tx.send(FromOverseer::Signal(OverseerSignal::StartWork(parent_hash_1))).await.unwrap();

			// Check that subsystem job issues a request for a validator set.
			match outgoing.next().await.unwrap() {
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
				) if parent == parent_hash_1 => {
					tx.send(vec![alice_id.clone(), bob_id]).unwrap();
				}
				msg => panic!("unexpected message {:?}", msg),
			}

			// Check that subsystem job issues a request for the validator groups.
			match outgoing.next().await.unwrap() {
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
				) if parent == parent_hash_1 => {
					tx.send(roster).unwrap();
				}
				msg => panic!("unexpected message {:?}", msg),
			}

			// Check that subsystem job issues a request for the validation code.
			match outgoing.next().await.unwrap() {
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidationCode(_, _, _, tx))
				) if parent == parent_hash_1 => {
					tx.send(validation_code.clone()).unwrap();
				}
				msg => panic!("unexpected message {:?}", msg),
			}

			// Check that subsystem job issues a request for the head data.
			match outgoing.next().await.unwrap() {
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::HeadData(_, tx))
				) if parent == parent_hash_1 => {
					tx.send(head_data.clone()).unwrap();
				}
				msg => panic!("unexpected message {:?}", msg),
			}

			// Check that subsystem job issues a request for the signing context.
			match outgoing.next().await.unwrap() {
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(parent, RuntimeApiRequest::SigningContext(tx))
				) if parent == parent_hash_1 => {
					tx.send(signing_context.clone()).unwrap();
				}
				msg => panic!("unexpected message {:?}", msg),
			}

			let mut candidate = AbridgedCandidateReceipt::default();
			candidate.parachain_index = 0.into();
			candidate.relay_parent = parent_hash_1;
			candidate.pov_block_hash = [2; 32].into();

			let second = CandidateBackingMessage::Second(parent_hash_1, candidate.clone());

			tx.send(FromOverseer::Communication{ msg: second }).await.unwrap();

			let pov_block = PoVBlock {
				block_data: BlockData(vec![42, 43, 44]),
			};

			match outgoing.next().await.unwrap() {
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::QueryPoV(
						pov_hash,
						tx,
					)
				) if pov_hash == [2; 32].into() => {
					tx.send(Some(pov_block.clone())).unwrap();
				}
				msg => panic!("unexpected msg {:?}", msg),
			}

			match outgoing.next().await.unwrap() {
				AllMessages::CandidateValidation(
					CandidateValidationMessage::Validate(
						parent_hash,
						c,
						pov,
						tx,
					)
				) if parent_hash == parent_hash_1 && pov == pov_block && c == candidate => {
					tx.send(Ok(())).unwrap();
				}
				msg => panic!("unexpected msg {:?}", msg),
			}

			match outgoing.next().await.unwrap() {
				AllMessages::StatementDistribution(
					StatementDistributionMessage::Share(
						parent_hash,
						signed_statement,
					)
				) if parent_hash == parent_hash_1 => {
					signed_statement.check_signature(&signing_context, &alice_id).unwrap();
				}
				msg => panic!("unexpected message {:?}", msg),
			}

			tx.send(FromOverseer::Signal(OverseerSignal::StopWork(parent_hash_1))).await.unwrap();
		});
	}
}
