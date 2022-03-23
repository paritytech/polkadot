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

use futures::{FutureExt, TryFutureExt};
use structopt::StructOpt;
use strum::VariantNames;

use codec::Encode;
use messages_relay::relay_strategy::MixStrategy;
use relay_substrate_client::{
	AccountIdOf, CallOf, Chain, ChainRuntimeVersion, Client, SignParam, TransactionSignScheme,
	UnsignedTransaction,
};
use relay_utils::metrics::MetricsParams;
use sp_core::{Bytes, Pair};
use substrate_relay_helper::{
	finality_pipeline::SubstrateFinalitySyncPipeline, messages_lane::MessagesRelayParams,
	on_demand_headers::OnDemandHeadersRelay, TransactionParams,
};

use crate::{
	cli::{relay_messages::RelayerMode, CliChain, HexLaneId, PrometheusParams, RuntimeVersionType},
	declare_chain_options,
};

/// Maximal allowed conversion rate error ratio (abs(real - stored) / stored) that we allow.
///
/// If it is zero, then transaction will be submitted every time we see difference between
/// stored and real conversion rates. If it is large enough (e.g. > than 10 percents, which is 0.1),
/// then rational relayers may stop relaying messages because they were submitted using
/// lesser conversion rate.
pub(crate) const CONVERSION_RATE_ALLOWED_DIFFERENCE_RATIO: f64 = 0.05;

/// Start headers+messages relayer process.
#[derive(StructOpt)]
pub enum RelayHeadersAndMessages {
	MillauRialto(MillauRialtoHeadersAndMessages),
	RococoWococo(RococoWococoHeadersAndMessages),
	KusamaPolkadot(KusamaPolkadotHeadersAndMessages),
}

/// Parameters that have the same names across all bridges.
#[derive(StructOpt)]
pub struct HeadersAndMessagesSharedParams {
	/// Hex-encoded lane identifiers that should be served by the complex relay.
	#[structopt(long, default_value = "00000000")]
	lane: Vec<HexLaneId>,
	#[structopt(long, possible_values = RelayerMode::VARIANTS, case_insensitive = true, default_value = "rational")]
	relayer_mode: RelayerMode,
	/// Create relayers fund accounts on both chains, if it does not exists yet.
	#[structopt(long)]
	create_relayers_fund_accounts: bool,
	/// If passed, only mandatory headers (headers that are changing the GRANDPA authorities set)
	/// are relayed.
	#[structopt(long)]
	only_mandatory_headers: bool,
	#[structopt(flatten)]
	prometheus_params: PrometheusParams,
}

