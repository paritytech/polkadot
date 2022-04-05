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

use crate::{
	chains::{
		kusama_headers_to_polkadot::KusamaFinalityToPolkadot,
		polkadot_headers_to_kusama::PolkadotFinalityToKusama,
	},
	cli::{
		swap_tokens::wait_until_transaction_is_finalized, SourceConnectionParams,
		TargetConnectionParams, TargetSigningParams,
	},
};
use bp_header_chain::justification::GrandpaJustification;
use bp_runtime::Chain;
use codec::Encode;
use finality_relay::{SourceClient, SourceHeader};
use frame_support::weights::Weight;
use num_traits::One;
use pallet_bridge_grandpa::weights::WeightInfo;
use relay_substrate_client::{
	AccountIdOf, BlockNumberOf, Chain as _, Client, Error as SubstrateError, HeaderOf, SignParam,
	SyncHeader, TransactionEra, TransactionSignScheme, UnsignedTransaction,
};
use sp_core::{Bytes, Pair};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};
use substrate_relay_helper::{
	finality_pipeline::SubstrateFinalitySyncPipeline, finality_source::SubstrateFinalitySource,
	finality_target::SubstrateFinalityTarget, messages_source::read_client_state,
	TransactionParams,
};

/// Reinitialize bridge pallet.
#[derive(Debug, PartialEq, StructOpt)]
pub struct ReinitBridge {
	/// A bridge instance to reinitialize.
	#[structopt(possible_values = ReinitBridgeName::VARIANTS, case_insensitive = true)]
	bridge: ReinitBridgeName,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
}

#[derive(Debug, EnumString, EnumVariantNames, PartialEq)]
#[strum(serialize_all = "kebab_case")]
/// Bridge to initialize.
pub enum ReinitBridgeName {
	KusamaToPolkadot,
	PolkadotToKusama,
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			ReinitBridgeName::KusamaToPolkadot => {
				use relay_polkadot_client::runtime;

				type Finality = KusamaFinalityToPolkadot;
				type Call = runtime::Call;

				fn submit_finality_proof_call(
					header_and_proof: HeaderAndProof<Finality>,
				) -> runtime::Call {
					runtime::Call::BridgeKusamaGrandpa(
						runtime::BridgeKusamaGrandpaCall::submit_finality_proof(
							Box::new(header_and_proof.0.into_inner()),
							header_and_proof.1,
						),
					)
				}

				fn set_pallet_operation_mode_call(operational: bool) -> runtime::Call {
					runtime::Call::BridgeKusamaGrandpa(
						runtime::BridgeKusamaGrandpaCall::set_operational(operational),
					)
				}

				fn batch_all_call(calls: Vec<Call>) -> runtime::Call {
					runtime::Call::Utility(runtime::UtilityCall::batch_all(calls))
				}

				$generic
			},
			ReinitBridgeName::PolkadotToKusama => {
				use relay_kusama_client::runtime;

				type Finality = PolkadotFinalityToKusama;
				type Call = runtime::Call;

				fn submit_finality_proof_call(
					header_and_proof: HeaderAndProof<Finality>,
				) -> runtime::Call {
					runtime::Call::BridgePolkadotGrandpa(
						runtime::BridgePolkadotGrandpaCall::submit_finality_proof(
							Box::new(header_and_proof.0.into_inner()),
							header_and_proof.1,
						),
					)
				}

				fn set_pallet_operation_mode_call(operational: bool) -> runtime::Call {
					runtime::Call::BridgePolkadotGrandpa(
						runtime::BridgePolkadotGrandpaCall::set_operational(operational),
					)
				}

				fn batch_all_call(calls: Vec<Call>) -> runtime::Call {
					runtime::Call::Utility(runtime::UtilityCall::batch_all(calls))
				}

				$generic
			},
		}
	};
}

