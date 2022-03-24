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

//! Types and functions intended to ease adding of new Substrate -> Substrate
//! finality proofs synchronization pipelines.

use crate::{
	finality_source::SubstrateFinalitySource, finality_target::SubstrateFinalityTarget,
	TransactionParams,
};

use async_trait::async_trait;
use bp_header_chain::justification::GrandpaJustification;
use finality_relay::FinalitySyncPipeline;
use pallet_bridge_grandpa::{Call as BridgeGrandpaCall, Config as BridgeGrandpaConfig};
use relay_substrate_client::{
	transaction_stall_timeout, AccountIdOf, AccountKeyPairOf, BlockNumberOf, CallOf, Chain,
	ChainWithGrandpa, Client, HashOf, HeaderOf, SyncHeader, TransactionSignScheme,
};
use relay_utils::metrics::MetricsParams;
use sp_core::Pair;
use std::{fmt::Debug, marker::PhantomData};

/// Default limit of recent finality proofs.
///
/// Finality delay of 4096 blocks is unlikely to happen in practice in
/// Substrate+GRANDPA based chains (good to know).
pub(crate) const RECENT_FINALITY_PROOFS_LIMIT: usize = 4096;

/// Substrate -> Substrate finality proofs synchronization pipeline.
#[async_trait]
pub trait SubstrateFinalitySyncPipeline: 'static + Clone + Debug + Send + Sync {
	/// Headers of this chain are submitted to the `TargetChain`.
	type SourceChain: ChainWithGrandpa;
	/// Headers of the `SourceChain` are submitted to this chain.
	type TargetChain: Chain;

	/// How submit finality proof call is built?
	type SubmitFinalityProofCallBuilder: SubmitFinalityProofCallBuilder<Self>;
	/// Scheme used to sign target chain transactions.
	type TransactionSignScheme: TransactionSignScheme;

	/// Add relay guards if required.
	async fn start_relay_guards(
		_target_client: &Client<Self::TargetChain>,
		_transaction_params: &TransactionParams<AccountKeyPairOf<Self::TransactionSignScheme>>,
		_enable_version_guard: bool,
	) -> relay_substrate_client::Result<()> {
		Ok(())
	}
}

/// Adapter that allows all `SubstrateFinalitySyncPipeline` to act as `FinalitySyncPipeline`.
#[derive(Clone, Debug)]
pub struct FinalitySyncPipelineAdapter<P: SubstrateFinalitySyncPipeline> {
	_phantom: PhantomData<P>,
}

impl<P: SubstrateFinalitySyncPipeline> FinalitySyncPipeline for FinalitySyncPipelineAdapter<P> {
	const SOURCE_NAME: &'static str = P::SourceChain::NAME;
	const TARGET_NAME: &'static str = P::TargetChain::NAME;

	type Hash = HashOf<P::SourceChain>;
	type Number = BlockNumberOf<P::SourceChain>;
	type Header = relay_substrate_client::SyncHeader<HeaderOf<P::SourceChain>>;
	type FinalityProof = GrandpaJustification<HeaderOf<P::SourceChain>>;
}

/// Different ways of building `submit_finality_proof` calls.
pub trait SubmitFinalityProofCallBuilder<P: SubstrateFinalitySyncPipeline> {
	/// Given source chain header and its finality proofs, build call of `submit_finality_proof`
	/// function of bridge GRANDPA module at the target chain.
	fn build_submit_finality_proof_call(
		header: SyncHeader<HeaderOf<P::SourceChain>>,
		proof: GrandpaJustification<HeaderOf<P::SourceChain>>,
	) -> CallOf<P::TargetChain>;
}

/// Building `submit_finality_proof` call when you have direct access to the target
/// chain runtime.
pub struct DirectSubmitFinalityProofCallBuilder<P, R, I> {
	_phantom: PhantomData<(P, R, I)>,
}

