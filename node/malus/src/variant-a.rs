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

//! A malicious overseer.
//!
//! An example on how to use the `OverseerGen` pattern to
//! instantiate a modified subsystem implementation
//! for usage with `simnet`/Gurke.

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

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_candidate_validation::{CandidateValidationSubsystem, Metrics};
use polkadot_node_subsystem::messages::CandidateValidationMessage;
use polkadot_node_subsystem_util::metrics::Metrics as _;

// Filter wrapping related types.
use malus::*;

use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use structopt::StructOpt;

/// Silly example, just drop every second outgoing message.
#[derive(Clone, Default, Debug)]
struct Skippy(Arc<AtomicUsize>);

impl MsgFilter for Skippy {
	type Message = CandidateValidationMessage;

	fn filter_in(&self, msg: FromOverseer<Self::Message>) -> Option<FromOverseer<Self::Message>> {
		if self.0.fetch_add(1, Ordering::Relaxed) % 2 == 0 {
			Some(msg)
		} else {
			None
		}
	}
	fn filter_out(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// Generates an overseer that exposes bad behavior.
struct BehaveMaleficient;

impl OverseerGen for BehaveMaleficient {
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
					polkadot_node_core_pvf::Metrics::register(registry)?,
				),
				Skippy::default(),
			),
		);

		Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner)
			.map_err(|e| e.into())
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = Cli::from_args();
	assert_matches::assert_matches!(cli.subcommand, None);
	polkadot_cli::run_node(cli, BehaveMaleficient)?;
	Ok(())
}