impl ReinitBridge {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self.bridge, {
			type Source = <Finality as SubstrateFinalitySyncPipeline>::SourceChain;
			type Target = <Finality as SubstrateFinalitySyncPipeline>::TargetChain;

			let source_client = self.source.to_client::<Source>().await?;
			let target_client = self.target.to_client::<Target>().await?;
			let target_sign = self.target_sign.to_keypair::<Target>()?;
			let transaction_params = TransactionParams {
				signer: target_sign,
				mortality: self.target_sign.target_transactions_mortality,
			};

			let finality_source =
				SubstrateFinalitySource::<Finality>::new(source_client.clone(), None);
			let finality_target = SubstrateFinalityTarget::<Finality>::new(
				target_client.clone(),
				transaction_params.clone(),
			);

			// this subcommand assumes that the pallet at the target chain is halted
			ensure_pallet_operating_mode(&finality_target, false).await?;

			// we can't call `finality_target.best_finalized_source_block_id()`, because pallet is
			// halted and the call will fail => just use what it uses internally
			let current_number =
				best_source_block_number_at_target::<Finality>(&target_client).await?;
			let target_number = finality_source.best_finalized_block_number().await?;
			log::info!(
				target: "bridge",
				"Best finalized {} header: at {}: {}, at {}: {}",
				Source::NAME,
				Source::NAME,
				target_number,
				Target::NAME,
				current_number,
			);

			// prepare list of mandatory headers from the range `(current_number; target_number]`
			let headers_to_submit = find_mandatory_headers_in_range(
				&finality_source,
				(current_number + 1, target_number),
			)
			.await?;
			let latest_andatory_header_number = headers_to_submit.last().map(|(h, _)| h.number());
			log::info!(
				target: "bridge",
				"Missing {} mandatory {} headers at {}",
				headers_to_submit.len(),
				Source::NAME,
				Target::NAME,
			);

			// split all mandatory headers into batches
			let headers_batches =
				make_mandatory_headers_batches::<Finality, _>(headers_to_submit, |(_, proof)| {
					// we don't have an access to the Kusama/Polkadot chain runtimes here, so we'll
					// be using Millau weights. It isn't super-critical, unless real weights are
					// magnitude higher or so
					pallet_bridge_grandpa::weights::MillauWeight::<millau_runtime::Runtime>::submit_finality_proof(
						proof.commit.precommits.len().try_into().unwrap_or(u32::MAX),
						proof.votes_ancestries.len().try_into().unwrap_or(u32::MAX),
					)
				});
			log::info!(
				target: "bridge",
				"We're going to submit {} transactions to {} node",
				headers_batches.len(),
				Target::NAME,
			);

