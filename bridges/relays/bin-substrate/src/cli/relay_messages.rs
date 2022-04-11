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

use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};

use messages_relay::relay_strategy::MixStrategy;
use substrate_relay_helper::{messages_lane::MessagesRelayParams, TransactionParams};

use crate::{
	cli::{
		bridge::FullBridge, HexLaneId, PrometheusParams, SourceConnectionParams,
		SourceSigningParams, TargetConnectionParams, TargetSigningParams,
	},
	select_full_bridge,
};

/// Relayer operating mode.
#[derive(Debug, EnumString, EnumVariantNames, Clone, Copy, PartialEq)]
#[strum(serialize_all = "kebab_case")]
pub enum RelayerMode {
	/// The relayer doesn't care about rewards.
	Altruistic,
	/// The relayer will deliver all messages and confirmations as long as he's not losing any
	/// funds.
	Rational,
}

impl From<RelayerMode> for messages_relay::message_lane_loop::RelayerMode {
	fn from(mode: RelayerMode) -> Self {
		match mode {
			RelayerMode::Altruistic => Self::Altruistic,
			RelayerMode::Rational => Self::Rational,
		}
	}
}

/// Start messages relayer process.
#[derive(StructOpt)]
pub struct RelayMessages {
	/// A bridge instance to relay messages for.
	#[structopt(possible_values = FullBridge::VARIANTS, case_insensitive = true)]
	bridge: FullBridge,
	/// Hex-encoded lane id that should be served by the relay. Defaults to `00000000`.
	#[structopt(long, default_value = "00000000")]
	lane: HexLaneId,
	#[structopt(long, possible_values = RelayerMode::VARIANTS, case_insensitive = true, default_value = "rational")]
	relayer_mode: RelayerMode,
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
			let source_transactions_mortality = self.source_sign.transactions_mortality()?;
			let target_client = self.target.to_client::<Target>().await?;
			let target_sign = self.target_sign.to_keypair::<Target>()?;
			let target_transactions_mortality = self.target_sign.transactions_mortality()?;
			let relayer_mode = self.relayer_mode.into();
			let relay_strategy = MixStrategy::new(relayer_mode);

			substrate_relay_helper::messages_lane::run::<MessagesLane>(MessagesRelayParams {
				source_client,
				source_transaction_params: TransactionParams {
					signer: source_sign,
					mortality: source_transactions_mortality,
				},
				target_client,
				target_transaction_params: TransactionParams {
					signer: target_sign,
					mortality: target_transactions_mortality,
				},
				source_to_target_headers_relay: None,
				target_to_source_headers_relay: None,
				lane_id: self.lane.into(),
				metrics_params: self.prometheus_params.into(),
				standalone_metrics: None,
				relay_strategy,
			})
			.await
			.map_err(|e| anyhow::format_err!("{}", e))
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn should_use_rational_relayer_mode_by_default() {
		assert_eq!(
			RelayMessages::from_iter(vec![
				"relay-messages",
				"rialto-to-millau",
				"--source-port=0",
				"--source-signer=//Alice",
				"--target-port=0",
				"--target-signer=//Alice",
				"--lane=00000000",
			])
			.relayer_mode,
			RelayerMode::Rational,
		);
	}

	#[test]
	fn should_accept_altruistic_relayer_mode() {
		assert_eq!(
			RelayMessages::from_iter(vec![
				"relay-messages",
				"rialto-to-millau",
				"--source-port=0",
				"--source-signer=//Alice",
				"--target-port=0",
				"--target-signer=//Alice",
				"--lane=00000000",
				"--relayer-mode=altruistic",
			])
			.relayer_mode,
			RelayerMode::Altruistic,
		);
	}
}
