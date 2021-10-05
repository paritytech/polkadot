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
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, OverseerGen,
		OverseerGenArgs, ParachainHost, ProvideRuntimeApi, SpawnNamed,
	},
	Cli,
};

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_candidate_validation::CandidateValidationSubsystem;
use polkadot_node_subsystem::{
	messages::{AllMessages, CandidateValidationMessage},
	overseer::{self, Overseer, OverseerConnector, OverseerHandle},
	FromOverseer,
};

use malus::*;

// Filter wrapping related types.
use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use structopt::StructOpt;

/// Silly example, just drop every second outgoing message.
#[derive(Clone, Default, Debug)]
struct Skippy(Arc<AtomicUsize>);

impl<Sender> MessageInterceptor<Sender> for Skippy
where
	Sender: overseer::SubsystemSender<AllMessages>
		+ overseer::SubsystemSender<CandidateValidationMessage>
		+ Clone
		+ 'static,
{
	type Message = CandidateValidationMessage;

	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		if self.0.fetch_add(1, Ordering::Relaxed) % 2 == 0 {
			Some(msg)
		} else {
			None
		}
	}
	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// Generates an overseer that exposes bad behavior.
struct BehaveMaleficient;

impl OverseerGen for BehaveMaleficient {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let candidate_validation_config = args.candidate_validation_config.clone();

		prepared_overseer_builder(args)?
			.replace_candidate_validation(|orig: CandidateValidationSubsystem| {
				InterceptedSubsystem::new(
					CandidateValidationSubsystem::with_config(
						candidate_validation_config,
						orig.metrics,
						orig.pvf_metrics,
					),
					Skippy::default(),
				)
			})
			.build_with_connector(connector)
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
