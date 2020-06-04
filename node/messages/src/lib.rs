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

//! Message types for the overseer and subsystems.
//!
//! Subsystems' APIs are defined separately from their implementation, leading to easier mocking.

use polkadot_primitives::Hash;

/// A signal used by [`Overseer`] to communicate with the [`Subsystem`]s.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
#[derive(PartialEq, Clone, Debug)]
pub enum OverseerSignal {
	/// `Subsystem` should start working on block-based work, given by the relay-chain block hash.
	StartWork(Hash),
	/// `Subsystem` should stop working on block-based work specified by the relay-chain block hash.
	StopWork(Hash),
	/// Conclude the work of the `Overseer` and all `Subsystem`s.
	Conclude,
}

/// A message type used by the Validation [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum ValidationSubsystemMessage {
	ValidityAttestation,
}

/// A message type used by the CandidateBacking [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum CandidateBackingSubsystemMessage {
	RegisterBackingWatcher,
	Second,
}

/// A message type tying together all message types that are used across [`Subsystem`]s.
///
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum AllMessages {
	Validation(ValidationSubsystemMessage),
	CandidateBacking(CandidateBackingSubsystemMessage),
}

/// A message type that a [`Subsystem`] receives from the [`Overseer`].
/// It wraps siglans from the [`Overseer`] and messages that are circulating
/// between subsystems.
///
/// It is generic over over the message type `M` that a particular `Subsystem` may use.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum FromOverseer<M: std::fmt::Debug> {
	/// Signal from the `Overseer`.
	Signal(OverseerSignal),

	/// Some other `Subsystem`'s message.
	Communication {
		msg: M,
	},
}
