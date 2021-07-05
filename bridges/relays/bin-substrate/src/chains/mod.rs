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

//! Chain-specific relayer configuration.

pub mod millau_headers_to_rialto;
pub mod millau_messages_to_rialto;
pub mod rialto_headers_to_millau;
pub mod rialto_messages_to_millau;
pub mod rococo_headers_to_wococo;
pub mod rococo_messages_to_wococo;
pub mod westend_headers_to_millau;
pub mod wococo_headers_to_rococo;
pub mod wococo_messages_to_rococo;

mod millau;
mod rialto;
mod rococo;
mod westend;
mod wococo;

use relay_utils::metrics::{FloatJsonValueMetric, MetricsParams};

pub(crate) fn add_polkadot_kusama_price_metrics<T: finality_relay::FinalitySyncPipeline>(
	params: MetricsParams,
) -> anyhow::Result<MetricsParams> {
	Ok(
		relay_utils::relay_metrics(Some(finality_relay::metrics_prefix::<T>()), params)
			// Polkadot/Kusama prices are added as metrics here, because atm we don't have Polkadot <-> Kusama
			// relays, but we want to test metrics/dashboards in advance
			.standalone_metric(|registry, prefix| {
				FloatJsonValueMetric::new(
					registry,
					prefix,
					"https://api.coingecko.com/api/v3/simple/price?ids=Polkadot&vs_currencies=btc".into(),
					"$.polkadot.btc".into(),
					"polkadot_to_base_conversion_rate".into(),
					"Rate used to convert from DOT to some BASE tokens".into(),
				)
			})
			.map_err(|e| anyhow::format_err!("{}", e))?
			.standalone_metric(|registry, prefix| {
				FloatJsonValueMetric::new(
					registry,
					prefix,
					"https://api.coingecko.com/api/v3/simple/price?ids=Kusama&vs_currencies=btc".into(),
					"$.kusama.btc".into(),
					"kusama_to_base_conversion_rate".into(),
					"Rate used to convert from KSM to some BASE tokens".into(),
				)
			})
			.map_err(|e| anyhow::format_err!("{}", e))?
			.into_params(),
	)
}

#[cfg(test)]
mod tests {
	use crate::cli::{encode_call, send_message};
	use bp_messages::source_chain::TargetHeaderChain;
	use codec::Encode;
	use frame_support::dispatch::GetDispatchInfo;
	use relay_millau_client::Millau;
	use relay_rialto_client::Rialto;
	use relay_substrate_client::TransactionSignScheme;
	use sp_core::Pair;
	use sp_runtime::traits::{IdentifyAccount, Verify};

	#[test]
	fn millau_signature_is_valid_on_rialto() {
		let millau_sign = relay_millau_client::SigningParams::from_string("//Dave", None).unwrap();

		let call = rialto_runtime::Call::System(rialto_runtime::SystemCall::remark(vec![]));

		let millau_public: bp_millau::AccountSigner = millau_sign.public().into();
		let millau_account_id: bp_millau::AccountId = millau_public.into_account();

		let digest = millau_runtime::millau_to_rialto_account_ownership_digest(
			&call,
			millau_account_id,
			rialto_runtime::VERSION.spec_version,
		);

		let rialto_signer = relay_rialto_client::SigningParams::from_string("//Dave", None).unwrap();
		let signature = rialto_signer.sign(&digest);

		assert!(signature.verify(&digest[..], &rialto_signer.public()));
	}

	#[test]
	fn rialto_signature_is_valid_on_millau() {
		let rialto_sign = relay_rialto_client::SigningParams::from_string("//Dave", None).unwrap();

		let call = millau_runtime::Call::System(millau_runtime::SystemCall::remark(vec![]));

		let rialto_public: bp_rialto::AccountSigner = rialto_sign.public().into();
		let rialto_account_id: bp_rialto::AccountId = rialto_public.into_account();

		let digest = rialto_runtime::rialto_to_millau_account_ownership_digest(
			&call,
			rialto_account_id,
			millau_runtime::VERSION.spec_version,
		);

		let millau_signer = relay_millau_client::SigningParams::from_string("//Dave", None).unwrap();
		let signature = millau_signer.sign(&digest);

		assert!(signature.verify(&digest[..], &millau_signer.public()));
	}

	#[test]
	fn maximal_rialto_to_millau_message_arguments_size_is_computed_correctly() {
		use rialto_runtime::millau_messages::Millau;

		let maximal_remark_size = encode_call::compute_maximal_message_arguments_size(
			bp_rialto::max_extrinsic_size(),
			bp_millau::max_extrinsic_size(),
		);

		let call: millau_runtime::Call = millau_runtime::SystemCall::remark(vec![42; maximal_remark_size as _]).into();
		let payload = send_message::message_payload(
			Default::default(),
			call.get_dispatch_info().weight,
			bp_message_dispatch::CallOrigin::SourceRoot,
			&call,
		);
		assert_eq!(Millau::verify_message(&payload), Ok(()));

		let call: millau_runtime::Call =
			millau_runtime::SystemCall::remark(vec![42; (maximal_remark_size + 1) as _]).into();
		let payload = send_message::message_payload(
			Default::default(),
			call.get_dispatch_info().weight,
			bp_message_dispatch::CallOrigin::SourceRoot,
			&call,
		);
		assert!(Millau::verify_message(&payload).is_err());
	}

	#[test]
	fn maximal_size_remark_to_rialto_is_generated_correctly() {
		assert!(
			bridge_runtime_common::messages::target::maximal_incoming_message_size(
				bp_rialto::max_extrinsic_size()
			) > bp_millau::max_extrinsic_size(),
			"We can't actually send maximal messages to Rialto from Millau, because Millau extrinsics can't be that large",
		)
	}

