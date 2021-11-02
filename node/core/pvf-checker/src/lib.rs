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
	messages::{CandidateValidationMessage, PvfCheckerMessage},
	overseer, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext,
	SubsystemError, SubsystemResult, SubsystemSender,
};
use polkadot_primitives::v1::{
	BlockNumber, Hash, PvfCheckStatement, SessionIndex, ValidationCodeHash, ValidatorId,
	ValidatorIndex,
};
use sp_keystore::SyncCryptoStorePtr;
use std::collections::HashSet;

const LOG_TARGET: &str = "parachain::pvf-checker";

mod interest_view;
mod runtime_api;

use self::interest_view::InterestView;

pub struct PvfCheckerSubsystem {
	keystore: SyncCryptoStorePtr,
}

impl PvfCheckerSubsystem {
	pub fn new(keystore: SyncCryptoStorePtr) -> Self {
		PvfCheckerSubsystem { keystore }
	}
}

impl<Context> overseer::Subsystem<Context, SubsystemError> for PvfCheckerSubsystem
where
	Context: SubsystemContext<Message = PvfCheckerMessage>,
	Context: overseer::SubsystemContext<Message = PvfCheckerMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run(ctx, self.keystore)
			.map_err(|e| SubsystemError::with_origin("pvf-checker", e))
			.boxed();

		SpawnedSubsystem { name: "pvf-checker-subsystem", future }
	}
}

struct SigningPower {
	validator_key: ValidatorId,
	validator_index: ValidatorIndex,
}

struct State {
	signing: Option<SigningPower>,
	recent_block: Option<(BlockNumber, Hash)>,
	latest_session: Option<SessionIndex>,
	voted: HashSet<ValidationCodeHash>,
	view: InterestView,
	currently_checking: FuturesUnordered<BoxFuture<'static, Option<(bool, ValidationCodeHash)>>>,
}

async fn run<Context>(mut ctx: Context, keystore: SyncCryptoStorePtr) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = PvfCheckerMessage>,
	Context: overseer::SubsystemContext<Message = PvfCheckerMessage>,
{
	let mut state = State {
		signing: None,
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
				if let Some((accept, validation_code_hash)) = precheck_response {
					handle_pvf_check(
						&mut state,
						&mut sender,
						&keystore,
						accept,
						validation_code_hash,
					).await;
				}
			}
			from_overseer = ctx.recv().fuse() => {
				handle_from_overseer(
					&mut state,
					&mut sender,
					&keystore,
					from_overseer?,
				).await;
			}
		}
	}
}

async fn handle_pvf_check(
	state: &mut State,
	sender: &mut impl SubsystemSender,
	keystore: &SyncCryptoStorePtr,
	accept: bool,
	validation_code_hash: ValidationCodeHash,
) {
	match state.view.on_judgement(validation_code_hash, accept) {
		Ok(()) => (),
		Err(()) => {
			tracing::debug!(
				target: LOG_TARGET,
				?validation_code_hash,
				"received judgement for an unknown (or removed) PVF hash",
			);
			return
		},
	}

	if let Some(ref signing) = state.signing {
		if let Some(recent_block) = state.recent_block {
			if let Some(session_index) = state.latest_session {
				cast_vote(
					sender,
					keystore,
					&mut state.voted,
					signing,
					recent_block.1,
					session_index,
					accept,
					validation_code_hash,
				)
				.await;
			}
		}
	}
}

async fn handle_from_overseer(
	state: &mut State,
	sender: &mut impl SubsystemSender,
	keystore: &SyncCryptoStorePtr,
	from_overseer: FromOverseer<PvfCheckerMessage>,
) {
	match from_overseer {
		FromOverseer::Signal(OverseerSignal::Conclude) => {
			tracing::info!(target: LOG_TARGET, "Received `Conclude` signal, exiting");
			return // TODO:
		},
		FromOverseer::Signal(OverseerSignal::BlockFinalized(_, _)) => {},
		FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
			handle_leaves_update(state, sender, keystore, update).await;
		},
		FromOverseer::Communication { msg } => match msg {
				// uninhabited type, thus statically unreachable.
			},
	}
}

