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

use crate::cli::{
	bridge::FullBridge,
	encode_message::{self, CliEncodeMessage},
	estimate_fee::{estimate_message_delivery_and_dispatch_fee, ConversionRateOverride},
	Balance, HexBytes, HexLaneId, SourceConnectionParams, SourceSigningParams,
};
use codec::Encode;
use relay_substrate_client::{Chain, SignParam, TransactionSignScheme, UnsignedTransaction};
use sp_core::{Bytes, Pair};
use sp_runtime::AccountId32;
use std::fmt::Debug;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};

/// Relayer operating mode.
#[derive(Debug, EnumString, EnumVariantNames, Clone, Copy, PartialEq, Eq)]
#[strum(serialize_all = "kebab_case")]
pub enum DispatchFeePayment {
	/// The dispatch fee is paid at the source chain.
	AtSourceChain,
	/// The dispatch fee is paid at the target chain.
	AtTargetChain,
}

impl From<DispatchFeePayment> for bp_runtime::messages::DispatchFeePayment {
	fn from(dispatch_fee_payment: DispatchFeePayment) -> Self {
		match dispatch_fee_payment {
			DispatchFeePayment::AtSourceChain => Self::AtSourceChain,
			DispatchFeePayment::AtTargetChain => Self::AtTargetChain,
		}
	}
}

/// Send bridge message.
#[derive(StructOpt)]
pub struct SendMessage {
	/// A bridge instance to encode call for.
	#[structopt(possible_values = FullBridge::VARIANTS, case_insensitive = true)]
	bridge: FullBridge,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	source_sign: SourceSigningParams,
	/// Hex-encoded lane id. Defaults to `00000000`.
	#[structopt(long, default_value = "00000000")]
	lane: HexLaneId,
	/// A way to override conversion rate between bridge tokens.
	///
	/// If not specified, conversion rate from runtime storage is used. It may be obsolete and
	/// your message won't be relayed.
	#[structopt(long)]
	conversion_rate_override: Option<ConversionRateOverride>,
	/// Delivery and dispatch fee in source chain base currency units. If not passed, determined
	/// automatically.
	#[structopt(long)]
	fee: Option<Balance>,
	/// Message type.
	#[structopt(subcommand)]
	message: crate::cli::encode_message::Message,
}

impl SendMessage {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		crate::select_full_bridge!(self.bridge, {
			let payload = encode_message::encode_message::<Source, Target>(&self.message)?;

			let source_client = self.source.to_client::<Source>().await?;
			let source_sign = self.source_sign.to_keypair::<Source>()?;

			let lane = self.lane.clone().into();
			let conversion_rate_override = self.conversion_rate_override;
			let fee = match self.fee {
				Some(fee) => fee,
				None => Balance(
					estimate_message_delivery_and_dispatch_fee::<Source, Target, _>(
						&source_client,
						conversion_rate_override,
						ESTIMATE_MESSAGE_FEE_METHOD,
						lane,
						payload.clone(),
					)
					.await? as _,
				),
			};
			let payload_len = payload.encode().len();
			let send_message_call = Source::encode_send_message_call(
				self.lane.0,
				payload,
				fee.cast().into(),
				self.bridge.bridge_instance_index(),
			)?;

			let source_genesis_hash = *source_client.genesis_hash();
			let (spec_version, transaction_version) =
				source_client.simple_runtime_version().await?;
			let estimated_transaction_fee = source_client
				.estimate_extrinsic_fee(Bytes(
					Source::sign_transaction(SignParam {
						spec_version,
						transaction_version,
						genesis_hash: source_genesis_hash,
						signer: source_sign.clone(),
						era: relay_substrate_client::TransactionEra::immortal(),
						unsigned: UnsignedTransaction::new(send_message_call.clone(), 0),
					})?
					.encode(),
				))
				.await?;
			source_client
				.submit_signed_extrinsic(source_sign.public().into(), move |_, transaction_nonce| {
					let signed_source_call = Source::sign_transaction(SignParam {
						spec_version,
						transaction_version,
						genesis_hash: source_genesis_hash,
						signer: source_sign.clone(),
						era: relay_substrate_client::TransactionEra::immortal(),
						unsigned: UnsignedTransaction::new(send_message_call, transaction_nonce),
					})?
					.encode();

					log::info!(
						target: "bridge",
						"Sending message to {}. Lane: {:?}. Size: {}. Fee: {}",
						Target::NAME,
						lane,
						payload_len,
						fee,
					);
					log::info!(
						target: "bridge",
						"The source account ({:?}) balance will be reduced by (at most) {} (message fee) + {} (tx fee	) = {} {} tokens",
						AccountId32::from(source_sign.public()),
						fee.0,
						estimated_transaction_fee.inclusion_fee(),
						fee.0.saturating_add(estimated_transaction_fee.inclusion_fee() as _),
						Source::NAME,
					);
					log::info!(
						target: "bridge",
						"Signed {} Call: {:?}",
						Source::NAME,
						HexBytes::encode(&signed_source_call)
					);

					Ok(Bytes(signed_source_call))
				})
				.await?;
		});

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::ExplicitOrMaximal;

	#[test]
	fn send_raw_rialto_to_millau() {
		// given
		let send_message = SendMessage::from_iter(vec![
			"send-message",
			"rialto-to-millau",
			"--source-port",
			"1234",
			"--source-signer",
			"//Alice",
			"--conversion-rate-override",
			"0.75",
			"raw",
			"dead",
		]);

		// then
		assert_eq!(send_message.bridge, FullBridge::RialtoToMillau);
		assert_eq!(send_message.source.source_port, 1234);
		assert_eq!(send_message.source_sign.source_signer, Some("//Alice".into()));
		assert_eq!(
			send_message.conversion_rate_override,
			Some(ConversionRateOverride::Explicit(0.75))
		);
		assert_eq!(
			send_message.message,
			crate::cli::encode_message::Message::Raw { data: HexBytes(vec![0xDE, 0xAD]) }
		);
	}

	#[test]
	fn send_sized_rialto_to_millau() {
		// given
		let send_message = SendMessage::from_iter(vec![
			"send-message",
			"rialto-to-millau",
			"--source-port",
			"1234",
			"--source-signer",
			"//Alice",
			"--conversion-rate-override",
			"metric",
			"sized",
			"max",
		]);

		// then
		assert_eq!(send_message.bridge, FullBridge::RialtoToMillau);
		assert_eq!(send_message.source.source_port, 1234);
		assert_eq!(send_message.source_sign.source_signer, Some("//Alice".into()));
		assert_eq!(send_message.conversion_rate_override, Some(ConversionRateOverride::Metric));
		assert_eq!(
			send_message.message,
			crate::cli::encode_message::Message::Sized { size: ExplicitOrMaximal::Maximal }
		);
	}
}
