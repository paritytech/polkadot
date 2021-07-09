// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.
//! Attempts to upgrade the polkadot runtime, in a simnet environment
use std::{error::Error, str::FromStr};

use polkadot_simnet::{run, dispatch_with_root};
use sp_core::crypto::AccountId32;

fn main() -> Result<(), Box<dyn Error>> {
    run(|node| async {
         let wasm_binary = polkadot_runtime::WASM_BINARY
             .ok_or("Polkadot development wasm not available")?
             .to_vec();
         // start runtime upgrade
         dispatch_with_root(system::Call::set_code(wasm_binary).into(), &node).await?;

         let (from, dest, balance) = (
             AccountId32::from_str("15j4dg5GzsL1bw2U2AWgeyAk6QTxq43V7ZPbXdAmbVLjvDCK")?,
             AccountId32::from_str("1rvXMZpAj9nKLQkPFCymyH7Fg3ZyKJhJbrc7UtHbTVhJm1A")?,
             10_000_000_000_000 // 10 dots
         );

         // post upgrade tests, a simple balance transfer
         node.submit_extrinsic(balances::Call::transfer(dest.into(), balance), from).await?;

         // done upgrading runtime, drop node.
         drop(node);

         Ok(())
    })?;

    Ok(())
}
