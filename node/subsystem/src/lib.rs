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

use overseer::Overseer;
pub use polkadot_node_jaeger as jaeger;
pub use jaeger::*;

use polkadot_node_subsystem_types::errors::*;
pub use polkadot_overseer::{OverseerSignal, ActiveLeavesUpdate, self as overseer};

pub use polkadot_node_subsystem_types::{
	errors::{self, *},
	ActivatedLeaf,
	LeafStatus,
};

pub mod messages {
	pub use super::overseer::AllMessages;
	pub use polkadot_node_subsystem_types::messages::*;
}


/// A `Result` type that wraps [`SubsystemError`].
///
/// [`SubsystemError`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;

// Simplify usage without having to do large scale modifications of all
// subsystems at once.

pub type FromOverseer<M> = polkadot_overseer::gen::FromOverseer<M, OverseerSignal>;


pub type SubsystemInstance<Message> = polkadot_overseer::gen::SubsystemInstance<Message, OverseerSignal>;

// Same for traits
pub trait SubsystemSender: polkadot_overseer::gen::SubsystemSender<messages::AllMessages> {
}

impl<T> SubsystemSender for T where T: polkadot_overseer::gen::SubsystemSender<messages::AllMessages> {
}

// pub use polkadot_overseer::gen::SubsystemSender;

pub type SpawnedSubsystem = polkadot_overseer::gen::SpawnedSubsystem<SubsystemError>;


pub trait SubsystemContext: polkadot_overseer::gen::SubsystemContext<
	Signal=OverseerSignal,
	AllMessages=messages::AllMessages,
	Error=SubsystemError,
>
{
	type Message: std::fmt::Debug + Send + 'static;
	type Sender: SubsystemSender + Send + Clone + 'static;
}

impl<T> SubsystemContext for T
where
	T: polkadot_overseer::gen::SubsystemContext<
		Signal=OverseerSignal,
		AllMessages=messages::AllMessages,
		Error=SubsystemError,
	>,
{
	type Message = <Self as polkadot_overseer::gen::SubsystemContext>::Message;
	type Sender = <Self as polkadot_overseer::gen::SubsystemContext>::Sender;
}