			// each batch is submitted as a separate transaction
			let signer_account_id: AccountIdOf<Target> = transaction_params.signer.public().into();
			let genesis_hash = *target_client.genesis_hash();
			let (spec_version, transaction_version) =
				target_client.simple_runtime_version().await?;
			let last_batch_index = headers_batches.len() - 1;
			for (i, headers_batch) in headers_batches.into_iter().enumerate() {
				let is_last_batch = i == last_batch_index;
				let expected_number =
					headers_batch.last().expect("all batches are non-empty").0.number();
				let transaction_params = transaction_params.clone();
				log::info!(
					target: "bridge",
					"Going to submit transaction that updates best {} header at {} to {}",
					Source::NAME,
					Target::NAME,
					expected_number,
				);

				// prepare `batch_all` call
				let mut batch_calls = Vec::with_capacity(headers_batch.len() + 2);
				// the first call is always resumes pallet operation
				batch_calls.push(set_pallet_operation_mode_call(true));
				// followed by submit-finality-proofs calls
				for header_and_proof in headers_batch {
					batch_calls.push(submit_finality_proof_call(header_and_proof));
				}
				// if it isn't the last batch, we shall halt pallet again
				if !is_last_batch {
					batch_calls.push(set_pallet_operation_mode_call(false));
				}
				let submit_batch_call = batch_all_call(batch_calls);

				let batch_transaction_events = target_client
					.submit_and_watch_signed_extrinsic(
						signer_account_id.clone(),
						move |best_block_id, transaction_nonce| {
							Ok(Bytes(
								Target::sign_transaction(SignParam {
									spec_version,
									transaction_version,
									genesis_hash,
									signer: transaction_params.signer.clone(),
									era: TransactionEra::new(
										best_block_id,
										transaction_params.mortality,
									),
									unsigned: UnsignedTransaction::new(
										submit_batch_call.into(),
										transaction_nonce,
									),
								})?
								.encode(),
							))
						},
					)
					.await?;
				wait_until_transaction_is_finalized::<Target>(batch_transaction_events).await?;

				// verify that the best finalized header at target has been updated
				let current_number =
					best_source_block_number_at_target::<Finality>(&target_client).await?;
				if current_number != expected_number {
					return Err(anyhow::format_err!(
						"Transaction has failed to update best {} header at {} to {}. It is {}",
						Source::NAME,
						Target::NAME,
						expected_number,
						current_number,
					))
				}

				// verify that the pallet is still halted (or operational if it is the last batch)
				ensure_pallet_operating_mode(&finality_target, is_last_batch).await?;
			}

			if let Some(latest_andatory_header_number) = latest_andatory_header_number {
				log::info!(
					target: "bridge",
					"Successfully updated best {} header at {} to {}. Pallet is now operational",
					Source::NAME,
					Target::NAME,
					latest_andatory_header_number,
				);
			}

			Ok(())
		})
	}
}

/// Mandatory header and its finality proof.
type HeaderAndProof<P> = (
	SyncHeader<HeaderOf<<P as SubstrateFinalitySyncPipeline>::SourceChain>>,
	GrandpaJustification<HeaderOf<<P as SubstrateFinalitySyncPipeline>::SourceChain>>,
);
/// Vector of mandatory headers and their finality proofs.
type HeadersAndProofs<P> = Vec<HeaderAndProof<P>>;

/// Returns best finalized source header number known to the bridge GRANDPA pallet at the target
/// chain.
///
/// This function works even if bridge GRANDPA pallet at the target chain is halted.
async fn best_source_block_number_at_target<P: SubstrateFinalitySyncPipeline>(
	target_client: &Client<P::TargetChain>,
) -> anyhow::Result<BlockNumberOf<P::SourceChain>> {
	Ok(read_client_state::<P::TargetChain, P::SourceChain>(
		target_client,
		None,
		P::SourceChain::BEST_FINALIZED_HEADER_ID_METHOD,
	)
	.await?
	.best_finalized_peer_at_best_self
	.0)
}

/// Verify that the bridge GRANDPA pallet at the target chain is either halted, or operational.
async fn ensure_pallet_operating_mode<P: SubstrateFinalitySyncPipeline>(
	finality_target: &SubstrateFinalityTarget<P>,
	operational: bool,
) -> anyhow::Result<()> {
	match (operational, finality_target.ensure_pallet_active().await) {
		(true, Ok(())) => Ok(()),
		(false, Err(SubstrateError::BridgePalletIsHalted)) => Ok(()),
		_ =>
			return Err(anyhow::format_err!(
				"Bridge GRANDPA pallet at {} is expected to be {}, but it isn't",
				P::TargetChain::NAME,
				if operational { "operational" } else { "halted" },
			)),
	}
}

