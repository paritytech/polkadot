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

pub use polkadot_node_jaeger as jaeger;
pub use jaeger::*;

use std::sync::Arc;
use std::fmt;
use smallvec::SmallVec;
use futures::prelude::*;
use futures::channel::{oneshot, mpsc};
use futures::future::BoxFuture;
use polkadot_node_subsystem_types::errors::*;
pub use polkadot_overseer::{OverseerSignal, ActiveLeavesUpdate};
use polkadot_primitives::v1::{Hash, BlockNumber};
/// How many slots are stack-reserved for active leaves updates
///
/// If there are fewer than this number of slots, then we've wasted some stack space.
/// If there are greater than this number of slots, then we fall back to a heap vector.
const ACTIVE_LEAVES_SMALLVEC_CAPACITY: usize = 8;

pub use polkadot_node_subsystem_types::errors::{self, *};
pub mod messages {
	pub use polkadot_overseer::AllMessages;
	pub use polkadot_node_subsystem_types::messages::*;
}

/// The status of an activated leaf.
#[derive(Debug, Clone)]
pub enum LeafStatus {
	/// A leaf is fresh when it's the first time the leaf has been encountered.
	/// Most leaves should be fresh.
	Fresh,
	/// A leaf is stale when it's encountered for a subsequent time. This will happen
	/// when the chain is reverted or the fork-choice rule abandons some chain.
	Stale,
}

impl LeafStatus {
	/// Returns a bool indicating fresh status.
	pub fn is_fresh(&self) -> bool {
		match *self {
			LeafStatus::Fresh => true,
			LeafStatus::Stale => false,
		}
	}

	/// Returns a bool indicating stale status.
	pub fn is_stale(&self) -> bool {
		match *self {
			LeafStatus::Fresh => false,
			LeafStatus::Stale => true,
		}
	}
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
pub trait SubsystemSender: polkadot_overseer::gen::SubsystemSender<messages::AllMessages> {}

impl<T> SubsystemSender for T where T: polkadot_overseer::gen::SubsystemSender<messages::AllMessages> {
}

// pub use polkadot_overseer::gen::SubsystemSender;

pub type SpawnedSubsystem = polkadot_overseer::gen::SpawnedSubsystem<SubsystemError>;
pub trait Subsystem<Ctx> : polkadot_overseer::gen::Subsystem<Ctx, SubsystemError>
where
	Ctx: SubsystemContext,
{
	fn start(self, ctx: Ctx) -> SpawnedSubsystem;
}

impl<Ctx, T> Subsystem<Ctx> for T
where
	T: polkadot_overseer::gen::Subsystem<Ctx, SubsystemError>,
	Ctx: SubsystemContext,
{
	fn start(self, ctx: Ctx) -> SpawnedSubsystem {
		<Self as polkadot_overseer::gen::Subsystem<
			Ctx,SubsystemError,
		>>::start(self, ctx)
	}
}

pub trait SubsystemContext: polkadot_overseer::gen::SubsystemContext<
	Signal=OverseerSignal,
	AllMessages=messages::AllMessages,
	Error=SubsystemError,
>
{

}

impl<T> SubsystemContext for T
where
	T: polkadot_overseer::gen::SubsystemContext<
		Signal=OverseerSignal,
		AllMessages=messages::AllMessages,
		Error=SubsystemError,
	>,
{

}