// The reason behind this macro is that 'normal' relays are using source and target chains
// terminology, which is unusable for both-way relays (if you're relaying headers from Rialto to
// Millau and from Millau to Rialto, then which chain is source?).
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
				left_messages_pallet_owner: [<$chain1 MessagesPalletOwnerSigningParams>],
				#[structopt(flatten)]
				right: [<$chain2 ConnectionParams>],
				#[structopt(flatten)]
				right_sign: [<$chain2 SigningParams>],
				#[structopt(flatten)]
				right_messages_pallet_owner: [<$chain2 MessagesPalletOwnerSigningParams>],
			}

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

				type LeftToRightFinality =
					crate::chains::millau_headers_to_rialto::MillauFinalityToRialto;
				type RightToLeftFinality =
					crate::chains::rialto_headers_to_millau::RialtoFinalityToMillau;

				type LeftAccountIdConverter = bp_millau::AccountIdConverter;
				type RightAccountIdConverter = bp_rialto::AccountIdConverter;

				use crate::chains::{
					millau_messages_to_rialto::MillauMessagesToRialto as LeftToRightMessageLane,
					rialto_messages_to_millau::RialtoMessagesToMillau as RightToLeftMessageLane,
				};

				async fn left_create_account(
					_left_client: Client<Left>,
					_left_sign: <Left as TransactionSignScheme>::AccountKeyPair,
					_account_id: AccountIdOf<Left>,
				) -> anyhow::Result<()> {
					Err(anyhow::format_err!("Account creation is not supported by this bridge"))
				}

				async fn right_create_account(
					_right_client: Client<Right>,
					_right_sign: <Right as TransactionSignScheme>::AccountKeyPair,
					_account_id: AccountIdOf<Right>,
				) -> anyhow::Result<()> {
					Err(anyhow::format_err!("Account creation is not supported by this bridge"))
				}

				$generic
			},
			RelayHeadersAndMessages::RococoWococo(_) => {
				type Params = RococoWococoHeadersAndMessages;

				type Left = relay_rococo_client::Rococo;
				type Right = relay_wococo_client::Wococo;

				type LeftToRightFinality =
					crate::chains::rococo_headers_to_wococo::RococoFinalityToWococo;
				type RightToLeftFinality =
					crate::chains::wococo_headers_to_rococo::WococoFinalityToRococo;

				type LeftAccountIdConverter = bp_rococo::AccountIdConverter;
				type RightAccountIdConverter = bp_wococo::AccountIdConverter;

				use crate::chains::{
					rococo_messages_to_wococo::RococoMessagesToWococo as LeftToRightMessageLane,
					wococo_messages_to_rococo::WococoMessagesToRococo as RightToLeftMessageLane,
				};

				async fn left_create_account(
					left_client: Client<Left>,
					left_sign: <Left as TransactionSignScheme>::AccountKeyPair,
					account_id: AccountIdOf<Left>,
				) -> anyhow::Result<()> {
					submit_signed_extrinsic(
						left_client,
						left_sign,
						relay_rococo_client::runtime::Call::Balances(
							relay_rococo_client::runtime::BalancesCall::transfer(
								bp_rococo::AccountAddress::Id(account_id),
								bp_rococo::EXISTENTIAL_DEPOSIT.into(),
							),
						),
					)
					.await
				}

				async fn right_create_account(
					right_client: Client<Right>,
					right_sign: <Right as TransactionSignScheme>::AccountKeyPair,
					account_id: AccountIdOf<Right>,
				) -> anyhow::Result<()> {
					submit_signed_extrinsic(
						right_client,
						right_sign,
						relay_wococo_client::runtime::Call::Balances(
							relay_wococo_client::runtime::BalancesCall::transfer(
								bp_wococo::AccountAddress::Id(account_id),
								bp_wococo::EXISTENTIAL_DEPOSIT.into(),
							),
						),
					)
					.await
				}

				$generic
			},
			RelayHeadersAndMessages::KusamaPolkadot(_) => {
				type Params = KusamaPolkadotHeadersAndMessages;

				type Left = relay_kusama_client::Kusama;
				type Right = relay_polkadot_client::Polkadot;

				type LeftToRightFinality =
					crate::chains::kusama_headers_to_polkadot::KusamaFinalityToPolkadot;
				type RightToLeftFinality =
					crate::chains::polkadot_headers_to_kusama::PolkadotFinalityToKusama;

				type LeftAccountIdConverter = bp_kusama::AccountIdConverter;
				type RightAccountIdConverter = bp_polkadot::AccountIdConverter;

				use crate::chains::{
					kusama_messages_to_polkadot::KusamaMessagesToPolkadot as LeftToRightMessageLane,
					polkadot_messages_to_kusama::PolkadotMessagesToKusama as RightToLeftMessageLane,
				};

				async fn left_create_account(
					left_client: Client<Left>,
					left_sign: <Left as TransactionSignScheme>::AccountKeyPair,
					account_id: AccountIdOf<Left>,
				) -> anyhow::Result<()> {
					submit_signed_extrinsic(
						left_client,
						left_sign,
						relay_kusama_client::runtime::Call::Balances(
							relay_kusama_client::runtime::BalancesCall::transfer(
								bp_kusama::AccountAddress::Id(account_id),
								bp_kusama::EXISTENTIAL_DEPOSIT.into(),
							),
						),
					)
					.await
				}

				async fn right_create_account(
					right_client: Client<Right>,
					right_sign: <Right as TransactionSignScheme>::AccountKeyPair,
					account_id: AccountIdOf<Right>,
				) -> anyhow::Result<()> {
					submit_signed_extrinsic(
						right_client,
						right_sign,
						relay_polkadot_client::runtime::Call::Balances(
							relay_polkadot_client::runtime::BalancesCall::transfer(
								bp_polkadot::AccountAddress::Id(account_id),
								bp_polkadot::EXISTENTIAL_DEPOSIT.into(),
							),
						),
					)
					.await
				}

				$generic
			},
		}
	};
}

// All supported chains.
declare_chain_options!(Millau, millau);
declare_chain_options!(Rialto, rialto);
declare_chain_options!(Rococo, rococo);
declare_chain_options!(Wococo, wococo);
declare_chain_options!(Kusama, kusama);
declare_chain_options!(Polkadot, polkadot);
// All supported bridges.
declare_bridge_options!(Millau, Rialto);
declare_bridge_options!(Rococo, Wococo);
declare_bridge_options!(Kusama, Polkadot);

impl RelayHeadersAndMessages {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self, {
			let params: Params = self.into();

			let left_client = params.left.to_client::<Left>().await?;
			let left_transactions_mortality = params.left_sign.transactions_mortality()?;
			let left_sign = params.left_sign.to_keypair::<Left>()?;
			let left_messages_pallet_owner =
				params.left_messages_pallet_owner.to_keypair::<Left>()?;
			let right_client = params.right.to_client::<Right>().await?;
			let right_transactions_mortality = params.right_sign.transactions_mortality()?;
			let right_sign = params.right_sign.to_keypair::<Right>()?;
			let right_messages_pallet_owner =
				params.right_messages_pallet_owner.to_keypair::<Right>()?;