/// Returns list of all mandatory headers in given range.
async fn find_mandatory_headers_in_range<P: SubstrateFinalitySyncPipeline>(
	finality_source: &SubstrateFinalitySource<P>,
	range: (BlockNumberOf<P::SourceChain>, BlockNumberOf<P::SourceChain>),
) -> anyhow::Result<HeadersAndProofs<P>> {
	let mut mandatory_headers = Vec::new();
	let mut current = range.0;
	while current <= range.1 {
		let (header, proof) = finality_source.header_and_finality_proof(current).await?;
		if header.is_mandatory() {
			match proof {
				Some(proof) => mandatory_headers.push((header, proof)),
				None =>
					return Err(anyhow::format_err!(
						"Missing GRANDPA justification for {} header {}",
						P::SourceChain::NAME,
						current,
					)),
			}
		}

		current += One::one();
	}

	Ok(mandatory_headers)
}

/// Given list of mandatory headers, prepare batches of headers, so that every batch may fit into
/// single transaction.
fn make_mandatory_headers_batches<
	P: SubstrateFinalitySyncPipeline,
	F: Fn(&HeaderAndProof<P>) -> Weight,
>(
	mut headers_to_submit: HeadersAndProofs<P>,
	submit_header_weight: F,
) -> Vec<HeadersAndProofs<P>> {
	// now that we have all mandatory headers, let's prepare transactions
	// (let's keep all our transactions below 2/3 of max tx size/weight to have some reserve
	// for utility overhead + for halting transaction)
	let maximal_tx_size = P::TargetChain::max_extrinsic_size() * 2 / 3;
	let maximal_tx_weight = P::TargetChain::max_extrinsic_weight() * 2 / 3;
	let mut current_batch_size: u32 = 0;
	let mut current_batch_weight: Weight = 0;
	let mut batches = Vec::new();
	let mut i = 0;
	while i < headers_to_submit.len() {
		let header_and_proof_size =
			headers_to_submit[i].0.encode().len() + headers_to_submit[i].1.encode().len();
		let header_and_proof_weight = submit_header_weight(&headers_to_submit[i]);

		let new_batch_size = current_batch_size
			.saturating_add(u32::try_from(header_and_proof_size).unwrap_or(u32::MAX));
		let new_batch_weight = current_batch_weight.saturating_add(header_and_proof_weight);

		let is_exceeding_tx_size = new_batch_size > maximal_tx_size;
		let is_exceeding_tx_weight = new_batch_weight > maximal_tx_weight;
		let is_new_batch_required = is_exceeding_tx_size || is_exceeding_tx_weight;

		if is_new_batch_required {
			// if `i` is 0 and we're here, it is a weird situation: even single header submission is
			// larger than we've planned for a bunch of headers. Let's be optimistic and hope that
			// the tx will still succeed.
			let spit_off_index = std::cmp::max(i, 1);
			let remaining_headers_to_submit = headers_to_submit.split_off(spit_off_index);
			batches.push(headers_to_submit);

			// we'll reiterate the same header again => so set `current_*` to zero
			current_batch_size = 0;
			current_batch_weight = 0;
			headers_to_submit = remaining_headers_to_submit;
			i = 0;
		} else {
			current_batch_size = new_batch_size;
			current_batch_weight = new_batch_weight;
			i += 1;
		}
	}
	if !headers_to_submit.is_empty() {
		batches.push(headers_to_submit);
	}
	batches
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::{RuntimeVersionType, SourceRuntimeVersionParams, TargetRuntimeVersionParams};
	use bp_test_utils::{make_default_justification, test_header};
	use relay_polkadot_client::Polkadot;
	use sp_runtime::{traits::Header as _, DigestItem};

	fn make_header_and_justification(
		i: u32,
		size: u32,
	) -> (SyncHeader<bp_kusama::Header>, GrandpaJustification<bp_kusama::Header>) {
		let size = size as usize;
		let mut header: bp_kusama::Header = test_header(i);
		let justification = make_default_justification(&header);
		let actual_size = header.encode().len() + justification.encode().len();
		// additional digest means some additional bytes, so let's decrease `additional_digest_size`
		// a bit
		let additional_digest_size = size.saturating_sub(actual_size).saturating_sub(100);
		header.digest_mut().push(DigestItem::Other(vec![0u8; additional_digest_size]));
		let justification = make_default_justification(&header);
		println!("{} {}", size, header.encode().len() + justification.encode().len());
		(header.into(), justification)
	}

	#[test]
	fn should_parse_cli_options() {
		// when
		let res = ReinitBridge::from_iter(vec![
			"reinit-bridge",
			"kusama-to-polkadot",
			"--source-host",
			"127.0.0.1",
			"--source-port",
			"42",
			"--target-host",
			"127.0.0.1",
			"--target-port",
			"43",
			"--target-signer",
			"//Alice",
		]);

		// then
		assert_eq!(
			res,
			ReinitBridge {
				bridge: ReinitBridgeName::KusamaToPolkadot,
				source: SourceConnectionParams {
					source_host: "127.0.0.1".into(),
					source_port: 42,
					source_secure: false,
					source_runtime_version: SourceRuntimeVersionParams {
						source_version_mode: RuntimeVersionType::Bundle,
						source_spec_version: None,
						source_transaction_version: None,
					}
				},
				target: TargetConnectionParams {
					target_host: "127.0.0.1".into(),
					target_port: 43,
					target_secure: false,
					target_runtime_version: TargetRuntimeVersionParams {
						target_version_mode: RuntimeVersionType::Bundle,
						target_spec_version: None,
						target_transaction_version: None,
					}
				},
				target_sign: TargetSigningParams {
					target_signer: Some("//Alice".into()),
					target_signer_password: None,
					target_signer_file: None,
					target_signer_password_file: None,
					target_transactions_mortality: None,
				},
			}
		);
	}

	#[test]
	fn make_mandatory_headers_batches_and_empty_headers() {
		let batches = make_mandatory_headers_batches::<KusamaFinalityToPolkadot, _>(vec![], |_| 0);
		assert!(batches.is_empty());
	}

	#[test]
	fn make_mandatory_headers_batches_with_single_batch() {
		let headers_to_submit =
			vec![make_header_and_justification(10, Polkadot::max_extrinsic_size() / 3)];
		let batches =
			make_mandatory_headers_batches::<KusamaFinalityToPolkadot, _>(headers_to_submit, |_| 0);
		assert_eq!(batches.into_iter().map(|x| x.len()).collect::<Vec<_>>(), vec![1],);
	}

	#[test]
	fn make_mandatory_headers_batches_group_by_size() {
		let headers_to_submit = vec![
			make_header_and_justification(10, Polkadot::max_extrinsic_size() / 3),
			make_header_and_justification(20, Polkadot::max_extrinsic_size() / 3),
			make_header_and_justification(30, Polkadot::max_extrinsic_size() * 2 / 3),
			make_header_and_justification(40, Polkadot::max_extrinsic_size()),
		];
		let batches =
			make_mandatory_headers_batches::<KusamaFinalityToPolkadot, _>(headers_to_submit, |_| 0);
		assert_eq!(batches.into_iter().map(|x| x.len()).collect::<Vec<_>>(), vec![2, 1, 1],);
	}

	#[test]
	fn make_mandatory_headers_batches_group_by_weight() {
		let headers_to_submit = vec![
			make_header_and_justification(10, 0),
			make_header_and_justification(20, 0),
			make_header_and_justification(30, 0),
			make_header_and_justification(40, 0),
		];
		let batches = make_mandatory_headers_batches::<KusamaFinalityToPolkadot, _>(
			headers_to_submit,
			|(header, _)| {
				if header.number() == 10 || header.number() == 20 {
					Polkadot::max_extrinsic_weight() / 3
				} else if header.number() == 30 {
					Polkadot::max_extrinsic_weight() * 2 / 3
				} else {
					Polkadot::max_extrinsic_weight()
				}
			},
		);
		assert_eq!(batches.into_iter().map(|x| x.len()).collect::<Vec<_>>(), vec![2, 1, 1],);
	}
}
