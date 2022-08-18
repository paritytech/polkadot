// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Implements the PVF pre-checking subsystem.
//!
//! This subsystem is responsible for scanning the chain for PVFs that are pending for the approval
//! as well as submitting statements regarding them passing or not the PVF pre-checking.

use futures::{channel::oneshot, future::BoxFuture, prelude::*, stream::FuturesUnordered};

use polkadot_node_subsystem::{
	messages::{CandidateValidationMessage, PreCheckOutcome, PvfCheckerMessage, RuntimeApiMessage},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
	SubsystemResult, SubsystemSender,
};
use polkadot_primitives::v2::{
	BlockNumber, Hash, PvfCheckStatement, SessionIndex, ValidationCodeHash, ValidatorId,
	ValidatorIndex,
};
use sp_keystore::SyncCryptoStorePtr;
use std::collections::HashSet;

const LOG_TARGET: &str = "parachain::pvf-checker";

mod interest_view;
mod metrics;
mod runtime_api;

#[cfg(test)]
mod tests;

use self::{
	interest_view::{InterestView, Judgement},
	metrics::Metrics,
};

/// PVF pre-checking subsystem.
pub struct PvfCheckerSubsystem {
	enabled: bool,
	keystore: SyncCryptoStorePtr,
	metrics: Metrics,
}

impl PvfCheckerSubsystem {
	pub fn new(enabled: bool, keystore: SyncCryptoStorePtr, metrics: Metrics) -> Self {
		PvfCheckerSubsystem { enabled, keystore, metrics }
	}
}

#[overseer::subsystem(PvfChecker, error=SubsystemError, prefix = self::overseer)]
impl<Context> PvfCheckerSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		if self.enabled {
			let future = run(ctx, self.keystore, self.metrics)
				.map_err(|e| SubsystemError::with_origin("pvf-checker", e))
				.boxed();

			SpawnedSubsystem { name: "pvf-checker-subsystem", future }
		} else {
			polkadot_overseer::DummySubsystem.start(ctx)
		}
	}
}

/// A struct that holds the credentials required to sign the PVF check statements. These credentials
/// are implicitly to pinned to a session where our node acts as a validator.
struct SigningCredentials {
	/// The validator public key.
	validator_key: ValidatorId,
	/// The validator index in the current session.
	validator_index: ValidatorIndex,
}

struct State {
	/// If `Some` then our node is in the active validator set during the current session.
	///
	/// Updated when a new session index is detected in one of the heads.
	credentials: Option<SigningCredentials>,

	/// The number and the hash of the most recent block that we have seen.
	///
	/// This is only updated when the PVF pre-checking API is detected in a new leaf block.
	recent_block: Option<(BlockNumber, Hash)>,

	/// The session index of the most recent session that we have seen.
	///
	/// This is only updated when the PVF pre-checking API is detected in a new leaf block.
	latest_session: Option<SessionIndex>,

	/// The set of PVF hashes that we cast a vote for within the current session.
	voted: HashSet<ValidationCodeHash>,

	/// The collection of PVFs that are observed throughout the active heads.
	view: InterestView,

	/// The container for the futures that are waiting for the outcome of the pre-checking.
	///
	/// Here are some fun facts about these futures:
	///
	/// - Pre-checking can take quite some time, in the matter of tens of seconds, so the futures here
	///   can soak for quite some time.
	/// - Pre-checking of one PVF can take drastically more time than pre-checking of another PVF.
	///   This leads to results coming out of order.
	///
	/// Resolving to `None` means that the request was dropped before replying.
	currently_checking:
		FuturesUnordered<BoxFuture<'static, Option<(PreCheckOutcome, ValidationCodeHash)>>>,
}