			let lanes = params.shared.lane;
			let relayer_mode = params.shared.relayer_mode.into();
			let relay_strategy = MixStrategy::new(relayer_mode);

			// create metrics registry and register standalone metrics
			let metrics_params: MetricsParams = params.shared.prometheus_params.into();
			let metrics_params = relay_utils::relay_metrics(metrics_params).into_params();
			let left_to_right_metrics =
				substrate_relay_helper::messages_metrics::standalone_metrics::<
					LeftToRightMessageLane,
				>(left_client.clone(), right_client.clone())?;
			let right_to_left_metrics = left_to_right_metrics.clone().reverse();

			// start conversion rate update loops for left/right chains
			if let Some(left_messages_pallet_owner) = left_messages_pallet_owner.clone() {
				let left_client = left_client.clone();
				let format_err = || {
					anyhow::format_err!(
						"Cannon run conversion rate updater: {} -> {}",
						Right::NAME,
						Left::NAME
					)
				};
				substrate_relay_helper::conversion_rate_update::run_conversion_rate_update_loop::<
					LeftToRightMessageLane,
					Left,
				>(
					left_client.clone(),
					TransactionParams {
						signer: left_messages_pallet_owner.clone(),
						mortality: left_transactions_mortality,
					},
					left_to_right_metrics
						.target_to_source_conversion_rate
						.as_ref()
						.ok_or_else(format_err)?
						.shared_value_ref(),
					left_to_right_metrics
						.target_to_base_conversion_rate
						.as_ref()
						.ok_or_else(format_err)?
						.shared_value_ref(),
					left_to_right_metrics
						.source_to_base_conversion_rate
						.as_ref()
						.ok_or_else(format_err)?
						.shared_value_ref(),
					CONVERSION_RATE_ALLOWED_DIFFERENCE_RATIO,
				);
			}
			if let Some(right_messages_pallet_owner) = right_messages_pallet_owner.clone() {
				let right_client = right_client.clone();
				let format_err = || {
					anyhow::format_err!(
						"Cannon run conversion rate updater: {} -> {}",
						Left::NAME,
						Right::NAME
					)
				};
				substrate_relay_helper::conversion_rate_update::run_conversion_rate_update_loop::<
					RightToLeftMessageLane,
					Right,
				>(
					right_client.clone(),
					TransactionParams {
						signer: right_messages_pallet_owner.clone(),
						mortality: right_transactions_mortality,
					},
					right_to_left_metrics
						.target_to_source_conversion_rate
						.as_ref()
						.ok_or_else(format_err)?
						.shared_value_ref(),
					right_to_left_metrics
						.target_to_base_conversion_rate
						.as_ref()
						.ok_or_else(format_err)?
						.shared_value_ref(),
					right_to_left_metrics
						.source_to_base_conversion_rate
						.as_ref()
						.ok_or_else(format_err)?
						.shared_value_ref(),
					CONVERSION_RATE_ALLOWED_DIFFERENCE_RATIO,
				);
			}

			// optionally, create relayers fund account
			if params.shared.create_relayers_fund_accounts {
				let relayer_fund_acount_id = pallet_bridge_messages::relayer_fund_account_id::<
					AccountIdOf<Left>,
					LeftAccountIdConverter,
				>();
				let relayers_fund_account_balance =
					left_client.free_native_balance(relayer_fund_acount_id.clone()).await;
				if let Err(relay_substrate_client::Error::AccountDoesNotExist) =
					relayers_fund_account_balance
				{
					log::info!(target: "bridge", "Going to create relayers fund account at {}.", Left::NAME);
					left_create_account(
						left_client.clone(),
						left_sign.clone(),
						relayer_fund_acount_id,
					)
					.await?;
				}

				let relayer_fund_acount_id = pallet_bridge_messages::relayer_fund_account_id::<
					AccountIdOf<Right>,
					RightAccountIdConverter,
				>();
				let relayers_fund_account_balance =
					right_client.free_native_balance(relayer_fund_acount_id.clone()).await;
				if let Err(relay_substrate_client::Error::AccountDoesNotExist) =
					relayers_fund_account_balance
				{
					log::info!(target: "bridge", "Going to create relayers fund account at {}.", Right::NAME);
					right_create_account(
						right_client.clone(),
						right_sign.clone(),
						relayer_fund_acount_id,
					)
					.await?;
				}
			}

			// add balance-related metrics
			let metrics_params =
				substrate_relay_helper::messages_metrics::add_relay_balances_metrics(
					left_client.clone(),
					metrics_params,
					Some(left_sign.public().into()),
					left_messages_pallet_owner.map(|kp| kp.public().into()),
				)
				.await?;
			let metrics_params =
				substrate_relay_helper::messages_metrics::add_relay_balances_metrics(
					right_client.clone(),
					metrics_params,
					Some(right_sign.public().into()),
					right_messages_pallet_owner.map(|kp| kp.public().into()),
				)
				.await?;

