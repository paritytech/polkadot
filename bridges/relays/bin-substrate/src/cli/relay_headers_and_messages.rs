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

//! Complex headers+messages relays support.
//!
//! To add new complex relay between `ChainA` and `ChainB`, you must:
//!
//! 1) ensure that there's a `declare_chain_options!(...)` for both chains;
//! 2) add `declare_bridge_options!(...)` for the bridge;
//! 3) add bridge support to the `select_bridge! { ... }` macro.

use crate::cli::{CliChain, HexLaneId, PrometheusParams};
use crate::declare_chain_options;
use crate::messages_lane::MessagesRelayParams;
use crate::on_demand_headers::OnDemandHeadersRelay;

use futures::{FutureExt, TryFutureExt};
use relay_utils::metrics::MetricsParams;
use structopt::StructOpt;

/// Start headers+messages relayer process.
#[derive(StructOpt)]
pub enum RelayHeadersAndMessages {
	MillauRialto(MillauRialtoHeadersAndMessages),
}

/// Parameters that have the same names across all bridges.
#[derive(StructOpt)]
pub struct HeadersAndMessagesSharedParams {
	/// Hex-encoded lane id that should be served by the relay. Defaults to `00000000`.
	#[structopt(long, default_value = "00000000")]
	lane: HexLaneId,
	#[structopt(flatten)]
	prometheus_params: PrometheusParams,
}

// The reason behind this macro is that 'normal' relays are using source and target chains terminology,
// which is unusable for both-way relays (if you're relaying headers from Rialto to Millau and from
// Millau to Rialto, then which chain is source?).
macro_rules! declare_bridge_options {
	($chain1:ident, $chain2:ident) => {
		paste::item! {
			#[doc = $chain1 " and " $chain2 " headers+messages relay params."]
			#[derive(StructOpt)]
			pub struct [<$chain1 $chain2 HeadersAndMessages>] {
				#[structopt(flatten)]
				shared: HeadersAndMessagesSharedParams,
				#[structopt(flatten)]
				left: [<$chain1 ConnectionParams>],
				#[structopt(flatten)]
				left_sign: [<$chain1 SigningParams>],
				#[structopt(flatten)]
				right: [<$chain2 ConnectionParams>],
				#[structopt(flatten)]
				right_sign: [<$chain2 SigningParams>],
			}

			#[allow(unreachable_patterns)]
			impl From<RelayHeadersAndMessages> for [<$chain1 $chain2 HeadersAndMessages>] {
				fn from(relay_params: RelayHeadersAndMessages) -> [<$chain1 $chain2 HeadersAndMessages>] {
					match relay_params {
						RelayHeadersAndMessages::[<$chain1 $chain2>](params) => params,
						_ => unreachable!(),
					}
				}
			}
		}
	};
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			RelayHeadersAndMessages::MillauRialto(_) => {
				type Params = MillauRialtoHeadersAndMessages;

				type Left = relay_millau_client::Millau;
				type Right = relay_rialto_client::Rialto;

				type LeftToRightFinality = crate::chains::millau_headers_to_rialto::MillauFinalityToRialto;
				type RightToLeftFinality = crate::chains::rialto_headers_to_millau::RialtoFinalityToMillau;

				type LeftToRightMessages = crate::chains::millau_messages_to_rialto::MillauMessagesToRialto;
				type RightToLeftMessages = crate::chains::rialto_messages_to_millau::RialtoMessagesToMillau;

				use crate::chains::millau_messages_to_rialto::run as left_to_right_messages;
				use crate::chains::rialto_messages_to_millau::run as right_to_left_messages;

				$generic
			}
		}
	};
}

// All supported chains.
declare_chain_options!(Millau, millau);
declare_chain_options!(Rialto, rialto);
// All supported bridges.
declare_bridge_options!(Millau, Rialto);

impl RelayHeadersAndMessages {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self, {
			let params: Params = self.into();

			let left_client = params.left.to_client::<Left>().await?;
			let left_sign = params.left_sign.to_keypair::<Left>()?;
			let right_client = params.right.to_client::<Right>().await?;
			let right_sign = params.right_sign.to_keypair::<Right>()?;

			let lane = params.shared.lane.into();

			let metrics_params: MetricsParams = params.shared.prometheus_params.into();
			let metrics_params = relay_utils::relay_metrics(None, metrics_params).into_params();

			let left_to_right_on_demand_headers = OnDemandHeadersRelay::new(
				left_client.clone(),
				right_client.clone(),
				LeftToRightFinality::new(right_client.clone(), right_sign.clone()),
			);
			let right_to_left_on_demand_headers = OnDemandHeadersRelay::new(
				right_client.clone(),
				left_client.clone(),
				RightToLeftFinality::new(left_client.clone(), left_sign.clone()),
			);

			let left_to_right_messages = left_to_right_messages(MessagesRelayParams {
				source_client: left_client.clone(),
				source_sign: left_sign.clone(),
				target_client: right_client.clone(),
				target_sign: right_sign.clone(),
				source_to_target_headers_relay: Some(left_to_right_on_demand_headers.clone()),
				target_to_source_headers_relay: Some(right_to_left_on_demand_headers.clone()),
				lane_id: lane,
				metrics_params: metrics_params
					.clone()
					.disable()
					.metrics_prefix(messages_relay::message_lane_loop::metrics_prefix::<LeftToRightMessages>(&lane)),
			})
			.map_err(|e| anyhow::format_err!("{}", e))
			.boxed();
			let right_to_left_messages = right_to_left_messages(MessagesRelayParams {
				source_client: right_client,
				source_sign: right_sign,
				target_client: left_client.clone(),
				target_sign: left_sign.clone(),
				source_to_target_headers_relay: Some(right_to_left_on_demand_headers),
				target_to_source_headers_relay: Some(left_to_right_on_demand_headers),
				lane_id: lane,
				metrics_params: metrics_params
					.clone()
					.disable()
					.metrics_prefix(messages_relay::message_lane_loop::metrics_prefix::<RightToLeftMessages>(&lane)),
			})
			.map_err(|e| anyhow::format_err!("{}", e))
			.boxed();

			relay_utils::relay_metrics(None, metrics_params)
				.expose()
				.await
				.map_err(|e| anyhow::format_err!("{}", e))?;

			futures::future::select(left_to_right_messages, right_to_left_messages)
				.await
				.factor_first()
				.0
		})
	}
}
