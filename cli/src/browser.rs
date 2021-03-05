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

use browser_utils::{browser_configuration, init_logging_and_telemetry, set_console_error_panic_hook, Client};
use log::info;
use wasm_bindgen::prelude::*;

/// Starts the client.
#[wasm_bindgen]
pub async fn start_client(chain_spec: String, log_level: String) -> Result<Client, JsValue> {
	start_inner(chain_spec, log_level).await.map_err(|err| JsValue::from_str(&err.to_string()))
}

async fn start_inner(chain_spec: String, log_directives: String) -> Result<Client, Box<dyn std::error::Error>> {
	set_console_error_panic_hook();
	let telemetry_worker = init_logging_and_telemetry(&log_directives)?;

	let chain_spec =
		service::PolkadotChainSpec::from_json_bytes(chain_spec.as_bytes().to_vec()).map_err(|e| format!("{:?}", e))?;
	let telemetry_handle = telemetry_worker.handle();
	let config = browser_configuration(chain_spec, Some(telemetry_handle)).await?;

	info!("Polkadot browser node");
	info!("  version {}", config.impl_version);
	info!("  by Parity Technologies, 2017-2020");
	info!("📋 Chain specification: {}", config.chain_spec.name());
	info!("🏷  Node name: {}", config.network.node_name);
	info!("👤 Role: {}", config.display_role());

	// Create the service. This is the most heavy initialization step.
	let (task_manager, rpc_handlers, _) = service::build_light(config).map_err(|e| format!("{:?}", e))?;

	task_manager.spawn_handle().spawn("telemetry", telemetry_worker.run());

	Ok(browser_utils::start_client(task_manager, rpc_handlers))
}