#[overseer::contextbounds(PvfChecker, prefix = self::overseer)]
async fn run<Context>(
	mut ctx: Context,
	keystore: SyncCryptoStorePtr,
	metrics: Metrics,
) -> SubsystemResult<()> {
	let mut state = State {
		credentials: None,
		recent_block: None,
		latest_session: None,
		voted: HashSet::with_capacity(16),
		view: InterestView::new(),
		currently_checking: FuturesUnordered::new(),
	};

	loop {
		let mut sender = ctx.sender().clone();
		futures::select! {
			precheck_response = state.currently_checking.select_next_some() => {
				if let Some((outcome, validation_code_hash)) = precheck_response {
					handle_pvf_check(
						&mut state,
						&mut sender,
						&keystore,
						&metrics,
						outcome,
						validation_code_hash,
					).await;
				} else {
					// See note in `initiate_precheck` for why this is possible and why we do not
					// care here.
				}
			}
			from_overseer = ctx.recv().fuse() => {
				let outcome = handle_from_overseer(
					&mut state,
					&mut sender,
					&keystore,
					&metrics,
					from_overseer?,
				)
				.await;
				if let Some(Conclude) = outcome {
					return Ok(());
				}
			}
		}
	}
}

/// Handle an incoming PVF pre-check result from the candidate-validation subsystem.
async fn handle_pvf_check(
	state: &mut State,
	sender: &mut impl overseer::PvfCheckerSenderTrait,
	keystore: &SyncCryptoStorePtr,
	metrics: &Metrics,
	outcome: PreCheckOutcome,
	validation_code_hash: ValidationCodeHash,
) {
	gum::debug!(
		target: LOG_TARGET,
		?validation_code_hash,
		"Received pre-check result: {:?}",
		outcome,
	);

	let judgement = match outcome {
		PreCheckOutcome::Valid => Judgement::Valid,
		PreCheckOutcome::Invalid => Judgement::Invalid,
		PreCheckOutcome::Failed => {
			// Abstain.
			//
			// Returning here will leave the PVF in the view dangling. Since it is there, no new
			// pre-checking request will be sent.
			gum::info!(
				target: LOG_TARGET,
				?validation_code_hash,
				"Pre-check failed, abstaining from voting",
			);
			return
		},
	};

	match state.view.on_judgement(validation_code_hash, judgement) {
		Ok(()) => (),
		Err(()) => {
			gum::debug!(
				target: LOG_TARGET,
				?validation_code_hash,
				"received judgement for an unknown (or removed) PVF hash",
			);
			return
		},
	}

	match (state.credentials.as_ref(), state.recent_block, state.latest_session) {
		// Note, the availability of credentials implies the availability of the recent block and
		// the session index.
		(Some(credentials), Some(recent_block), Some(session_index)) => {
			sign_and_submit_pvf_check_statement(
				sender,
				keystore,
				&mut state.voted,
				credentials,
				metrics,
				recent_block.1,
				session_index,
				judgement,
				validation_code_hash,
			)
			.await;
		},
		_ => (),
	}
}

/// A marker for the outer loop that the subsystem should stop.
struct Conclude;

async fn handle_from_overseer(
	state: &mut State,
	sender: &mut impl overseer::PvfCheckerSenderTrait,
	keystore: &SyncCryptoStorePtr,
	metrics: &Metrics,
	from_overseer: FromOrchestra<PvfCheckerMessage>,
) -> Option<Conclude> {
	match from_overseer {
		FromOrchestra::Signal(OverseerSignal::Conclude) => {
			gum::info!(target: LOG_TARGET, "Received `Conclude` signal, exiting");
			Some(Conclude)
		},
		FromOrchestra::Signal(OverseerSignal::BlockFinalized(_, _)) => {
			// ignore
			None
		},
		FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
			handle_leaves_update(state, sender, keystore, metrics, update).await;
			None
		},
		FromOrchestra::Communication { msg } => match msg {
				// uninhabited type, thus statically unreachable.
			},
	}
}

