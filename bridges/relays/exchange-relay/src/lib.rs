// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Relaying [`currency-exchange`](../pallet_bridge_currency_exchange/index.html) application
//! specific data. Currency exchange application allows exchanging tokens between bridged chains.
//! This module provides entrypoints for crafting and submitting (single and multiple)
//! proof-of-exchange-at-source-chain transaction(s) to target chain.

#![warn(missing_docs)]

pub mod exchange;
pub mod exchange_loop;
pub mod exchange_loop_metrics;
