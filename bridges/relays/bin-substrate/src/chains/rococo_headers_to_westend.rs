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

//! Rococo-to-Westend headers sync entrypoint.

use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use relay_rococo_client::{Rococo, SyncHeader as RococoSyncHeader};
use relay_substrate_client::{Chain, TransactionSignScheme};
use relay_utils::metrics::MetricsParams;
use relay_westend_client::{SigningParams as WestendSigningParams, Westend};
use sp_core::{Bytes, Pair};

/// Rococo-to-Westend finality sync pipeline.
pub(crate) type RococoFinalityToWestend = SubstrateFinalityToSubstrate<Rococo, Westend, WestendSigningParams>;

impl SubstrateFinalitySyncPipeline for RococoFinalityToWestend {
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_rococo::BEST_FINALIZED_ROCOCO_HEADER_METHOD;

	type TargetChain = Westend;

	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		crate::chains::add_polkadot_kusama_price_metrics::<Self>(params)
	}

	fn transactions_author(&self) -> bp_westend::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Westend as Chain>::Index,
		header: RococoSyncHeader,
		proof: GrandpaJustification<bp_rococo::Header>,
	) -> Bytes {
		let call = bp_westend::Call::BridgeGrandpaRococo(bp_westend::BridgeGrandpaRococoCall::submit_finality_proof(
			header.into_inner(),
			proof,
		));
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Westend::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);

		Bytes(transaction.encode())
	}
}
