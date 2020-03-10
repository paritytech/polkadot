// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use crate::ChainSpec;
use log::info;
use wasm_bindgen::prelude::*;
use service::IsKusama;

/// Starts the client.
///
/// You must pass a libp2p transport that supports .
#[wasm_bindgen]
pub async fn start_client(chain_spec: String, wasm_ext: browser_utils::Transport) -> Result<browser_utils::Client, JsValue> {
	start_inner(chain_spec, wasm_ext)
		.await
		.map_err(|err| JsValue::from_str(&err.to_string()))
}

async fn start_inner(chain_spec: String, wasm_ext: browser_utils::Transport) -> Result<browser_utils::Client, Box<dyn std::error::Error>> {
	browser_utils::set_console_error_panic_hook();
	browser_utils::init_console_log(log::Level::Info)?;

	let chain_spec = ChainSpec::from(&chain_spec)
		.ok_or_else(|| format!("Chain spec: {:?} doesn't exist.", chain_spec))?
		.load()
		.map_err(|e| format!("{:?}", e))?;
	let config = browser_utils::browser_configuration(wasm_ext, chain_spec)
		.await?;

	info!("Polkadot browser node");
	info!("  version {}", config.full_version());
	info!("  by Parity Technologies, 2017-2019");
	if let Some(chain_spec) = &config.chain_spec {
		info!("Chain specification: {}", chain_spec.name());
		if chain_spec.is_kusama() {
			info!("----------------------------");
			info!("This chain is not in any way");
			info!("      endorsed by the       ");
			info!("     KUSAMA FOUNDATION      ");
			info!("----------------------------");
		}
	}
	info!("Node name: {}", config.name);
	info!("Roles: {:?}", config.roles);

	// Create the service. This is the most heavy initialization step.
	let service = service::kusama_new_light(config).map_err(|e| format!("{:?}", e))?;

	Ok(browser_utils::start_client(service))
}
