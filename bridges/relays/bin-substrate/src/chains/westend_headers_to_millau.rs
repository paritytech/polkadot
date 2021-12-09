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

//! Westend-to-Millau headers sync entrypoint.

use codec::Encode;
use sp_core::{Bytes, Pair};

use bp_header_chain::justification::GrandpaJustification;
use relay_millau_client::{Millau, SigningParams as MillauSigningParams};
use relay_substrate_client::{Client, IndexOf, TransactionSignScheme, UnsignedTransaction};
use relay_utils::metrics::MetricsParams;
use relay_westend_client::{SyncHeader as WestendSyncHeader, Westend};
use substrate_relay_helper::finality_pipeline::{
	SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate,
};

/// Westend-to-Millau finality sync pipeline.
pub(crate) type FinalityPipelineWestendFinalityToMillau =
	SubstrateFinalityToSubstrate<Westend, Millau, MillauSigningParams>;

#[derive(Clone, Debug)]
pub(crate) struct WestendFinalityToMillau {
	finality_pipeline: FinalityPipelineWestendFinalityToMillau,
}

impl WestendFinalityToMillau {
	pub fn new(target_client: Client<Millau>, target_sign: MillauSigningParams) -> Self {
		Self {
			finality_pipeline: FinalityPipelineWestendFinalityToMillau::new(
				target_client,
				target_sign,
			),
		}
	}
}

impl SubstrateFinalitySyncPipeline for WestendFinalityToMillau {
	type FinalitySyncPipeline = FinalityPipelineWestendFinalityToMillau;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str =
		bp_westend::BEST_FINALIZED_WESTEND_HEADER_METHOD;

	type TargetChain = Millau;

	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		crate::chains::add_polkadot_kusama_price_metrics::<Self::FinalitySyncPipeline>(params)
	}

	fn transactions_author(&self) -> bp_millau::AccountId {
		(*self.finality_pipeline.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		era: bp_runtime::TransactionEraOf<Millau>,
		transaction_nonce: IndexOf<Millau>,
		header: WestendSyncHeader,
		proof: GrandpaJustification<bp_westend::Header>,
	) -> Bytes {
		let call = millau_runtime::BridgeGrandpaCall::<
			millau_runtime::Runtime,
			millau_runtime::WestendGrandpaInstance,
		>::submit_finality_proof {
			finality_target: Box::new(header.into_inner()),
			justification: proof,
		}
		.into();

		let genesis_hash = *self.finality_pipeline.target_client.genesis_hash();
		let transaction = Millau::sign_transaction(
			genesis_hash,
			&self.finality_pipeline.target_sign,
			era,
			UnsignedTransaction::new(call, transaction_nonce),
		);

		Bytes(transaction.encode())
	}
}
