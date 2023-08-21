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

//! The collation generation subsystem is the interface between polkadot and the collators.
//!
//! # Protocol
//!
//! On every `ActiveLeavesUpdate`:
//!
//! * If there is no collation generation config, ignore.
//! * Otherwise, for each `activated` head in the update:
//!   * Determine if the para is scheduled on any core by fetching the `availability_cores` Runtime
//!     API.
//!   * Use the Runtime API subsystem to fetch the full validation data.
//!   * Invoke the `collator`, and use its outputs to produce a [`CandidateReceipt`], signed with
//!     the configuration's `key`.
//!   * Dispatch a [`CollatorProtocolMessage::DistributeCollation`]`(receipt, pov)`.

#![deny(missing_docs)]

use futures::{channel::oneshot, future::FutureExt, join, select};
use parity_scale_codec::Encode;
use polkadot_node_primitives::{
	AvailableData, Collation, CollationGenerationConfig, CollationSecondedSignal, PoV,
	SubmitCollationParams,
};
use polkadot_node_subsystem::{
	messages::{CollationGenerationMessage, CollatorProtocolMessage},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, RuntimeApiError, SpawnedSubsystem,
	SubsystemContext, SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	request_availability_cores, request_persisted_validation_data,
	request_staging_async_backing_params, request_validation_code, request_validation_code_hash,
	request_validators,
};
use polkadot_primitives::{
	collator_signature_payload, CandidateCommitments, CandidateDescriptor, CandidateReceipt,
	CollatorPair, CoreState, Hash, Id as ParaId, OccupiedCoreAssumption, PersistedValidationData,
	ValidationCodeHash,
};
use sp_core::crypto::Pair;
use std::sync::Arc;

mod error;

#[cfg(test)]
mod tests;

mod metrics;
use self::metrics::Metrics;

const LOG_TARGET: &'static str = "parachain::collation-generation";

/// Collation Generation Subsystem
pub struct CollationGenerationSubsystem {
	config: Option<Arc<CollationGenerationConfig>>,
	metrics: Metrics,
}

#[overseer::contextbounds(CollationGeneration, prefix = self::overseer)]
impl CollationGenerationSubsystem {
	/// Create a new instance of the `CollationGenerationSubsystem`.
	pub fn new(metrics: Metrics) -> Self {
		Self { config: None, metrics }
	}

	/// Run this subsystem
	///
	/// Conceptually, this is very simple: it just loops forever.
	///
	/// - On incoming overseer messages, it starts or stops jobs as appropriate.
	/// - On other incoming messages, if they can be converted into `Job::ToJob` and include a hash,
	///   then they're forwarded to the appropriate individual job.
	/// - On outgoing messages from the jobs, it forwards them to the overseer.
	///
	/// If `err_tx` is not `None`, errors are forwarded onto that channel as they occur.
	/// Otherwise, most are logged and then discarded.
	async fn run<Context>(mut self, mut ctx: Context) {
		loop {
			select! {
				incoming = ctx.recv().fuse() => {
					if self.handle_incoming::<Context>(incoming, &mut ctx).await {
						break;
					}
				},
			}
		}
	}

