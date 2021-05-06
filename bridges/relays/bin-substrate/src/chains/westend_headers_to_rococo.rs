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

//! Westend-to-Rococo headers sync entrypoint.

use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use relay_rococo_client::{Rococo, SigningParams as RococoSigningParams};
use relay_substrate_client::{Chain, TransactionSignScheme};
use relay_utils::metrics::MetricsParams;
use relay_westend_client::{SyncHeader as WestendSyncHeader, Westend};
use sp_core::{Bytes, Pair};

/// Westend-to-Rococo finality sync pipeline.
pub(crate) type WestendFinalityToRococo = SubstrateFinalityToSubstrate<Westend, Rococo, RococoSigningParams>;

impl SubstrateFinalitySyncPipeline for WestendFinalityToRococo {
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_westend::BEST_FINALIZED_WESTEND_HEADER_METHOD;

	type TargetChain = Rococo;

	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		crate::chains::add_polkadot_kusama_price_metrics::<Self>(params)
	}

	fn transactions_author(&self) -> bp_rococo::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Rococo as Chain>::Index,
		header: WestendSyncHeader,
		proof: GrandpaJustification<bp_westend::Header>,
	) -> Bytes {
		let call = bp_rococo::Call::BridgeGrandpaWestend(bp_rococo::BridgeGrandpaCall::submit_finality_proof(
			header.into_inner(),
			proof,
		));
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Rococo::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);

		Bytes(transaction.encode())
	}
}
