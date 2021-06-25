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

//! Subsystem accumulation.
//!
//! Node-side types and generated overseer.

#![warn(missing_docs)]

use polkadot_node_subsystem_types::messages::AvailabilityStoreMessage;
pub use polkadot_node_subsystem_types::{errors, messages};

pub use polkadot_node_jaeger as jaeger;
pub use jaeger::*;


pub use polkadot_node_subsystem_types::{
	LeafStatus,
	ActivatedLeaf,
	ActiveLeavesUpdate,
	OverseerSignal,
};

pub use polkadot_overseer::{
	AllMessages,
	Overseer,
	SubsystemError,
	gen::{
		OverseerError,
		SubsystemContext,
		SubsystemMeters,
		SubsystemMeterReadouts,
		TimeoutExt,
	},
};

// Simplify usage without having to do large scale modifications of all
// subsystems at once.
pub type SubsystemSender = polkadot_overseer::gen::SubsystemSender<AllMessages>;

pub type FromOverseer<M> = polkadot_overseer::gen::FromOverseer<M, OverseerSignal>;

pub type Subsystem<Ctx> = polkadot_overseer::gen::Subsystem<Ctx, SubsystemError>;

pub type SubsystemInstance<Message> = polkadot_overseer::gen::SubsystemInstance<Message, OverseerSignal>;