	// handle an incoming message. return true if we should break afterwards.
	// note: this doesn't strictly need to be a separate function; it's more an administrative
	// function so that we don't clutter the run loop. It could in principle be inlined directly
	// into there. it should hopefully therefore be ok that it's an async function mutably borrowing
	// self.
	async fn handle_incoming<Context>(
		&mut self,
		incoming: SubsystemResult<FromOrchestra<<Context as SubsystemContext>::Message>>,
		ctx: &mut Context,
	) -> bool {
		match incoming {
			Ok(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated,
				..
			}))) => {
				// follow the procedure from the guide
				if let Some(config) = &self.config {
					let metrics = self.metrics.clone();
					if let Err(err) = handle_new_activations(
						config.clone(),
						activated.into_iter().map(|v| v.hash),
						ctx,
						metrics,
					)
					.await
					{
						gum::warn!(target: LOG_TARGET, err = ?err, "failed to handle new activations");
					}
				}

				false
			},
			Ok(FromOrchestra::Signal(OverseerSignal::Conclude)) => true,
			Ok(FromOrchestra::Communication {
				msg: CollationGenerationMessage::Initialize(config),
			}) => {
				if self.config.is_some() {
					gum::error!(target: LOG_TARGET, "double initialization");
				} else {
					self.config = Some(Arc::new(config));
				}
				false
			},
			Ok(FromOrchestra::Communication {
				msg: CollationGenerationMessage::SubmitCollation(params),
			}) => {
				if let Some(config) = &self.config {
					if let Err(err) =
						handle_submit_collation(params, config, ctx, &self.metrics).await
					{
						gum::error!(target: LOG_TARGET, ?err, "Failed to submit collation");
					}
				} else {
					gum::error!(target: LOG_TARGET, "Collation submitted before initialization");
				}

				false
			},
			Ok(FromOrchestra::Signal(OverseerSignal::BlockFinalized(..))) => false,
			Err(err) => {
				gum::error!(
					target: LOG_TARGET,
					err = ?err,
					"error receiving message from subsystem context: {:?}",
					err
				);
				true
			},
		}
	}
}

#[overseer::subsystem(CollationGeneration, error=SubsystemError, prefix=self::overseer)]
impl<Context> CollationGenerationSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = async move {
			self.run(ctx).await;
			Ok(())
		}
		.boxed();

		SpawnedSubsystem { name: "collation-generation-subsystem", future }
	}
}

#[overseer::contextbounds(CollationGeneration, prefix = self::overseer)]
async fn handle_new_activations<Context>(
	config: Arc<CollationGenerationConfig>,
	activated: impl IntoIterator<Item = Hash>,
	ctx: &mut Context,
	metrics: Metrics,
) -> crate::error::Result<()> {
	// follow the procedure from the guide:
	// https://paritytech.github.io/polkadot/book/node/collators/collation-generation.html

	if config.collator.is_none() {
		return Ok(())
	}

	let _overall_timer = metrics.time_new_activations();

	for relay_parent in activated {
		let _relay_parent_timer = metrics.time_new_activations_relay_parent();

		let (availability_cores, validators, async_backing_params) = join!(
			request_availability_cores(relay_parent, ctx.sender()).await,
			request_validators(relay_parent, ctx.sender()).await,
			request_staging_async_backing_params(relay_parent, ctx.sender()).await,
		);

		let availability_cores = availability_cores??;
		let n_validators = validators??.len();
		let async_backing_params = async_backing_params?.ok();

		for (core_idx, core) in availability_cores.into_iter().enumerate() {
			let _availability_core_timer = metrics.time_new_activations_availability_core();

			let (scheduled_core, assumption) = match core {
				CoreState::Scheduled(scheduled_core) =>
					(scheduled_core, OccupiedCoreAssumption::Free),
				CoreState::Occupied(occupied_core) => match async_backing_params {
					Some(params) if params.max_candidate_depth >= 1 => {
						// maximum candidate depth when building on top of a block
						// pending availability is necessarily 1 - the depth of the
						// pending block is 0 so the child has depth 1.

						// TODO [now]: this assumes that next up == current.
						// in practice we should only set `OccupiedCoreAssumption::Included`
						// when the candidate occupying the core is also of the same para.
						if let Some(scheduled) = occupied_core.next_up_on_available {
							(scheduled, OccupiedCoreAssumption::Included)
						} else {
							continue
						}
					},
					_ => {
						gum::trace!(
							target: LOG_TARGET,
							core_idx = %core_idx,
							relay_parent = ?relay_parent,
							"core is occupied. Keep going.",
						);
						continue
					},
				},
				CoreState::Free => {
					gum::trace!(
						target: LOG_TARGET,
						core_idx = %core_idx,
						"core is free. Keep going.",
					);
					continue
				},
			};

			if scheduled_core.para_id != config.para_id {
				gum::trace!(
					target: LOG_TARGET,
					core_idx = %core_idx,
					relay_parent = ?relay_parent,
					our_para = %config.para_id,
					their_para = %scheduled_core.para_id,
					"core is not assigned to our para. Keep going.",
				);
				continue
			}

			// we get validation data and validation code synchronously for each core instead of
			// within the subtask loop, because we have only a single mutable handle to the
			// context, so the work can't really be distributed

			let validation_data = match request_persisted_validation_data(
				relay_parent,
				scheduled_core.para_id,
				assumption,
				ctx.sender(),
			)
			.await
			.await??
			{
				Some(v) => v,
				None => {
					gum::trace!(
						target: LOG_TARGET,
						core_idx = %core_idx,
						relay_parent = ?relay_parent,
						our_para = %config.para_id,
						their_para = %scheduled_core.para_id,
						"validation data is not available",
					);
					continue
				},
			};

			let validation_code_hash = match obtain_validation_code_hash_with_assumption(
				relay_parent,
				scheduled_core.para_id,
				assumption,
				ctx.sender(),
			)
			.await?
			{
				Some(v) => v,
				None => {
					gum::trace!(
						target: LOG_TARGET,
						core_idx = %core_idx,
						relay_parent = ?relay_parent,
						our_para = %config.para_id,
						their_para = %scheduled_core.para_id,
						"validation code hash is not found.",
					);
					continue
				},
			};

			let task_config = config.clone();
			let metrics = metrics.clone();
			let mut task_sender = ctx.sender().clone();
			ctx.spawn(
				"collation-builder",
				Box::pin(async move {
					let collator_fn = match task_config.collator.as_ref() {
						Some(x) => x,
						None => return,
					};

					let (collation, result_sender) =
						match collator_fn(relay_parent, &validation_data).await {
							Some(collation) => collation.into_inner(),
							None => {
								gum::debug!(
									target: LOG_TARGET,
									para_id = %scheduled_core.para_id,
									"collator returned no collation on collate",
								);
								return
							},
						};

					construct_and_distribute_receipt(
						PreparedCollation {
							collation,
							para_id: scheduled_core.para_id,
							relay_parent,
							validation_data,
							validation_code_hash,
							n_validators,
						},
						task_config.key.clone(),
						&mut task_sender,
						result_sender,
						&metrics,
					)
					.await;
				}),
			)?;
		}
	}

	Ok(())
}

