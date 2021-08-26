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

//! Rococo-to-Wococo headers sync entrypoint.

use crate::chains::wococo_headers_to_rococo::MAXIMAL_BALANCE_DECREASE_PER_DAY;
use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use relay_rococo_client::{Rococo, SyncHeader as RococoSyncHeader};
use relay_substrate_client::{Chain, TransactionSignScheme};
use relay_utils::metrics::MetricsParams;
use relay_wococo_client::{SigningParams as WococoSigningParams, Wococo};
use sp_core::{Bytes, Pair};

/// Rococo-to-Wococo finality sync pipeline.
pub(crate) type RococoFinalityToWococo = SubstrateFinalityToSubstrate<Rococo, Wococo, WococoSigningParams>;

impl SubstrateFinalitySyncPipeline for RococoFinalityToWococo {
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_rococo::BEST_FINALIZED_ROCOCO_HEADER_METHOD;

	type TargetChain = Wococo;

	fn customize_metrics(params: MetricsParams) -> anyhow::Result<MetricsParams> {
		crate::chains::add_polkadot_kusama_price_metrics::<Self>(params)
	}

	fn start_relay_guards(&self) {
		relay_substrate_client::guard::abort_on_spec_version_change(
			self.target_client.clone(),
			bp_wococo::VERSION.spec_version,
		);
		relay_substrate_client::guard::abort_when_account_balance_decreased(
			self.target_client.clone(),
			self.transactions_author(),
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		);
	}

	fn transactions_author(&self) -> bp_wococo::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Wococo as Chain>::Index,
		header: RococoSyncHeader,
		proof: GrandpaJustification<bp_rococo::Header>,
	) -> Bytes {
		let call = relay_wococo_client::runtime::Call::BridgeGrandpaRococo(
			relay_wococo_client::runtime::BridgeGrandpaRococoCall::submit_finality_proof(header.into_inner(), proof),
		);
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Wococo::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);

		Bytes(transaction.encode())
	}
}
