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

//! Predefined chains.

use service;

/// The chain specification (this should eventually be replaced by a more general JSON-based chain
/// specification).
#[derive(Clone, Debug)]
pub enum ChainSpec {
	/// Whatever the current polkadot runtime is, with just Alice as an auth.
	PolkadotDevelopment,
	/// Whatever the current pokadot runtime is, with simple Alice/Bob auths.
	PolkadotLocalTestnet,
	/// The Kusama network.
	Kusama,
	/// Whatever the current kusama runtime is, with just Alice as an auth.
	KusamaDevelopment,
	/// The Westend network,
	Westend,
	/// Whatever the current polkadot runtime is with the "global testnet" defaults.
	PolkadotStagingTestnet,
	/// Whatever the current kusama runtime is with the "global testnet" defaults.
	KusamaStagingTestnet,
	/// Whatever the current kusama runtime is, with simple Alice/Bob auths.
	KusamaLocalTestnet,
}

impl Default for ChainSpec {
	fn default() -> Self {
		ChainSpec::Kusama
	}
}

/// Get a chain config from a spec setting.
impl ChainSpec {
	pub(crate) fn load(self) -> Result<Box<dyn service::ChainSpec>, String> {
		Ok(match self {
			ChainSpec::PolkadotDevelopment => Box::new(service::chain_spec::polkadot_development_config()),
			ChainSpec::PolkadotLocalTestnet => Box::new(service::chain_spec::polkadot_local_testnet_config()),
			ChainSpec::PolkadotStagingTestnet => Box::new(service::chain_spec::polkadot_staging_testnet_config()),
			ChainSpec::KusamaDevelopment =>Box::new(service::chain_spec::kusama_development_config()),
			ChainSpec::KusamaLocalTestnet => Box::new(service::chain_spec::kusama_local_testnet_config()),
			ChainSpec::KusamaStagingTestnet => Box::new(service::chain_spec::kusama_staging_testnet_config()),
			ChainSpec::Westend => Box::new(service::chain_spec::westend_config()?),
			ChainSpec::Kusama => Box::new(service::chain_spec::kusama_config()?),
		})
	}

	pub(crate) fn from(s: &str) -> Option<Self> {
		match s {
			"polkadot-dev" | "dev" => Some(ChainSpec::PolkadotDevelopment),
			"polkadot-local" => Some(ChainSpec::PolkadotLocalTestnet),
			"polkadot-staging" => Some(ChainSpec::PolkadotStagingTestnet),
			"kusama-dev" => Some(ChainSpec::KusamaDevelopment),
			"kusama-local" => Some(ChainSpec::KusamaLocalTestnet),
			"kusama-staging" => Some(ChainSpec::KusamaStagingTestnet),
			"kusama" => Some(ChainSpec::Kusama),
			"westend" => Some(ChainSpec::Westend),
			"" => Some(ChainSpec::default()),
			_ => None,
		}
	}
}

/// Load the `ChainSpec` for the given `id`.
/// `force_kusama` treats chain specs coming from a file as kusama specs.
pub fn load_spec(id: &str, force_kusama: bool) -> Result<Box<dyn service::ChainSpec>, String> {
	Ok(match ChainSpec::from(id) {
		Some(spec) => spec.load()?,
		None if force_kusama => Box::new(service::KusamaChainSpec::from_json_file(std::path::PathBuf::from(id))?),
		None => Box::new(service::PolkadotChainSpec::from_json_file(std::path::PathBuf::from(id))?),
	})
}
