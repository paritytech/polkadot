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
	encode_call::{self, CliEncodeCall},
	estimate_fee::{estimate_message_delivery_and_dispatch_fee, ConversionRateOverride},
	Balance, ExplicitOrMaximal, HexBytes, HexLaneId, Origins, SourceConnectionParams,
	SourceSigningParams, TargetConnectionParams, TargetSigningParams,
};
use bp_message_dispatch::{CallOrigin, MessagePayload};
use bp_runtime::Chain as _;
use codec::Encode;
use frame_support::weights::Weight;
use relay_substrate_client::{Chain, SignParam, TransactionSignScheme, UnsignedTransaction};
use sp_core::{Bytes, Pair};
use sp_runtime::{traits::IdentifyAccount, AccountId32, MultiSignature, MultiSigner};
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
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
	/// Hex-encoded lane id. Defaults to `00000000`.
	#[structopt(long, default_value = "00000000")]
	lane: HexLaneId,
	/// A way to override conversion rate between bridge tokens.
	///
	/// If not specified, conversion rate from runtime storage is used. It may be obsolete and
	/// your message won't be relayed.
	#[structopt(long)]
	conversion_rate_override: Option<ConversionRateOverride>,
	/// Where dispatch fee is paid?
	#[structopt(
		long,
		possible_values = DispatchFeePayment::VARIANTS,
		case_insensitive = true,
		default_value = "at-source-chain",
	)]
	dispatch_fee_payment: DispatchFeePayment,
	/// Dispatch weight of the message. If not passed, determined automatically.
	#[structopt(long)]
	dispatch_weight: Option<ExplicitOrMaximal<Weight>>,
	/// Delivery and dispatch fee in source chain base currency units. If not passed, determined
	/// automatically.
	#[structopt(long)]
	fee: Option<Balance>,
	/// Message type.
	#[structopt(subcommand)]
	message: crate::cli::encode_call::Call,
	/// The origin to use when dispatching the message on the target chain. Defaults to
	/// `SourceAccount`.
	#[structopt(long, possible_values = &Origins::variants(), default_value = "Source")]
	origin: Origins,

	// Normally we don't need to connect to the target chain to send message. But for testing
	// we may want to use **actual** `spec_version` of the target chain when composing a message.
	// Then we'll need to read version from the target chain node.
	#[structopt(flatten)]
	target: TargetConnectionParams,
}

impl SendMessage {
	pub async fn encode_payload(
		&mut self,
	) -> anyhow::Result<MessagePayload<AccountId32, MultiSigner, MultiSignature, Vec<u8>>> {
		crate::select_full_bridge!(self.bridge, {
			let SendMessage {
				source_sign,
				target_sign,
				ref mut message,
				dispatch_fee_payment,
				dispatch_weight,
				origin,
				bridge,
				..
			} = self;

			let source_sign = source_sign.to_keypair::<Source>()?;

			encode_call::preprocess_call::<Source, Target>(message, bridge.bridge_instance_index());
			let target_call = Target::encode_call(message)?;
			let target_spec_version = self.target.selected_chain_spec_version::<Target>().await?;

			let payload = {
				let target_call_weight = prepare_call_dispatch_weight(
					dispatch_weight,
					|| {
						Ok(ExplicitOrMaximal::Explicit(
							Target::get_dispatch_info(&target_call)?.weight,
						))
					},
					compute_maximal_message_dispatch_weight(Target::max_extrinsic_weight()),
				)?;
				let source_sender_public: MultiSigner = source_sign.public().into();
				let source_account_id = source_sender_public.into_account();

				message_payload(
					target_spec_version,
					target_call_weight,
					match origin {
						Origins::Source => CallOrigin::SourceAccount(source_account_id),
						Origins::Target => {
							let target_sign = target_sign.to_keypair::<Target>()?;
							let digest = account_ownership_digest(
								&target_call,
								source_account_id.clone(),
								target_spec_version,
							);
							let target_origin_public = target_sign.public();
							let digest_signature = target_sign.sign(&digest);
							CallOrigin::TargetAccount(
								source_account_id,
								target_origin_public.into(),
								digest_signature.into(),
							)
						},
					},
					&target_call,
					*dispatch_fee_payment,
				)
			};
			Ok(payload)
		})
	}

