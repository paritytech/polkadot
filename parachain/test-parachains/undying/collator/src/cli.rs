// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Polkadot CLI library.

use clap::Parser;
use sc_cli::{RuntimeVersion, SubstrateCli};

/// Sub-commands supported by the collator.
#[derive(Debug, Parser)]
pub enum Subcommand {
	/// Export the genesis state of the parachain.
	#[command(name = "export-genesis-state")]
	ExportGenesisState(ExportGenesisStateCommand),

	/// Export the genesis wasm of the parachain.
	#[command(name = "export-genesis-wasm")]
	ExportGenesisWasm(ExportGenesisWasmCommand),
}

/// Command for exporting the genesis state of the parachain
#[derive(Debug, Parser)]
pub struct ExportGenesisStateCommand {
	/// Id of the parachain this collator collates for.
	#[arg(long, default_value_t = 100)]
	pub parachain_id: u32,

	/// The target raw PoV size in bytes. Minimum value is 64.
	#[arg(long, default_value_t = 1024)]
	pub pov_size: usize,

	/// The PVF execution complexity. Actually specifies how  many iterations/signatures
	/// we compute per block.
	#[arg(long, default_value_t = 1)]
	pub pvf_complexity: u32,
}

/// Command for exporting the genesis wasm file.
#[derive(Debug, Parser)]
pub struct ExportGenesisWasmCommand {}

#[allow(missing_docs)]
#[derive(Debug, Parser)]
#[group(skip)]
pub struct RunCmd {
	#[allow(missing_docs)]
	#[clap(flatten)]
	pub base: sc_cli::RunCmd,

	/// Id of the parachain this collator collates for.
	#[arg(long, default_value_t = 2000)]
	pub parachain_id: u32,

	/// The target raw PoV size in bytes. Minimum value is 64.
	#[arg(long, default_value_t = 1024)]
	pub pov_size: usize,

	/// The PVF execution complexity. Actually specifies how many iterations/signatures
	/// we compute per block.
	#[arg(long, default_value_t = 1)]
	pub pvf_complexity: u32,
}

#[allow(missing_docs)]
#[derive(Debug, Parser)]
pub struct Cli {
	#[command(subcommand)]
	pub subcommand: Option<Subcommand>,

	#[clap(flatten)]
	pub run: RunCmd,
}

impl SubstrateCli for Cli {
	fn impl_name() -> String {
		"Parity Zombienet/Undying".into()
	}

	fn impl_version() -> String {
		env!("CARGO_PKG_VERSION").into()
	}

	fn description() -> String {
		env!("CARGO_PKG_DESCRIPTION").into()
	}

	fn author() -> String {
		env!("CARGO_PKG_AUTHORS").into()
	}

	fn support_url() -> String {
		"https://github.com/paritytech/polkadot/issues/new".into()
	}

	fn copyright_start_year() -> i32 {
		2022
	}

	fn executable_name() -> String {
		"undying-collator".into()
	}

	fn load_spec(&self, id: &str) -> std::result::Result<Box<dyn sc_service::ChainSpec>, String> {
		let id = if id.is_empty() { "rococo" } else { id };
		Ok(match id {
			"rococo-staging" =>
				Box::new(polkadot_service::chain_spec::rococo_staging_testnet_config()?),
			"rococo-local" =>
				Box::new(polkadot_service::chain_spec::rococo_local_testnet_config()?),
			"rococo" => Box::new(polkadot_service::chain_spec::rococo_config()?),
			path => {
				let path = std::path::PathBuf::from(path);
				Box::new(polkadot_service::RococoChainSpec::from_json_file(path)?)
			},
		})
	}

	fn native_runtime_version(
		_spec: &Box<dyn polkadot_service::ChainSpec>,
	) -> &'static RuntimeVersion {
		&polkadot_service::rococo_runtime::VERSION
	}
}
