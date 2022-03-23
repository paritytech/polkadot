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

//! Relay errors.

use relay_substrate_client as client;
use sp_finality_grandpa::AuthorityList;
use sp_runtime::traits::MaybeDisplay;
use std::fmt::Debug;
use thiserror::Error;

/// Relay errors.
#[derive(Error, Debug)]
pub enum Error<Hash: Debug + MaybeDisplay, HeaderNumber: Debug + MaybeDisplay> {
	/// Failed to submit signed extrinsic from to the target chain.
	#[error("Failed to submit {0} transaction: {1:?}")]
	SubmitTransaction(&'static str, client::Error),
	/// Failed subscribe to justification stream of the source chain.
	#[error("Failed to subscribe to {0} justifications: {1:?}")]
	Subscribe(&'static str, client::Error),
	/// Failed subscribe to read justification from the source chain (client error).
	#[error("Failed to read {0} justification from the stream: {1}")]
	ReadJustification(&'static str, client::Error),
	/// Failed subscribe to read justification from the source chain (stream ended).
	#[error("Failed to read {0} justification from the stream: stream has ended unexpectedly")]
	ReadJustificationStreamEnded(&'static str),
	/// Failed subscribe to decode justification from the source chain.
	#[error("Failed to decode {0} justification: {1:?}")]
	DecodeJustification(&'static str, codec::Error),
	/// GRANDPA authorities read from the source chain are invalid.
	#[error("Read invalid {0} authorities set: {1:?}")]
	ReadInvalidAuthorities(&'static str, AuthorityList),
	/// Failed to guess initial GRANDPA authorities at the given header of the source chain.
	#[error("Failed to guess initial {0} GRANDPA authorities set id: checked all possible ids in range [0; {1}]")]
	GuessInitialAuthorities(&'static str, HeaderNumber),
	/// Failed to retrieve GRANDPA authorities at the given header from the source chain.
	#[error("Failed to retrive {0} GRANDPA authorities set at header {1}: {2:?}")]
	RetrieveAuthorities(&'static str, Hash, client::Error),
	/// Failed to decode GRANDPA authorities at the given header of the source chain.
	#[error("Failed to decode {0} GRANDPA authorities set at header {1}: {2:?}")]
	DecodeAuthorities(&'static str, Hash, codec::Error),
	/// Failed to retrieve header by the hash from the source chain.
	#[error("Failed to retrieve {0} header with hash {1}: {:?}")]
	RetrieveHeader(&'static str, Hash, client::Error),
	/// Failed to retrieve best finalized source header hash from the target chain.
	#[error("Failed to retrieve best finalized {0} header from the target chain: {1}")]
	RetrieveBestFinalizedHeaderHash(&'static str, client::Error),
}
