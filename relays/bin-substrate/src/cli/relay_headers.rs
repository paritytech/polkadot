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

use crate::cli::{PrometheusParams, SourceConnectionParams, TargetConnectionParams, TargetSigningParams};
use crate::finality_pipeline::SubstrateFinalitySyncPipeline;
use structopt::{clap::arg_enum, StructOpt};

/// Start headers relayer process.
#[derive(StructOpt)]
pub struct RelayHeaders {
	/// A bridge instance to relay headers for.
	#[structopt(possible_values = &RelayHeadersBridge::variants(), case_insensitive = true)]
	bridge: RelayHeadersBridge,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
	#[structopt(flatten)]
	prometheus_params: PrometheusParams,
}

// TODO [#851] Use kebab-case.
arg_enum! {
	#[derive(Debug)]
	/// Headers relay bridge.
	pub enum RelayHeadersBridge {
		MillauToRialto,
		RialtoToMillau,
		WestendToMillau,
		WestendToRococo,
		RococoToWestend,
	}
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			RelayHeadersBridge::MillauToRialto => {
				type Source = relay_millau_client::Millau;
				type Target = relay_rialto_client::Rialto;
				type Finality = crate::chains::millau_headers_to_rialto::MillauFinalityToRialto;

				$generic
			}
			RelayHeadersBridge::RialtoToMillau => {
				type Source = relay_rialto_client::Rialto;
				type Target = relay_millau_client::Millau;
				type Finality = crate::chains::rialto_headers_to_millau::RialtoFinalityToMillau;

				$generic
			}
			RelayHeadersBridge::WestendToMillau => {
				type Source = relay_westend_client::Westend;
				type Target = relay_millau_client::Millau;
				type Finality = crate::chains::westend_headers_to_millau::WestendFinalityToMillau;

				$generic
			}
			RelayHeadersBridge::WestendToRococo => {
				type Source = relay_westend_client::Westend;
				type Target = relay_rococo_client::Rococo;
				type Finality = crate::chains::westend_headers_to_rococo::WestendFinalityToRococo;

				$generic
			}
			RelayHeadersBridge::RococoToWestend => {
				type Source = relay_rococo_client::Rococo;
				type Target = relay_westend_client::Westend;
				type Finality = crate::chains::rococo_headers_to_westend::RococoFinalityToWestend;

				$generic
			}
		}
	};
}

impl RelayHeaders {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self.bridge, {
			let source_client = self.source.to_client::<Source>().await?;
			let target_client = self.target.to_client::<Target>().await?;
			let target_sign = self.target_sign.to_keypair::<Target>()?;
			let metrics_params = Finality::customize_metrics(self.prometheus_params.into())?;

			crate::finality_pipeline::run(
				Finality::new(target_client.clone(), target_sign),
				source_client,
				target_client,
				metrics_params,
			)
			.await
		})
	}
}
