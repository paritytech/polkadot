// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! A malicious overseer backing anything that comes in.

#![allow(missing_docs)]

use color_eyre::eyre;
use polkadot_cli::{
	create_default_subsystems,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost, ProvideRuntimeApi, SpawnNamed,
	},
	Cli,
};
use sp_keystore::SyncCryptoStorePtr;

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_candidate_validation::{CandidateValidationSubsystem, Metrics};
use polkadot_node_subsystem::messages::{CandidateValidationMessage, ValidationFailed};
use polkadot_node_subsystem_util::metrics::Metrics as _;

// Filter wrapping related types.
use malus::*;
use polkadot_node_primitives::{BlockData, PoV, ValidationResult};

use polkadot_primitives::v1::{
	CandidateCommitments, CandidateDescriptor, PersistedValidationData, ValidationCode,
	ValidationCodeHash,
};

use futures::channel::{mpsc, oneshot};
use std::{
	pin::Pin,
	sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	},
};

use structopt::StructOpt;

use shared::*;

mod shared;

#[derive(Clone, Default, Debug)]
struct BribedPassage;

impl BribedPassage {
	fn let_pass(
		&self,
		persisted_validation_data: PersistedValidationData,
		validation_code: Option<ValidationCode>,
		candidate_descriptor: CandidateDescriptor,
		pov: Arc<PoV>,
		response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	) {
		let mut candidate_commitmentments = CandidateCommitments {
			head_data: persisted_validation_data.parent_head.clone(),
			new_validation_code: Some(validation_code),
			..Default::default()
		};

		response_sender.send(Ok(ValidationResult::Valid(
			candidate_commitmentments,
			persisted_validation_data,
		)));
	}
}

impl MsgFilter for BribedPassage {
	type Message = CandidateValidationMessage;

	fn filter_in(&self, msg: FromOverseer<Self::Message>) -> Option<FromOverseer<Self::Message>> {
		match msg {
			FromOverseer::Communication {
				msg:
					CandidateValidationMessage::ValidateFromExhaustive(
						persisted_validation_data,
						validation_code,
						candidate_descriptor,
						pov,
						response_sender,
					),
			} if pov.block_data.0.as_slice() == MALICIOUS_POV => {
				self.let_pass(
					persisted_validation_data,
					validation_code,
					candidate_descriptor,
					pov,
					response_sender,
				);
				None
			},
			FromOverseer::Communication {
				msg:
					CandidateValidationMessage::ValidateFromChainState(
						candidate_descriptor,
						pov,
						response_sender,
					),
			} if pov.block_data.0.as_slice() == MALICIOUS_POV => {
				let relay_parent_number = todo!();
				let relay_parent_storage_root = todo!();
				let max_pov_size = todo!();
				let persisted_validation_data = PersistedValidationData {
					parent_head: todo!(),
					relay_parent_number,
					relay_parent_storage_root,
					max_pov_size,
				};
				self.let_pass(
					persisted_validation_data,
					None,
					candidate_descriptor,
					pov,
					response_sender,
				);
				None
			},
			msg => Some(msg),
		}
	}

	fn filter_out(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// Generates an overseer that exposes bad behavior.
struct BackGarbageCandidate;

impl OverseerGen for BackGarbageCandidate {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let spawner = args.spawner.clone();
		let leaves = args.leaves.clone();
		let runtime_client = args.runtime_client.clone();
		let registry = args.registry.clone();
		let candidate_validation_config = args.candidate_validation_config.clone();

		// modify the subsystem(s) as needed:
		let all_subsystems = create_default_subsystems(args)?.replace_candidate_validation(
			// create the filtered subsystem
			FilteredSubsystem::new(
				CandidateValidationSubsystem::with_config(
					candidate_validation_config,
					Metrics::register(registry)?,
				),
				BribedPassage::default(),
			),
		);

		let (overseer, handle) =
			Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner)
				.map_err(|e| e.into())?;

		Ok((overseer, handle))
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = Cli::from_args();
	assert_matches::assert_matches!(cli.subcommand, None);
	polkadot_cli::run_node(cli, BackGarbageCandidate)?;
	Ok(())
}
