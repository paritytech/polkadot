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

//! Binary used for simnet nodes, supports all runtimes, although only polkadot is implemented currently.
//! This binary accepts all the cli args the polkadot binary does, Only difference is it uses
//! manual-seal™ and babe for block authorship, it has a no-op verifier, so all blocks recieved over the network
//! are imported and executed straight away. Block authorship/Finalization maybe done by calling the `engine_createBlock` &
//! `engine_FinalizeBlock` rpc methods respectively.

use std::error::Error;

use test_runner::{Node, client_parts, ConfigOrChainSpec};
use polkadot_runtime_test::PolkadotChainInfo;
use polkadot_cli::Cli;
use structopt::StructOpt;
use sc_cli::{SubstrateCli, CliConfiguration, print_node_infos};

fn main() -> Result<(), Box<dyn Error>> {
    let cmd = <Cli as StructOpt>::from_args();
    let config = cmd.create_configuration(&cmd.run.base, task_executor).unwrap();
    let filters = cmd.run.base.log_filters().unwrap();
    let logger = sc_tracing::logging::LoggerBuilder::new(filters);
    logger.init().unwrap();
    print_node_infos::<Cli>(&config);

    let (rpc, mut tokio_runtime, task_manager, client, pool, command_sink, backend) =
        client_parts::<PolkadotChainInfo>(ConfigOrChainSpec::Config(config))
            .unwrap();
    let node = Node::<PolkadotChainInfo>::new(rpc, task_manager, client, pool, command_sink, backend);

    // wait for ctrl_c signal, then drop node.
    tokio_runtime.block_on(node.until_shutdown());
    drop(node);

    Ok(())
}