			// start on-demand header relays
			let left_to_right_transaction_params = TransactionParams {
				mortality: right_transactions_mortality,
				signer: right_sign.clone(),
			};
			let right_to_left_transaction_params = TransactionParams {
				mortality: left_transactions_mortality,
				signer: left_sign.clone(),
			};
			LeftToRightFinality::start_relay_guards(
				&right_client,
				&left_to_right_transaction_params,
				params.right.can_start_version_guard(),
			)
			.await?;
			RightToLeftFinality::start_relay_guards(
				&left_client,
				&right_to_left_transaction_params,
				params.left.can_start_version_guard(),
			)
			.await?;
			let left_to_right_on_demand_headers = OnDemandHeadersRelay::new::<LeftToRightFinality>(
				left_client.clone(),
				right_client.clone(),
				left_to_right_transaction_params,
				params.shared.only_mandatory_headers,
			);
			let right_to_left_on_demand_headers = OnDemandHeadersRelay::new::<RightToLeftFinality>(
				right_client.clone(),
				left_client.clone(),
				right_to_left_transaction_params,
				params.shared.only_mandatory_headers,
			);

			// Need 2x capacity since we consider both directions for each lane
			let mut message_relays = Vec::with_capacity(lanes.len() * 2);
			for lane in lanes {
				let lane = lane.into();
				let left_to_right_messages = substrate_relay_helper::messages_lane::run::<
					LeftToRightMessageLane,
				>(MessagesRelayParams {
					source_client: left_client.clone(),
					source_transaction_params: TransactionParams {
						signer: left_sign.clone(),
						mortality: left_transactions_mortality,
					},
					target_client: right_client.clone(),
					target_transaction_params: TransactionParams {
						signer: right_sign.clone(),
						mortality: right_transactions_mortality,
					},
					source_to_target_headers_relay: Some(left_to_right_on_demand_headers.clone()),
					target_to_source_headers_relay: Some(right_to_left_on_demand_headers.clone()),
					lane_id: lane,
					metrics_params: metrics_params.clone().disable(),
					standalone_metrics: Some(left_to_right_metrics.clone()),
					relay_strategy: relay_strategy.clone(),
				})
				.map_err(|e| anyhow::format_err!("{}", e))
				.boxed();
				let right_to_left_messages = substrate_relay_helper::messages_lane::run::<
					RightToLeftMessageLane,
				>(MessagesRelayParams {
					source_client: right_client.clone(),
					source_transaction_params: TransactionParams {
						signer: right_sign.clone(),
						mortality: right_transactions_mortality,
					},
					target_client: left_client.clone(),
					target_transaction_params: TransactionParams {
						signer: left_sign.clone(),
						mortality: left_transactions_mortality,
					},
					source_to_target_headers_relay: Some(right_to_left_on_demand_headers.clone()),
					target_to_source_headers_relay: Some(left_to_right_on_demand_headers.clone()),
					lane_id: lane,
					metrics_params: metrics_params.clone().disable(),
					standalone_metrics: Some(right_to_left_metrics.clone()),
					relay_strategy: relay_strategy.clone(),
				})
				.map_err(|e| anyhow::format_err!("{}", e))
				.boxed();

				message_relays.push(left_to_right_messages);
				message_relays.push(right_to_left_messages);
			}

			relay_utils::relay_metrics(metrics_params)
				.expose()
				.await
				.map_err(|e| anyhow::format_err!("{}", e))?;

			futures::future::select_all(message_relays).await.0
		})
	}
}

/// Sign and submit transaction with given call to the chain.
async fn submit_signed_extrinsic<C: Chain + TransactionSignScheme<Chain = C>>(
	client: Client<C>,
	sign: C::AccountKeyPair,
	call: CallOf<C>,
) -> anyhow::Result<()>
where
	AccountIdOf<C>: From<<<C as TransactionSignScheme>::AccountKeyPair as Pair>::Public>,
	CallOf<C>: Send,
{
	let genesis_hash = *client.genesis_hash();
	let (spec_version, transaction_version) = client.simple_runtime_version().await?;
	client
		.submit_signed_extrinsic(sign.public().into(), move |_, transaction_nonce| {
			Ok(Bytes(
				C::sign_transaction(SignParam {
					spec_version,
					transaction_version,
					genesis_hash,
					signer: sign,
					era: relay_substrate_client::TransactionEra::immortal(),
					unsigned: UnsignedTransaction::new(call.into(), transaction_nonce),
				})?
				.encode(),
			))
		})
		.await
		.map(drop)
		.map_err(|e| anyhow::format_err!("{}", e))
}
