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

use async_trait::async_trait;
use relay_wococo_client::Wococo;
use substrate_relay_helper::{finality_pipeline::SubstrateFinalitySyncPipeline, TransactionParams};

/// Description of Rococo -> Wococo finalized headers bridge.
#[derive(Clone, Debug)]
pub struct RococoFinalityToWococo;
substrate_relay_helper::generate_mocked_submit_finality_proof_call_builder!(
	RococoFinalityToWococo,
	RococoFinalityToWococoCallBuilder,
	relay_wococo_client::runtime::Call::BridgeGrandpaRococo,
	relay_wococo_client::runtime::BridgeGrandpaRococoCall::submit_finality_proof
);

#[async_trait]
impl SubstrateFinalitySyncPipeline for RococoFinalityToWococo {
	type SourceChain = relay_rococo_client::Rococo;
	type TargetChain = Wococo;

	type SubmitFinalityProofCallBuilder = RococoFinalityToWococoCallBuilder;
	type TransactionSignScheme = Wococo;

	async fn start_relay_guards(
		target_client: &relay_substrate_client::Client<Wococo>,
		transaction_params: &TransactionParams<sp_core::sr25519::Pair>,
		enable_version_guard: bool,
	) -> relay_substrate_client::Result<()> {
		substrate_relay_helper::finality_guards::start::<Wococo, Wococo>(
			target_client,
			transaction_params,
			enable_version_guard,
			MAXIMAL_BALANCE_DECREASE_PER_DAY,
		)
		.await
	}
}
