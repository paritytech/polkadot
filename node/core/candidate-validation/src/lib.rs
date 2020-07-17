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

//! The Candidate Validation subsystem.
//!
//! This handles incoming requests from other subsystems to validate candidates
//! according to a validation function. This delegates validation to an underlying
//! pool of processes used for execution of the Wasm.

use polkadot_subsystem::{Subsystem, SubsystemContext, SpawnedSubsystem};
use polkadot_subsystem::messages::{AllMessages, CandidateValidationMessage, RuntimeApiMessage};
use polkadot_parachain::wasm_executor::{self, ValidationPool};

pub struct CandidateValidationSubsystem;

async fn run(mut ctx: impl SubsystemContext<Message = CandidateValidationMessage>) {
	let pool = ValidationPool::new();

	loop {

	}
}