#[overseer::contextbounds(CollationGeneration, prefix = self::overseer)]
async fn handle_submit_collation<Context>(
	params: SubmitCollationParams,
	config: &CollationGenerationConfig,
	ctx: &mut Context,
	metrics: &Metrics,
) -> crate::error::Result<()> {
	let _timer = metrics.time_submit_collation();

	let SubmitCollationParams {
		relay_parent,
		collation,
		parent_head,
		validation_code_hash,
		result_sender,
	} = params;

	let validators = request_validators(relay_parent, ctx.sender()).await.await??;
	let n_validators = validators.len();

	// We need to swap the parent-head data, but all other fields here will be correct.
	let mut validation_data = match request_persisted_validation_data(
		relay_parent,
		config.para_id,
		OccupiedCoreAssumption::TimedOut,
		ctx.sender(),
	)
	.await
	.await??
	{
		Some(v) => v,
		None => {
			gum::debug!(
				target: LOG_TARGET,
				relay_parent = ?relay_parent,
				our_para = %config.para_id,
				"No validation data for para - does it exist at this relay-parent?",
			);
			return Ok(())
		},
	};

	validation_data.parent_head = parent_head;

	let collation = PreparedCollation {
		collation,
		relay_parent,
		para_id: config.para_id,
		validation_data,
		validation_code_hash,
		n_validators,
	};

	construct_and_distribute_receipt(
		collation,
		config.key.clone(),
		ctx.sender(),
		result_sender,
		metrics,
	)
	.await;

	Ok(())
}

struct PreparedCollation {
	collation: Collation,
	para_id: ParaId,
	relay_parent: Hash,
	validation_data: PersistedValidationData,
	validation_code_hash: ValidationCodeHash,
	n_validators: usize,
}