	/// Run the command.
	pub async fn run(mut self) -> anyhow::Result<()> {
		crate::select_full_bridge!(self.bridge, {
			let payload = self.encode_payload().await?;

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
			let dispatch_weight = payload.weight;
			let payload_len = payload.encode().len();
			let send_message_call = Source::encode_call(&encode_call::Call::BridgeSendMessage {
				bridge_instance_index: self.bridge.bridge_instance_index(),
				lane: self.lane,
				payload: HexBytes::encode(&payload),
				fee,
			})?;

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
						"Sending message to {}. Lane: {:?}. Size: {}. Dispatch weight: {}. Fee: {}",
						Target::NAME,
						lane,
						payload_len,
						dispatch_weight,
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

fn prepare_call_dispatch_weight(
	user_specified_dispatch_weight: &Option<ExplicitOrMaximal<Weight>>,
	weight_from_pre_dispatch_call: impl Fn() -> anyhow::Result<ExplicitOrMaximal<Weight>>,
	maximal_allowed_weight: Weight,
) -> anyhow::Result<Weight> {
	match user_specified_dispatch_weight
		.clone()
		.map(Ok)
		.unwrap_or_else(weight_from_pre_dispatch_call)?
	{
		ExplicitOrMaximal::Explicit(weight) => Ok(weight),
		ExplicitOrMaximal::Maximal => Ok(maximal_allowed_weight),
	}
}

pub(crate) fn message_payload<SAccountId, TPublic, TSignature>(
	spec_version: u32,
	weight: Weight,
	origin: CallOrigin<SAccountId, TPublic, TSignature>,
	call: &impl Encode,
	dispatch_fee_payment: DispatchFeePayment,
) -> MessagePayload<SAccountId, TPublic, TSignature, Vec<u8>>
where
	SAccountId: Encode + Debug,
	TPublic: Encode + Debug,
	TSignature: Encode + Debug,
{
	// Display nicely formatted call.
	let payload = MessagePayload {
		spec_version,
		weight,
		origin,
		dispatch_fee_payment: dispatch_fee_payment.into(),
		call: HexBytes::encode(call),
	};

	log::info!(target: "bridge", "Created Message Payload: {:#?}", payload);
	log::info!(target: "bridge", "Encoded Message Payload: {:?}", HexBytes::encode(&payload));

	// re-pack to return `Vec<u8>`
	let MessagePayload { spec_version, weight, origin, dispatch_fee_payment, call } = payload;
	MessagePayload { spec_version, weight, origin, dispatch_fee_payment, call: call.0 }
}

pub(crate) fn compute_maximal_message_dispatch_weight(maximal_extrinsic_weight: Weight) -> Weight {
	bridge_runtime_common::messages::target::maximal_incoming_message_dispatch_weight(
		maximal_extrinsic_weight,
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::CliChain;
	use hex_literal::hex;

	#[async_std::test]
	async fn send_remark_rialto_to_millau() {
		// given
		let mut send_message = SendMessage::from_iter(vec![
			"send-message",
			"rialto-to-millau",
			"--source-port",
			"1234",
			"--source-signer",
			"//Alice",
			"--conversion-rate-override",
			"0.75",
			"remark",
			"--remark-payload",
			"1234",
		]);

		// when
		let payload = send_message.encode_payload().await.unwrap();

		// then
		assert_eq!(
			payload,
			MessagePayload {
				spec_version: relay_millau_client::Millau::RUNTIME_VERSION.spec_version,
				weight: 0,
				origin: CallOrigin::SourceAccount(
					sp_keyring::AccountKeyring::Alice.to_account_id()
				),
				dispatch_fee_payment: bp_runtime::messages::DispatchFeePayment::AtSourceChain,
				call: hex!("0001081234").to_vec(),
			}
		);
	}

	#[async_std::test]
	async fn send_remark_millau_to_rialto() {
		// given
		let mut send_message = SendMessage::from_iter(vec![
			"send-message",
			"millau-to-rialto",
			"--source-port",
			"1234",
			"--source-signer",
			"//Alice",
			"--origin",
			"Target",
			"--target-signer",
			"//Bob",
			"--conversion-rate-override",
			"metric",
			"remark",
			"--remark-payload",
			"1234",
		]);

		// when
		let payload = send_message.encode_payload().await.unwrap();

		// then
		// Since signatures are randomized we extract it from here and only check the rest.
		let signature = match payload.origin {
			CallOrigin::TargetAccount(_, _, ref sig) => sig.clone(),
			_ => panic!("Unexpected `CallOrigin`: {:?}", payload),
		};
		assert_eq!(
			payload,
			MessagePayload {
				spec_version: relay_millau_client::Millau::RUNTIME_VERSION.spec_version,
				weight: 0,
				origin: CallOrigin::TargetAccount(
					sp_keyring::AccountKeyring::Alice.to_account_id(),
					sp_keyring::AccountKeyring::Bob.into(),
					signature,
				),
				dispatch_fee_payment: bp_runtime::messages::DispatchFeePayment::AtSourceChain,
				call: hex!("0001081234").to_vec(),
			}
		);
	}

	#[test]
	fn accepts_send_message_command_without_target_sign_options() {
		// given
		let send_message = SendMessage::from_iter_safe(vec![
			"send-message",
			"rialto-to-millau",
			"--source-port",
			"1234",
			"--source-signer",
			"//Alice",
			"--origin",
			"Target",
			"remark",
			"--remark-payload",
			"1234",
		]);

		assert!(send_message.is_ok());
	}

	#[async_std::test]
	async fn accepts_non_default_dispatch_fee_payment() {
		// given
		let mut send_message = SendMessage::from_iter(vec![
			"send-message",
			"rialto-to-millau",
			"--source-port",
			"1234",
			"--source-signer",
			"//Alice",
			"--dispatch-fee-payment",
			"at-target-chain",
			"remark",
		]);

		// when
		let payload = send_message.encode_payload().await.unwrap();

		// then
		assert_eq!(
			payload.dispatch_fee_payment,
			bp_runtime::messages::DispatchFeePayment::AtTargetChain
		);
	}
}
