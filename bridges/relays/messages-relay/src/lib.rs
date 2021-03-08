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

//! Relaying [`message-lane`](../pallet_message_lane/index.html) application specific
//! data. Message lane allows sending arbitrary messages between bridged chains. This
//! module provides entrypoint that starts reading messages from given message lane
//! of source chain and submits proof-of-message-at-source-chain transactions to the
//! target chain. Additionaly, proofs-of-messages-delivery are sent back from the
//! target chain to the source chain.

// required for futures::select!
#![recursion_limit = "1024"]
#![warn(missing_docs)]

mod metrics;

pub mod message_lane;
pub mod message_lane_loop;

mod message_race_delivery;
mod message_race_loop;
mod message_race_receiving;
mod message_race_strategy;