/// Takes a prepared collation, along with its context, and produces a candidate receipt
/// which is distributed to validators.
async fn construct_and_distribute_receipt(
	collation: PreparedCollation,
	key: CollatorPair,
	sender: &mut impl overseer::CollationGenerationSenderTrait,
	result_sender: Option<oneshot::Sender<CollationSecondedSignal>>,
	metrics: &Metrics,
) {
	let PreparedCollation {
		collation,
		para_id,
		relay_parent,
		validation_data,
		validation_code_hash,
		n_validators,
	} = collation;

	let persisted_validation_data_hash = validation_data.hash();
	let parent_head_data_hash = validation_data.parent_head.hash();

	// Apply compression to the block data.
	let pov = {
		let pov = collation.proof_of_validity.into_compressed();
		let encoded_size = pov.encoded_size();

		// As long as `POV_BOMB_LIMIT` is at least `max_pov_size`, this ensures
		// that honest collators never produce a PoV which is uncompressed.
		//
		// As such, honest collators never produce an uncompressed PoV which starts with
		// a compression magic number, which would lead validators to reject the collation.
		if encoded_size > validation_data.max_pov_size as usize {
			gum::debug!(
				target: LOG_TARGET,
				para_id = %para_id,
				size = encoded_size,
				max_size = validation_data.max_pov_size,
				"PoV exceeded maximum size"
			);

			return
		}

		pov
	};

	let pov_hash = pov.hash();

	let signature_payload = collator_signature_payload(
		&relay_parent,
		&para_id,
		&persisted_validation_data_hash,
		&pov_hash,
		&validation_code_hash,
	);

	let erasure_root = match erasure_root(n_validators, validation_data, pov.clone()) {
		Ok(erasure_root) => erasure_root,
		Err(err) => {
			gum::error!(
				target: LOG_TARGET,
				para_id = %para_id,
				err = ?err,
				"failed to calculate erasure root",
			);
			return
		},
	};

	let commitments = CandidateCommitments {
		upward_messages: collation.upward_messages,
		horizontal_messages: collation.horizontal_messages,
		new_validation_code: collation.new_validation_code,
		head_data: collation.head_data,
		processed_downward_messages: collation.processed_downward_messages,
		hrmp_watermark: collation.hrmp_watermark,
	};

	let ccr = CandidateReceipt {
		commitments_hash: commitments.hash(),
		descriptor: CandidateDescriptor {
			signature: key.sign(&signature_payload),
			para_id,
			relay_parent,
			collator: key.public(),
			persisted_validation_data_hash,
			pov_hash,
			erasure_root,
			para_head: commitments.head_data.hash(),
			validation_code_hash,
		},
	};

	gum::debug!(
		target: LOG_TARGET,
		candidate_hash = ?ccr.hash(),
		?pov_hash,
		?relay_parent,
		para_id = %para_id,
		"candidate is generated",
	);
	metrics.on_collation_generated();

	sender
		.send_message(CollatorProtocolMessage::DistributeCollation(
			ccr,
			parent_head_data_hash,
			pov,
			result_sender,
		))
		.await;
}

async fn obtain_validation_code_hash_with_assumption(
	relay_parent: Hash,
	para_id: ParaId,
	assumption: OccupiedCoreAssumption,
	sender: &mut impl overseer::CollationGenerationSenderTrait,
) -> crate::error::Result<Option<ValidationCodeHash>> {
	match request_validation_code_hash(relay_parent, para_id, assumption, sender)
		.await
		.await?
	{
		Ok(Some(v)) => Ok(Some(v)),
		Ok(None) => Ok(None),
		Err(RuntimeApiError::NotSupported { .. }) => {
			match request_validation_code(relay_parent, para_id, assumption, sender).await.await? {
				Ok(Some(v)) => Ok(Some(v.hash())),
				Ok(None) => Ok(None),
				Err(e) => {
					// We assume that the `validation_code` API is always available, so any error
					// is unexpected.
					Err(e.into())
				},
			}
		},
		Err(e @ RuntimeApiError::Execution { .. }) => Err(e.into()),
	}
}

fn erasure_root(
	n_validators: usize,
	persisted_validation: PersistedValidationData,
	pov: PoV,
) -> crate::error::Result<Hash> {
	let available_data =
		AvailableData { validation_data: persisted_validation, pov: Arc::new(pov) };

	let chunks = polkadot_erasure_coding::obtain_chunks_v1(n_validators, &available_data)?;
	Ok(polkadot_erasure_coding::branches(&chunks).root())
}
