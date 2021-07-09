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
//! Attempts to upgrade the polkadot runtime, in a simnet environment
use std::{error::Error, str::FromStr};

use test_runner::{Node, client_parts, ConfigOrChainSpec, task_executor};
use polkadot_runtime_test::{PolkadotChainInfo, dispatch_with_root};
use sc_cli::{build_runtime, SubstrateCli, CliConfiguration, print_node_infos};
use polkadot_cli::Cli;
use structopt::StructOpt;
use sp_core::crypto::AccountId32;

fn main() -> Result<(), Box<dyn Error>> {
    let mut tokio_runtime = build_runtime()?;
    let task_executor = task_executor(tokio_runtime.handle().clone());
    // parse cli args
    let cmd = <Cli as StructOpt>::from_args();
    // set up logging
    let filters = cmd.run.base.log_filters()?;
    let logger = sc_tracing::logging::LoggerBuilder::new(filters);
    logger.init()?;

    // set up the test-runner
    let config = cmd.create_configuration(&cmd.run.base, task_executor)?;
    print_node_infos::<Cli>(&config);
    let (rpc, task_manager, client, pool, command_sink, backend) =
        client_parts::<PolkadotChainInfo>(ConfigOrChainSpec::Config(config))?;
    let node = Node::<PolkadotChainInfo>::new(rpc, task_manager, client, pool, command_sink, backend);

    let wasm_binary = polkadot_runtime::WASM_BINARY
        .ok_or("Polkadot development wasm not available")?
        .to_vec();

    tokio_runtime.block_on(async {
        // start runtime upgrade
        dispatch_with_root(system::Call::set_code(wasm_binary).into(), &node).await?;

        let (from, dest, balance) = (
            AccountId32::from_str("15j4dg5GzsL1bw2U2AWgeyAk6QTxq43V7ZPbXdAmbVLjvDCK")?,
            AccountId32::from_str("1rvXMZpAj9nKLQkPFCymyH7Fg3ZyKJhJbrc7UtHbTVhJm1A")?,
            10_000_000_000_000 // 10 dots
        );

        // post upgrade tests, a simple balance transfer
        node.submit_extrinsic(balances::Call::transfer(dest.into(), balance), from).await?;

        Ok::<_, Box<dyn Error>>(())
    })?;

    // done upgrading runtime, drop node.
    drop(node);

    Ok(())
}