async fn handle_leaves_update(
	state: &mut State,
	sender: &mut impl overseer::PvfCheckerSenderTrait,
	keystore: &SyncCryptoStorePtr,
	metrics: &Metrics,
	update: ActiveLeavesUpdate,
) {
	if let Some(activated) = update.activated {
		let ActivationEffect { new_session_index, recent_block, pending_pvfs } =
			match examine_activation(state, sender, keystore, activated.hash, activated.number)
				.await
			{
				None => {
					// None indicates that the pre-checking runtime API is not supported.
					return
				},
				Some(e) => e,
			};

		// Note that this is not necessarily the newly activated leaf.
		let recent_block_hash = recent_block.1;
		state.recent_block = Some(recent_block);

		// Update the PVF view and get the previously unseen PVFs and start working on them.
		let outcome = state
			.view
			.on_leaves_update(Some((activated.hash, pending_pvfs)), &update.deactivated);
		metrics.on_pvf_observed(outcome.newcomers.len());
		metrics.on_pvf_left(outcome.left_num);
		for newcomer in outcome.newcomers {
			initiate_precheck(state, sender, recent_block_hash, newcomer, metrics).await;
		}

		if let Some((new_session_index, credentials)) = new_session_index {
			// New session change:
			// - update the session index
			// - reset the set of all PVFs we voted.
			// - set (or reset) the credentials.
			state.latest_session = Some(new_session_index);
			state.voted.clear();
			state.credentials = credentials;

			// If our node is a validator in the new session, we need to re-sign and submit all
			// previously obtained judgements.
			if let Some(ref credentials) = state.credentials {
				for (code_hash, judgement) in state.view.judgements() {
					sign_and_submit_pvf_check_statement(
						sender,
						keystore,
						&mut state.voted,
						credentials,
						metrics,
						recent_block_hash,
						new_session_index,
						judgement,
						code_hash,
					)
					.await;
				}
			}
		}
	} else {
		state.view.on_leaves_update(None, &update.deactivated);
	}
}

struct ActivationEffect {
	/// If the activated leaf is in a new session, the index of the new session. If the new session
	/// has a validator in the set our node happened to have private key for, the signing
	new_session_index: Option<(SessionIndex, Option<SigningCredentials>)>,
	/// This is the block hash and number of the newly activated block if it's "better" than the
	/// last one we've seen. The block is better if it's number is higher or if there are no blocks
	/// observed whatsoever. If the leaf is not better then this holds the existing recent block.
	recent_block: (BlockNumber, Hash),
	/// The full list of PVFs that are pending pre-checking according to the runtime API. In case
	/// the API returned an error this list is empty.
	pending_pvfs: Vec<ValidationCodeHash>,
}

/// Examines the new leaf and returns the effects of the examination.
///
/// Returns `None` if the PVF pre-checking runtime API is not supported for the given leaf hash.
async fn examine_activation(
	state: &mut State,
	sender: &mut impl overseer::PvfCheckerSenderTrait,
	keystore: &SyncCryptoStorePtr,
	leaf_hash: Hash,
	leaf_number: BlockNumber,
) -> Option<ActivationEffect> {
	gum::debug!(
		target: LOG_TARGET,
		"Examining activation of leaf {:?} ({})",
		leaf_hash,
		leaf_number,
	);

	let pending_pvfs = match runtime_api::pvfs_require_precheck(sender, leaf_hash).await {
		Err(runtime_api::RuntimeRequestError::NotSupported) => return None,
		Err(_) => {
			gum::debug!(
				target: LOG_TARGET,
				relay_parent = ?leaf_hash,
				"cannot fetch PVFs that require pre-checking from runtime API",
			);
			Vec::new()
		},
		Ok(v) => v,
	};

	let recent_block = match state.recent_block {
		Some((recent_block_num, recent_block_hash)) if leaf_number < recent_block_num => {
			// the existing recent block is not worse than the new activation, so leave it.
			(recent_block_num, recent_block_hash)
		},
		_ => (leaf_number, leaf_hash),
	};

	let new_session_index = match runtime_api::session_index_for_child(sender, leaf_hash).await {
		Ok(session_index) =>
			if state.latest_session.map_or(true, |l| l < session_index) {
				let signing_credentials =
					check_signing_credentials(sender, keystore, leaf_hash).await;
				Some((session_index, signing_credentials))
			} else {
				None
			},
		Err(e) => {
			gum::warn!(
				target: LOG_TARGET,
				relay_parent = ?leaf_hash,
				"cannot fetch session index from runtime API: {:?}",
				e,
			);
			None
		},
	};

	Some(ActivationEffect { new_session_index, recent_block, pending_pvfs })
}