	#[test]
	fn maximal_rialto_to_millau_message_dispatch_weight_is_computed_correctly() {
		use rialto_runtime::millau_messages::Millau;

		let maximal_dispatch_weight =
			send_message::compute_maximal_message_dispatch_weight(bp_millau::max_extrinsic_weight());
		let call: millau_runtime::Call = rialto_runtime::SystemCall::remark(vec![]).into();

		let payload = send_message::message_payload(
			Default::default(),
			maximal_dispatch_weight,
			bp_message_dispatch::CallOrigin::SourceRoot,
			&call,
		);
		assert_eq!(Millau::verify_message(&payload), Ok(()));

		let payload = send_message::message_payload(
			Default::default(),
			maximal_dispatch_weight + 1,
			bp_message_dispatch::CallOrigin::SourceRoot,
			&call,
		);
		assert!(Millau::verify_message(&payload).is_err());
	}

	#[test]
	fn maximal_weight_fill_block_to_rialto_is_generated_correctly() {
		use millau_runtime::rialto_messages::Rialto;

		let maximal_dispatch_weight =
			send_message::compute_maximal_message_dispatch_weight(bp_rialto::max_extrinsic_weight());
		let call: rialto_runtime::Call = millau_runtime::SystemCall::remark(vec![]).into();

		let payload = send_message::message_payload(
			Default::default(),
			maximal_dispatch_weight,
			bp_message_dispatch::CallOrigin::SourceRoot,
			&call,
		);
		assert_eq!(Rialto::verify_message(&payload), Ok(()));

		let payload = send_message::message_payload(
			Default::default(),
			maximal_dispatch_weight + 1,
			bp_message_dispatch::CallOrigin::SourceRoot,
			&call,
		);
		assert!(Rialto::verify_message(&payload).is_err());
	}

	#[test]
	fn rialto_tx_extra_bytes_constant_is_correct() {
		let rialto_call = rialto_runtime::Call::System(rialto_runtime::SystemCall::remark(vec![]));
		let rialto_tx = Rialto::sign_transaction(
			Default::default(),
			&sp_keyring::AccountKeyring::Alice.pair(),
			0,
			rialto_call.clone(),
		);
		let extra_bytes_in_transaction = rialto_tx.encode().len() - rialto_call.encode().len();
		assert!(
			bp_rialto::TX_EXTRA_BYTES as usize >= extra_bytes_in_transaction,
			"Hardcoded number of extra bytes in Rialto transaction {} is lower than actual value: {}",
			bp_rialto::TX_EXTRA_BYTES,
			extra_bytes_in_transaction,
		);
	}

	#[test]
	fn millau_tx_extra_bytes_constant_is_correct() {
		let millau_call = millau_runtime::Call::System(millau_runtime::SystemCall::remark(vec![]));
		let millau_tx = Millau::sign_transaction(
			Default::default(),
			&sp_keyring::AccountKeyring::Alice.pair(),
			0,
			millau_call.clone(),
		);
		let extra_bytes_in_transaction = millau_tx.encode().len() - millau_call.encode().len();
		assert!(
			bp_millau::TX_EXTRA_BYTES as usize >= extra_bytes_in_transaction,
			"Hardcoded number of extra bytes in Millau transaction {} is lower than actual value: {}",
			bp_millau::TX_EXTRA_BYTES,
			extra_bytes_in_transaction,
		);
	}
}

#[cfg(test)]
mod rococo_tests {
	use bp_header_chain::justification::GrandpaJustification;
	use codec::Encode;

	#[test]
	fn scale_compatibility_of_bridges_call() {
		// given
		let header = sp_runtime::generic::Header {
			parent_hash: Default::default(),
			number: Default::default(),
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: sp_runtime::generic::Digest { logs: vec![] },
		};

		let justification = GrandpaJustification {
			round: 0,
			commit: finality_grandpa::Commit {
				target_hash: Default::default(),
				target_number: Default::default(),
				precommits: vec![],
			},
			votes_ancestries: vec![],
		};

		let actual = relay_rococo_client::runtime::BridgeGrandpaWococoCall::submit_finality_proof(
			header.clone(),
			justification.clone(),
		);
		let expected = millau_runtime::BridgeGrandpaRialtoCall::<millau_runtime::Runtime>::submit_finality_proof(
			header,
			justification,
		);

		// when
		let actual_encoded = actual.encode();
		let expected_encoded = expected.encode();

		// then
		assert_eq!(
			actual_encoded, expected_encoded,
			"\n\nEncoding difference.\nGot {:#?} \nExpected: {:#?}",
			actual, expected
		);
	}
}

#[cfg(test)]
mod westend_tests {
	use bp_header_chain::justification::GrandpaJustification;
	use codec::Encode;

	#[test]
	fn scale_compatibility_of_bridges_call() {
		// given
		let header = sp_runtime::generic::Header {
			parent_hash: Default::default(),
			number: Default::default(),
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: sp_runtime::generic::Digest { logs: vec![] },
		};

		let justification = GrandpaJustification {
			round: 0,
			commit: finality_grandpa::Commit {
				target_hash: Default::default(),
				target_number: Default::default(),
				precommits: vec![],
			},
			votes_ancestries: vec![],
		};

		let actual = bp_westend::BridgeGrandpaRococoCall::submit_finality_proof(header.clone(), justification.clone());
		let expected = millau_runtime::BridgeGrandpaRialtoCall::<millau_runtime::Runtime>::submit_finality_proof(
			header,
			justification,
		);

		// when
		let actual_encoded = actual.encode();
		let expected_encoded = expected.encode();

		// then
		assert_eq!(
			actual_encoded, expected_encoded,
			"\n\nEncoding difference.\nGot {:#?} \nExpected: {:#?}",
			actual, expected
		);
	}
}
