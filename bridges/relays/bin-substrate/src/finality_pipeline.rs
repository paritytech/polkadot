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

//! Substrate-to-Substrate headers sync entrypoint.

use crate::finality_target::SubstrateFinalityTarget;

use bp_header_chain::justification::GrandpaJustification;
use finality_relay::{FinalitySyncParams, FinalitySyncPipeline};
use relay_substrate_client::{finality_source::FinalitySource, BlockNumberOf, Chain, Client, HashOf, SyncHeader};
use relay_utils::{metrics::MetricsParams, BlockNumberBase};
use sp_core::Bytes;
use std::{fmt::Debug, marker::PhantomData, time::Duration};

/// Default synchronization loop timeout.
const STALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Default limit of recent finality proofs.
///
/// Finality delay of 4096 blocks is unlikely to happen in practice in
/// Substrate+GRANDPA based chains (good to know).
const RECENT_FINALITY_PROOFS_LIMIT: usize = 4096;

/// Headers sync pipeline for Substrate <-> Substrate relays.
pub trait SubstrateFinalitySyncPipeline: FinalitySyncPipeline {
	/// Name of the runtime method that returns id of best finalized source header at target chain.
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str;

	/// Chain with GRANDPA bridge pallet.
	type TargetChain: Chain;

	/// Customize metrics exposed by headers sync loop.
	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		Ok(params)
	}

	/// Returns id of account that we're using to sign transactions at target chain.
	fn transactions_author(&self) -> <Self::TargetChain as Chain>::AccountId;

	/// Make submit header transaction.
	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Self::TargetChain as Chain>::Index,
		header: Self::Header,
		proof: Self::FinalityProof,
	) -> Bytes;
}

/// Substrate-to-Substrate finality proof pipeline.
#[derive(Clone)]
pub struct SubstrateFinalityToSubstrate<SourceChain, TargetChain: Chain, TargetSign> {
	/// Client for the target chain.
	pub(crate) target_client: Client<TargetChain>,
	/// Data required to sign target chain transactions.
	pub(crate) target_sign: TargetSign,
	/// Unused generic arguments dump.
	_marker: PhantomData<SourceChain>,
}

impl<SourceChain, TargetChain: Chain, TargetSign> Debug
	for SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>
{
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		f.debug_struct("SubstrateFinalityToSubstrate")
			.field("target_client", &self.target_client)
			.finish()
	}
}

impl<SourceChain, TargetChain: Chain, TargetSign> SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign> {
	/// Create new Substrate-to-Substrate headers pipeline.
	pub fn new(target_client: Client<TargetChain>, target_sign: TargetSign) -> Self {
		SubstrateFinalityToSubstrate {
			target_client,
			target_sign,
			_marker: Default::default(),
		}
	}
}

impl<SourceChain, TargetChain, TargetSign> FinalitySyncPipeline
	for SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>
where
	SourceChain: Clone + Chain + Debug,
	BlockNumberOf<SourceChain>: BlockNumberBase,
	TargetChain: Clone + Chain + Debug,
	TargetSign: 'static + Clone + Send + Sync,
{
	const SOURCE_NAME: &'static str = SourceChain::NAME;
	const TARGET_NAME: &'static str = TargetChain::NAME;

	type Hash = HashOf<SourceChain>;
	type Number = BlockNumberOf<SourceChain>;
	type Header = SyncHeader<SourceChain::Header>;
	type FinalityProof = GrandpaJustification<SourceChain::Header>;
}

/// Run Substrate-to-Substrate finality sync.
pub async fn run<SourceChain, TargetChain, P>(
	pipeline: P,
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	is_on_demand_task: bool,
	metrics_params: MetricsParams,
) -> anyhow::Result<()>
where
	P: SubstrateFinalitySyncPipeline<
		Hash = HashOf<SourceChain>,
		Number = BlockNumberOf<SourceChain>,
		Header = SyncHeader<SourceChain::Header>,
		FinalityProof = GrandpaJustification<SourceChain::Header>,
		TargetChain = TargetChain,
	>,
	SourceChain: Clone + Chain,
	BlockNumberOf<SourceChain>: BlockNumberBase,
	TargetChain: Clone + Chain,
{
	log::info!(
		target: "bridge",
		"Starting {} -> {} finality proof relay",
		SourceChain::NAME,
		TargetChain::NAME,
	);

	finality_relay::run(
		FinalitySource::new(source_client),
		SubstrateFinalityTarget::new(target_client, pipeline),
		FinalitySyncParams {
			is_on_demand_task,
			tick: std::cmp::max(SourceChain::AVERAGE_BLOCK_INTERVAL, TargetChain::AVERAGE_BLOCK_INTERVAL),
			recent_finality_proofs_limit: RECENT_FINALITY_PROOFS_LIMIT,
			stall_timeout: STALL_TIMEOUT,
		},
		metrics_params,
		futures::future::pending(),
	)
	.await
	.map_err(|e| anyhow::format_err!("{}", e))
}
