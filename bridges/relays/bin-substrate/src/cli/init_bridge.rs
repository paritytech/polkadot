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
use relay_substrate_client::{Chain, TransactionSignScheme};
use sp_core::{Bytes, Pair};
use structopt::{clap::arg_enum, StructOpt};

/// Initialize bridge pallet.
#[derive(StructOpt)]
pub struct InitBridge {
	/// A bridge instance to initalize.
	#[structopt(possible_values = &InitBridgeName::variants(), case_insensitive = true)]
	bridge: InitBridgeName,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
}

// TODO [#851] Use kebab-case.
arg_enum! {
	#[derive(Debug)]
	/// Bridge to initialize.
	pub enum InitBridgeName {
		MillauToRialto,
		RialtoToMillau,
		WestendToMillau,
		RococoToWococo,
		WococoToRococo,
	}
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
					rialto_runtime::SudoCall::sudo(Box::new(
						rialto_runtime::BridgeGrandpaMillauCall::initialize(init_data).into(),
					))
					.into()
				}

				$generic
			}
			InitBridgeName::RialtoToMillau => {
				type Source = relay_rialto_client::Rialto;
				type Target = relay_millau_client::Millau;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					let initialize_call = millau_runtime::BridgeGrandpaRialtoCall::<
						millau_runtime::Runtime,
						millau_runtime::RialtoGrandpaInstance,
					>::initialize(init_data);
					millau_runtime::SudoCall::sudo(Box::new(initialize_call.into())).into()
				}

				$generic
			}
			InitBridgeName::WestendToMillau => {
				type Source = relay_westend_client::Westend;
				type Target = relay_millau_client::Millau;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					// at Westend -> Millau initialization we're not using sudo, because otherwise our deployments
					// may fail, because we need to initialize both Rialto -> Millau and Westend -> Millau bridge.
					// => since there's single possible sudo account, one of transaction may fail with duplicate nonce error
					millau_runtime::BridgeGrandpaWestendCall::<
						millau_runtime::Runtime,
						millau_runtime::WestendGrandpaInstance,
					>::initialize(init_data)
					.into()
				}

				$generic
			}
			InitBridgeName::RococoToWococo => {
				type Source = relay_rococo_client::Rococo;
				type Target = relay_wococo_client::Wococo;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					bp_wococo::Call::BridgeGrandpaRococo(bp_wococo::BridgeGrandpaRococoCall::initialize(init_data))
				}

				$generic
			}
			InitBridgeName::WococoToRococo => {
				type Source = relay_wococo_client::Wococo;
				type Target = relay_rococo_client::Rococo;

				fn encode_init_bridge(
					init_data: InitializationData<<Source as ChainBase>::Header>,
				) -> <Target as Chain>::Call {
					bp_rococo::Call::BridgeGrandpaWococo(bp_rococo::BridgeGrandpaWococoCall::initialize(init_data))
				}

				$generic
			}
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

			crate::headers_initialize::initialize(
				source_client,
				target_client.clone(),
				target_sign.public().into(),
				move |transaction_nonce, initialization_data| {
					Bytes(
						Target::sign_transaction(
							*target_client.genesis_hash(),
							&target_sign,
							transaction_nonce,
							encode_init_bridge(initialization_data),
						)
						.encode(),
					)
				},
			)
			.await;

			Ok(())
		})
	}
}
