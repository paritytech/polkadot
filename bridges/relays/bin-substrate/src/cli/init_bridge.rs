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

use crate::cli::{SourceConnectionParams, TargetConnectionParams, TargetSigningParams};
use bp_header_chain::InitializationData;
use bp_runtime::Chain as ChainBase;
use codec::Encode;
use relay_substrate_client::{Chain, SignParam, TransactionSignScheme, UnsignedTransaction};
use sp_core::{Bytes, Pair};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};

/// Initialize bridge pallet.
#[derive(StructOpt)]
pub struct InitBridge {
	/// A bridge instance to initialize.
	#[structopt(possible_values = InitBridgeName::VARIANTS, case_insensitive = true)]
	bridge: InitBridgeName,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
}

#[derive(Debug, EnumString, EnumVariantNames)]
#[strum(serialize_all = "kebab_case")]
/// Bridge to initialize.
pub enum InitBridgeName {
	MillauToRialto,
	RialtoToMillau,
	WestendToMillau,
	RococoToWococo,
	WococoToRococo,
	KusamaToPolkadot,
	PolkadotToKusama,
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			InitBridgeName::MillauToRialto => {
				type Source = relay_millau_client::Millau;
				type Target = relay_rialto_client::Rialto;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					rialto_runtime::SudoCall::sudo {
						call: Box::new(
							rialto_runtime::BridgeGrandpaMillauCall::initialize { init_data }
								.into(),
						),
					}
					.into()
				}

				$generic
			},
			InitBridgeName::RialtoToMillau => {
				type Source = relay_rialto_client::Rialto;
				type Target = relay_millau_client::Millau;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					let initialize_call = millau_runtime::BridgeGrandpaCall::<
						millau_runtime::Runtime,
						millau_runtime::RialtoGrandpaInstance,
					>::initialize {
						init_data,
					};
					millau_runtime::SudoCall::sudo { call: Box::new(initialize_call.into()) }.into()
				}

				$generic
			},
			InitBridgeName::WestendToMillau => {
				type Source = relay_westend_client::Westend;
				type Target = relay_millau_client::Millau;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					// at Westend -> Millau initialization we're not using sudo, because otherwise
					// our deployments may fail, because we need to initialize both Rialto -> Millau
					// and Westend -> Millau bridge. => since there's single possible sudo account,
					// one of transaction may fail with duplicate nonce error
					millau_runtime::BridgeGrandpaCall::<
						millau_runtime::Runtime,
						millau_runtime::WestendGrandpaInstance,
					>::initialize {
						init_data,
					}
					.into()
				}

				$generic
			},
			InitBridgeName::RococoToWococo => {
				type Source = relay_rococo_client::Rococo;
				type Target = relay_wococo_client::Wococo;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					relay_wococo_client::runtime::Call::BridgeGrandpaRococo(
						relay_wococo_client::runtime::BridgeGrandpaRococoCall::initialize(
							init_data,
						),
					)
				}

				$generic
			},
			InitBridgeName::WococoToRococo => {
				type Source = relay_wococo_client::Wococo;
				type Target = relay_rococo_client::Rococo;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					relay_rococo_client::runtime::Call::BridgeGrandpaWococo(
						relay_rococo_client::runtime::BridgeGrandpaWococoCall::initialize(
							init_data,
						),
					)
				}

				$generic
			},
			InitBridgeName::KusamaToPolkadot => {
				type Source = relay_kusama_client::Kusama;
				type Target = relay_polkadot_client::Polkadot;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					relay_polkadot_client::runtime::Call::BridgeKusamaGrandpa(
						relay_polkadot_client::runtime::BridgeKusamaGrandpaCall::initialize(
							init_data,
						),
					)
				}

				$generic
			},
			InitBridgeName::PolkadotToKusama => {
				type Source = relay_polkadot_client::Polkadot;
				type Target = relay_kusama_client::Kusama;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					relay_kusama_client::runtime::Call::BridgePolkadotGrandpa(
						relay_kusama_client::runtime::BridgePolkadotGrandpaCall::initialize(
							init_data,
						),
					)
				}

				$generic
			},
		}
	};
}

impl InitBridge {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self.bridge, {
			let source_client = self.source.to_client::<Source>().await?;
			let target_client = self.target.to_client::<Target>().await?;
			let target_sign = self.target_sign.to_keypair::<Target>()?;

			let (spec_version, transaction_version) =
				target_client.simple_runtime_version().await?;
			substrate_relay_helper::headers_initialize::initialize(
				source_client,
				target_client.clone(),
				target_sign.public().into(),
				move |transaction_nonce, initialization_data| {
					Ok(Bytes(
						Target::sign_transaction(SignParam {
							spec_version,
							transaction_version,
							genesis_hash: *target_client.genesis_hash(),
							signer: target_sign,
							era: relay_substrate_client::TransactionEra::immortal(),
							unsigned: UnsignedTransaction::new(
								encode_init_bridge(initialization_data).into(),
								transaction_nonce,
							),
						})?
						.encode(),
					))
				},
			)
			.await;

			Ok(())
		})
	}
}
