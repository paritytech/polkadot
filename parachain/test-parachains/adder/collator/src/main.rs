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

//! Collator for the adder test parachain.

use polkadot_cli::{Error, Result};
use polkadot_node_primitives::CollationGenerationConfig;
use polkadot_node_subsystem::messages::{CollationGenerationMessage, CollatorProtocolMessage};
use polkadot_primitives::v2::Id as ParaId;
use sc_cli::{Error as SubstrateCliError, SubstrateCli};
use sp_core::hexdisplay::HexDisplay;
use test_parachain_adder_collator::Collator;

/// The parachain ID to collate for in case it wasn't set explicitly through CLI.
const DEFAULT_PARA_ID: ParaId = ParaId::new(100);

mod cli;
use cli::Cli;

fn main() -> Result<()> {
	let cli = Cli::from_args();

	match cli.subcommand {
		Some(cli::Subcommand::ExportGenesisState(_params)) => {
			let collator = Collator::new();
			println!("0x{:?}", HexDisplay::from(&collator.genesis_head()));

			Ok::<_, Error>(())
		},
		Some(cli::Subcommand::ExportGenesisWasm(_params)) => {
			let collator = Collator::new();
			println!("0x{:?}", HexDisplay::from(&collator.validation_code()));

			Ok(())
		},
		None => {
			let runner = cli.create_runner(&cli.run.base).map_err(|e| {
				SubstrateCliError::Application(
					Box::new(e) as Box<(dyn 'static + Send + Sync + std::error::Error)>
				)
			})?;

			runner.run_node_until_exit(|config| async move {
				let collator = Collator::new();

				let full_node = polkadot_service::build_full(
					config,
					polkadot_service::IsCollator::Yes(collator.collator_key()),
					None,
					false,
					None,
					None,
					false,
					polkadot_service::RealOverseerGen,
					None,
					None,
					None,
				)
				.map_err(|e| e.to_string())?;
				let mut overseer_handle = full_node
					.overseer_handle
					.expect("Overseer handle should be initialized for collators");

				let genesis_head_hex =
					format!("0x{:?}", HexDisplay::from(&collator.genesis_head()));
				let validation_code_hex =
					format!("0x{:?}", HexDisplay::from(&collator.validation_code()));

				let para_id = cli.run.parachain_id.map(ParaId::from).unwrap_or(DEFAULT_PARA_ID);

				log::info!("Running adder collator for parachain id: {}", para_id);
				log::info!("Genesis state: {}", genesis_head_hex);
				log::info!("Validation code: {}", validation_code_hex);

				let config = CollationGenerationConfig {
					key: collator.collator_key(),
					collator: collator
						.create_collation_function(full_node.task_manager.spawn_handle()),
					para_id,
				};
				overseer_handle
					.send_msg(CollationGenerationMessage::Initialize(config), "Collator")
					.await;

				overseer_handle
					.send_msg(CollatorProtocolMessage::CollateOn(para_id), "Collator")
					.await;

				Ok(full_node.task_manager)
			})
		},
	}?;
	Ok(())
}