impl<P, R, I> SubmitFinalityProofCallBuilder<P> for DirectSubmitFinalityProofCallBuilder<P, R, I>
where
	P: SubstrateFinalitySyncPipeline,
	R: BridgeGrandpaConfig<I>,
	I: 'static,
	R::BridgedChain: bp_runtime::Chain<Header = HeaderOf<P::SourceChain>>,
	CallOf<P::TargetChain>: From<BridgeGrandpaCall<R, I>>,
{
	fn build_submit_finality_proof_call(
		header: SyncHeader<HeaderOf<P::SourceChain>>,
		proof: GrandpaJustification<HeaderOf<P::SourceChain>>,
	) -> CallOf<P::TargetChain> {
		BridgeGrandpaCall::<R, I>::submit_finality_proof {
			finality_target: Box::new(header.into_inner()),
			justification: proof,
		}
		.into()
	}
}

/// Macro that generates `SubmitFinalityProofCallBuilder` implementation for the case when
/// you only have an access to the mocked version of target chain runtime. In this case you
/// should provide "name" of the call variant for the bridge GRANDPA calls and the "name" of
/// the variant for the `submit_finality_proof` call within that first option.
#[rustfmt::skip]
#[macro_export]
macro_rules! generate_mocked_submit_finality_proof_call_builder {
	($pipeline:ident, $mocked_builder:ident, $bridge_grandpa:path, $submit_finality_proof:path) => {
		pub struct $mocked_builder;

		impl $crate::finality_pipeline::SubmitFinalityProofCallBuilder<$pipeline>
			for $mocked_builder
		{
			fn build_submit_finality_proof_call(
				header: relay_substrate_client::SyncHeader<
					relay_substrate_client::HeaderOf<
						<$pipeline as $crate::finality_pipeline::SubstrateFinalitySyncPipeline>::SourceChain
					>
				>,
				proof: bp_header_chain::justification::GrandpaJustification<
					relay_substrate_client::HeaderOf<
						<$pipeline as $crate::finality_pipeline::SubstrateFinalitySyncPipeline>::SourceChain
					>
				>,
			) -> relay_substrate_client::CallOf<
				<$pipeline as $crate::finality_pipeline::SubstrateFinalitySyncPipeline>::TargetChain
			> {
				$bridge_grandpa($submit_finality_proof(Box::new(header.into_inner()), proof))
			}
		}
	};
}

/// Run Substrate-to-Substrate finality sync loop.
pub async fn run<P: SubstrateFinalitySyncPipeline>(
	source_client: Client<P::SourceChain>,
	target_client: Client<P::TargetChain>,
	only_mandatory_headers: bool,
	transaction_params: TransactionParams<AccountKeyPairOf<P::TransactionSignScheme>>,
	metrics_params: MetricsParams,
) -> anyhow::Result<()>
where
	AccountIdOf<P::TargetChain>: From<<AccountKeyPairOf<P::TransactionSignScheme> as Pair>::Public>,
	P::TransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
{
	log::info!(
		target: "bridge",
		"Starting {} -> {} finality proof relay",
		P::SourceChain::NAME,
		P::TargetChain::NAME,
	);

	finality_relay::run(
		SubstrateFinalitySource::<P>::new(source_client, None),
		SubstrateFinalityTarget::<P>::new(target_client, transaction_params.clone()),
		finality_relay::FinalitySyncParams {
			tick: std::cmp::max(
				P::SourceChain::AVERAGE_BLOCK_INTERVAL,
				P::TargetChain::AVERAGE_BLOCK_INTERVAL,
			),
			recent_finality_proofs_limit: RECENT_FINALITY_PROOFS_LIMIT,
			stall_timeout: transaction_stall_timeout(
				transaction_params.mortality,
				P::TargetChain::AVERAGE_BLOCK_INTERVAL,
				crate::STALL_TIMEOUT,
			),
			only_mandatory_headers,
		},
		metrics_params,
		futures::future::pending(),
	)
	.await
	.map_err(|e| anyhow::format_err!("{}", e))
}
