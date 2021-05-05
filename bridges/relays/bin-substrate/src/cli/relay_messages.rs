// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use crate::cli::bridge::FullBridge;
use crate::cli::{
	HexLaneId, PrometheusParams, SourceConnectionParams, SourceSigningParams, TargetConnectionParams,
	TargetSigningParams,
};
use crate::messages_lane::MessagesRelayParams;
use crate::select_full_bridge;

use structopt::StructOpt;

/// Start messages relayer process.
#[derive(StructOpt)]
pub struct RelayMessages {
	/// A bridge instance to relay messages for.
	#[structopt(possible_values = &FullBridge::variants(), case_insensitive = true)]
	bridge: FullBridge,
	/// Hex-encoded lane id that should be served by the relay. Defaults to `00000000`.
	#[structopt(long, default_value = "00000000")]
	lane: HexLaneId,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	source_sign: SourceSigningParams,
	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
	#[structopt(flatten)]
	prometheus_params: PrometheusParams,
}

impl RelayMessages {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_full_bridge!(self.bridge, {
			let source_client = self.source.to_client::<Source>().await?;
			let source_sign = self.source_sign.to_keypair::<Source>()?;
			let target_client = self.target.to_client::<Target>().await?;
			let target_sign = self.target_sign.to_keypair::<Target>()?;

			relay_messages(MessagesRelayParams {
				source_client,
				source_sign,
				target_client,
				target_sign,
				source_to_target_headers_relay: None,
				target_to_source_headers_relay: None,
				lane_id: self.lane.into(),
				metrics_params: self.prometheus_params.into(),
			})
			.await
			.map_err(|e| anyhow::format_err!("{}", e))
		})
	}
}
