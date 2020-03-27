// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot-specific network implementation.
//!
//! This manages routing for parachain statements, parachain block and outgoing message
//! data fetching, communication between collators and validators, and more.

pub mod collator_pool;
pub mod local_collations;

use codec::Decode;
use futures::prelude::*;
use polkadot_primitives::Hash;
use sc_network::PeerId;
use sc_network_gossip::TopicNotification;
use log::debug;

use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

pub use gossip::{GossipService, GossipMessage};

pub mod gossip {
    pub use polkadot_gossip_primitives::*;
    pub use av_store::networking::*;
}