async fn handle_leaves_update(
	state: &mut State,
	sender: &mut impl SubsystemSender,
	keystore: &SyncCryptoStorePtr,
	update: ActiveLeavesUpdate,
) {
	if let Some(activated) = update.activated {
		let ActivationEffect { new_session_index, recent_block, pending_pvfs } =
			examine_activation(state, sender, keystore, activated.hash, activated.number).await;
		state.recent_block = Some(recent_block);

		// Update the PVF view and get the previously unseen PVFs and start working on them.
		let newcomers = state
			.view
			.on_leaves_update(Some((activated.hash, pending_pvfs)), &update.deactivated);
		for newcomer in newcomers {
			initiate_precheck(state, sender, recent_block.1, newcomer).await;
		}

		if let Some((new_session_index, signing)) = new_session_index {
			state.latest_session = Some(new_session_index);
			state.voted.clear();
			state.signing = signing;

			if let Some(ref signing) = state.signing {
				for (code_hash, accept) in state.view.judgements() {
					cast_vote(
						sender,
						keystore,
						&mut state.voted,
						signing,
						recent_block.1,
						new_session_index,
						accept,
						code_hash.clone(),
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
	new_session_index: Option<(SessionIndex, Option<SigningPower>)>,
	recent_block: (BlockNumber, Hash),
	pending_pvfs: Vec<ValidationCodeHash>,
}

async fn examine_activation(
	state: &mut State,
	sender: &mut impl SubsystemSender,
	keystore: &SyncCryptoStorePtr,
	leaf_hash: Hash,
	leaf_number: BlockNumber,
) -> ActivationEffect {
	let recent_block = match state.recent_block {
		Some((recent_block_num, recent_block_hash)) if leaf_number < recent_block_num => {
			// the existing recent block is not worse than the new activation, so leave it.
			(recent_block_num, recent_block_hash)
		},
		_ => (leaf_number, leaf_hash),
	};

	let mut new_session_index = None;
	if let Ok(session_index) = runtime_api::session_index_for_child(sender, leaf_hash).await {
		if state.latest_session.map_or(true, |l| l < session_index) {
			let signing_power = is_in_validator_set(sender, keystore, leaf_hash).await;

			new_session_index = Some((session_index, signing_power));
		}
	} // TODO: log on error

	let pending_pvfs = match runtime_api::pvfs_require_precheck(sender, leaf_hash).await {
		Ok(v) => v,
		Err(_) => {
			tracing::debug!(
				target: LOG_TARGET,
				relay_parent = ?leaf_hash,
				"cannot fetch PVFs that require pre-checking from runtime API",
			);
			Vec::new()
		},
	};

	ActivationEffect { new_session_index, recent_block, pending_pvfs }
}

async fn is_in_validator_set(
	sender: &mut impl SubsystemSender,
	keystore: &SyncCryptoStorePtr,
	relay_parent: Hash,
) -> Option<SigningPower> {
	let validators = match runtime_api::validators(sender, relay_parent).await {
		Ok(v) => v,
		Err(e) => {
			tracing::warn!(
				target: LOG_TARGET,
				?relay_parent,
				"error occured during requesting validators",
			);
			return None
		},
	};

	polkadot_node_subsystem_util::signing_key_and_index(&validators, keystore)
		.await
		.map(|(validator_key, validator_index)| SigningPower { validator_key, validator_index })
}

async fn cast_vote(
	sender: &mut impl SubsystemSender,
	keystore: &SyncCryptoStorePtr,
	voted: &mut HashSet<ValidationCodeHash>,
	signing: &SigningPower,
	relay_parent: Hash,
	session_index: SessionIndex,
	accept: bool,
	validation_code_hash: ValidationCodeHash,
) {
	if voted.contains(&validation_code_hash) {
		return
	}

	voted.insert(validation_code_hash);

	let stmt = PvfCheckStatement {
		accept,
		session_index,
		subject: validation_code_hash,
		validator_index: signing.validator_index,
	};
	match polkadot_node_subsystem_util::sign(
		keystore,
		&signing.validator_key,
		&stmt.signing_payload(),
	)
	.await
	{
		Ok(Some(signature)) => {
			runtime_api::submit_pvf_check_statement(sender, relay_parent, stmt, signature).await;
		},
		Ok(None) => {
			// TODO: signing key is not found?
		},
		Err(e) => {
			// TODO: error during signing?
		},
	}
}

async fn initiate_precheck(
	state: &mut State,
	sender: &mut impl SubsystemSender,
	relay_parent: Hash,
	validation_code_hash: ValidationCodeHash,
) {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(
			CandidateValidationMessage::PreCheck(relay_parent, validation_code_hash, tx).into(),
		)
		.await;
	state
		.currently_checking
		.push(Box::pin(async move { rx.await.ok().map(|accept| (accept, validation_code_hash)) }));
}