/// Checks the active validators for the given leaf. If we have a signing key for one of them,
/// returns the [`SigningCredentials`].
async fn check_signing_credentials(
	sender: &mut impl SubsystemSender<RuntimeApiMessage>,
	keystore: &SyncCryptoStorePtr,
	leaf: Hash,
) -> Option<SigningCredentials> {
	let validators = match runtime_api::validators(sender, leaf).await {
		Ok(v) => v,
		Err(e) => {
			gum::warn!(
				target: LOG_TARGET,
				relay_parent = ?leaf,
				"error occured during requesting validators: {:?}",
				e
			);
			return None
		},
	};

	polkadot_node_subsystem_util::signing_key_and_index(&validators, keystore)
		.await
		.map(|(validator_key, validator_index)| SigningCredentials {
			validator_key,
			validator_index,
		})
}

/// Signs and submits a vote for or against a given validation code.
///
/// If the validator already voted for the given code, this function does nothing.
async fn sign_and_submit_pvf_check_statement(
	sender: &mut impl overseer::PvfCheckerSenderTrait,
	keystore: &SyncCryptoStorePtr,
	voted: &mut HashSet<ValidationCodeHash>,
	credentials: &SigningCredentials,
	metrics: &Metrics,
	relay_parent: Hash,
	session_index: SessionIndex,
	judgement: Judgement,
	validation_code_hash: ValidationCodeHash,
) {
	gum::debug!(
		target: LOG_TARGET,
		?validation_code_hash,
		?relay_parent,
		"submitting a PVF check statement for validation code = {:?}",
		judgement,
	);

	metrics.on_vote_submission_started();

	if voted.contains(&validation_code_hash) {
		gum::trace!(
			target: LOG_TARGET,
			relay_parent = ?relay_parent,
			?validation_code_hash,
			"already voted for this validation code",
		);
		metrics.on_vote_duplicate();
		return
	}

	voted.insert(validation_code_hash);

	let stmt = PvfCheckStatement {
		accept: judgement.is_valid(),
		session_index,
		subject: validation_code_hash,
		validator_index: credentials.validator_index,
	};
	let signature = match polkadot_node_subsystem_util::sign(
		keystore,
		&credentials.validator_key,
		&stmt.signing_payload(),
	)
	.await
	{
		Ok(Some(signature)) => signature,
		Ok(None) => {
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				validator_index = ?credentials.validator_index,
				?validation_code_hash,
				"private key for signing is not available",
			);
			return
		},
		Err(e) => {
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				validator_index = ?credentials.validator_index,
				?validation_code_hash,
				"error signing the statement: {:?}",
				e,
			);
			return
		},
	};

	match runtime_api::submit_pvf_check_statement(sender, relay_parent, stmt, signature).await {
		Ok(()) => {
			metrics.on_vote_submitted();
		},
		Err(e) => {
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				?validation_code_hash,
				"error occured during submitting a vote: {:?}",
				e,
			);
		},
	}
}

/// Sends a request to the candidate-validation subsystem to validate the given PVF.
///
/// The relay-parent is used as an anchor from where to fetch the PVF code. The request will be put
/// into the `currently_checking` set.
async fn initiate_precheck(
	state: &mut State,
	sender: &mut impl overseer::PvfCheckerSenderTrait,
	relay_parent: Hash,
	validation_code_hash: ValidationCodeHash,
	metrics: &Metrics,
) {
	gum::debug!(target: LOG_TARGET, ?validation_code_hash, ?relay_parent, "initiating a precheck",);

	let (tx, rx) = oneshot::channel();
	sender
		.send_message(CandidateValidationMessage::PreCheck(relay_parent, validation_code_hash, tx))
		.await;

	let timer = metrics.time_pre_check_judgement();
	state.currently_checking.push(Box::pin(async move {
		let _timer = timer;
		match rx.await {
			Ok(accept) => Some((accept, validation_code_hash)),
			Err(oneshot::Canceled) => {
				// Pre-checking request dropped before replying. That can happen in case the
				// overseer is shutting down. Our part of shutdown will be handled by the
				// overseer conclude signal. Log it here just in case.
				gum::debug!(
					target: LOG_TARGET,
					?validation_code_hash,
					?relay_parent,
					"precheck request was canceled",
				);
				None
			},
		}
	}));
}
