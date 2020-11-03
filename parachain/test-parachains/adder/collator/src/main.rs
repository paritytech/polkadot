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

use sc_cli::{Result, Role, SubstrateCli};
use polkadot_cli::Cli;
use polkadot_node_subsystem::messages::{CollatorProtocolMessage, CollationGenerationMessage};
use polkadot_node_primitives::CollationGenerationConfig;
use polkadot_primitives::v1::{CollatorPair, Id as ParaId};
use test_parachain_adder_collator::Collator;
use sp_core::{Pair, hexdisplay::HexDisplay};
use std::time::Duration;

const PARA_ID: ParaId = ParaId::new(100);

fn main() -> Result<()> {
	let cli = Cli::from_args();

	if cli.subcommand.is_some() {
		return Err("Subcommands are not supported".into())
	}

	let runner = cli.create_runner(&cli.run.base)?;

	runner.run_node_until_exit(|config| async move {
		let role = config.role.clone();

		match role {
			Role::Light => Err("Light client not supported".into()),
			_ => {
				let collator_key = CollatorPair::generate().0;

				let full_node = polkadot_service::build_full(
					config,
					polkadot_service::IsCollator::Yes(collator_key.public()),
					None,
					Some(sc_authority_discovery::WorkerConfig {
						query_interval: Duration::from_secs(1),
						query_start_delay: Duration::from_secs(0),
						..Default::default()
					}),
				)?;
				let mut overseer_handler = full_node.overseer_handler
					.expect("Overseer handler should be initialized for collators");

				let collator = Collator::new();
				let genesis_head_hex = format!("0x{:?}", HexDisplay::from(&collator.genesis_head()));
				let validation_code_hex = format!("0x{:?}", HexDisplay::from(&collator.validation_code()));

				log::info!("Running adder collator for parachain id: {}", PARA_ID);
				log::info!("Genesis state: {}", genesis_head_hex);
				log::info!("Validation code: {}", validation_code_hex);

				let config = CollationGenerationConfig {
					key: collator_key,
					collator: collator.create_collation_function(),
					para_id: PARA_ID,
				};
				overseer_handler
					.send_msg(CollationGenerationMessage::Initialize(config))
					.await
					.expect("Registers collator");

				overseer_handler
					.send_msg(CollatorProtocolMessage::CollateOn(PARA_ID))
					.await
					.expect("Collates on");

				Ok(full_node.task_manager)
			},
		}
	})
}
