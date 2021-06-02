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
use log::LevelFilter;

mod polkadot_chain_info;

use polkadot_chain_info::PolkadotSimnetChainInfo;
use test_runner::{NodeConfig, Node};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = NodeConfig {
        log_targets: vec![
            ("yamux", LevelFilter::Off),
            ("multistream_select", LevelFilter::Off),
            ("libp2p", LevelFilter::Off),
            ("jsonrpc_client_transports", LevelFilter::Off),
            ("sc_network", LevelFilter::Off),
            ("tokio_reactor", LevelFilter::Off),
            ("parity-db", LevelFilter::Off),
            ("sub-libp2p", LevelFilter::Off),
            ("peerset", LevelFilter::Off),
            ("ws", LevelFilter::Off),
            ("sc_service", LevelFilter::Off),
            ("sc_basic_authorship", LevelFilter::Off),
            ("telemetry-logger", LevelFilter::Off),
            ("sc_peerset", LevelFilter::Off),
            ("rpc", LevelFilter::Off),

            ("sync", LevelFilter::Debug),
            ("sc_network", LevelFilter::Debug),
            ("runtime", LevelFilter::Trace),
            ("babe", LevelFilter::Debug)
        ],
    };
    let node = Node::<PolkadotSimnetChainInfo>::new(config)?;

    // wait for ctrl_c signal, then drop node.
    tokio::signal::ctrl_c().await?;

    drop(node);

    Ok(())
}
