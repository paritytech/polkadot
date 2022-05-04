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

//! Tools for starting guards of finality relays.

use crate::TransactionParams;

use relay_substrate_client::{
	AccountIdOf, AccountKeyPairOf, ChainWithBalances, TransactionSignScheme,
};
use sp_core::Pair;

/// Start finality relay guards.
pub async fn start<C: ChainWithBalances, S: TransactionSignScheme<Chain = C>>(
	target_client: &relay_substrate_client::Client<C>,
	transaction_params: &TransactionParams<S::AccountKeyPair>,
	enable_version_guard: bool,
	maximal_balance_decrease_per_day: C::Balance,
) -> relay_substrate_client::Result<()>
where
	AccountIdOf<C>: From<<AccountKeyPairOf<S> as Pair>::Public>,
{
	if enable_version_guard {
		relay_substrate_client::guard::abort_on_spec_version_change(
			target_client.clone(),
			target_client.simple_runtime_version().await?.0,
		);
	}
	relay_substrate_client::guard::abort_when_account_balance_decreased(
		target_client.clone(),
		transaction_params.signer.public().into(),
		maximal_balance_decrease_per_day,
	);
	Ok(())
}
