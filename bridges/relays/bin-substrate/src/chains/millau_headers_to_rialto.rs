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

//! Millau-to-Rialto headers sync entrypoint.

use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use relay_millau_client::{Millau, SyncHeader as MillauSyncHeader};
use relay_rialto_client::{Rialto, SigningParams as RialtoSigningParams};
use relay_substrate_client::{Chain, TransactionSignScheme};
use sp_core::{Bytes, Pair};

/// Millau-to-Rialto finality sync pipeline.
pub(crate) type MillauFinalityToRialto = SubstrateFinalityToSubstrate<Millau, Rialto, RialtoSigningParams>;

impl SubstrateFinalitySyncPipeline for MillauFinalityToRialto {
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_millau::BEST_FINALIZED_MILLAU_HEADER_METHOD;

	type TargetChain = Rialto;

	fn transactions_author(&self) -> bp_rialto::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_submit_finality_proof_transaction(
		&self,
		transaction_nonce: <Rialto as Chain>::Index,
		header: MillauSyncHeader,
		proof: GrandpaJustification<bp_millau::Header>,
	) -> Bytes {
		let call = rialto_runtime::BridgeGrandpaMillauCall::submit_finality_proof(header.into_inner(), proof).into();

		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Rialto::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);

		Bytes(transaction.encode())
	}
}
