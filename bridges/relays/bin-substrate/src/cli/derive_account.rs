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

use crate::cli::{bridge::FullBridge, AccountId};
use crate::select_full_bridge;
use relay_substrate_client::Chain;
use structopt::StructOpt;

/// Given a source chain `AccountId`, derive the corresponding `AccountId` for the target chain.
///
/// The (derived) target chain `AccountId` is going to be used as dispatch origin of the call
/// that has been sent over the bridge.
/// This account can also be used to receive target-chain funds (or other form of ownership),
/// since messages sent over the bridge will be able to spend these.
#[derive(StructOpt)]
pub struct DeriveAccount {
	/// A bridge instance to initalize.
	#[structopt(possible_values = &FullBridge::variants(), case_insensitive = true)]
	bridge: FullBridge,
	/// Source-chain address to derive Target-chain address from.
	account: AccountId,
}

impl DeriveAccount {
	/// Parse CLI arguments and derive account.
	///
	/// Returns both the Source account in correct SS58 format and the derived account.
	fn derive_account(&self) -> (AccountId, AccountId) {
		select_full_bridge!(self.bridge, {
			let mut account = self.account.clone();
			account.enforce_chain::<Source>();
			let acc = bp_runtime::SourceAccount::Account(account.raw_id());
			let id = derive_account(acc);
			let derived_account = AccountId::from_raw::<Target>(id);
			(account, derived_account)
		})
	}

	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_full_bridge!(self.bridge, {
			let (account, derived_account) = self.derive_account();
			println!("Source address:\n{} ({})", account, Source::NAME);
			println!(
				"->Corresponding (derived) address:\n{} ({})",
				derived_account,
				Target::NAME,
			);

			Ok(())
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn derive_account_cli(bridge: &str, account: &str) -> (AccountId, AccountId) {
		DeriveAccount::from_iter(vec!["derive-account", bridge, account]).derive_account()
	}

	#[test]
	fn should_derive_accounts_correctly() {
		// given
		let rialto = "5sauUXUfPjmwxSgmb3tZ5d6yx24eZX4wWJ2JtVUBaQqFbvEU";
		let millau = "752paRyW1EGfq9YLTSSqcSJ5hqnBDidBmaftGhBo8fy6ypW9";

		// when
		let (rialto_parsed, rialto_derived) = derive_account_cli("RialtoToMillau", rialto);
		let (millau_parsed, millau_derived) = derive_account_cli("MillauToRialto", millau);
		let (millau2_parsed, millau2_derived) = derive_account_cli("MillauToRialto", rialto);

		// then
		assert_eq!(format!("{}", rialto_parsed), rialto);
		assert_eq!(format!("{}", millau_parsed), millau);
		assert_eq!(format!("{}", millau2_parsed), millau);

		assert_eq!(
			format!("{}", rialto_derived),
			"73gLnUwrAdH4vMjbXCiNEpgyz1PLk9JxCaY4cKzvfSZT73KE"
		);
		assert_eq!(
			format!("{}", millau_derived),
			"5rpTJqGv1BPAYy2sXzkPpc3Wx1ZpQtgfuBsrDpNV4HsXAmbi"
		);
		assert_eq!(millau_derived, millau2_derived);
	}
}
