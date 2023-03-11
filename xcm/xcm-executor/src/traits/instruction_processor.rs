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

use crate::XcVmRegisters;
use xcm::latest::{Error as XcmError, Instruction};

pub trait ProcessInstruction<Call>: Sized {
	/// Initialize the processor
	fn new() -> Self;

	/// Process a single XCM instruction, mutating the state of the XCM virtual machine.
	fn process_instruction(
		&mut self,
		vm_state: &mut XcVmRegisters<Call>,
		instr: Instruction<Call>,
	) -> Result<(), XcmError>;

	/// Execute any final operations after having executed the XCM message.
	/// This includes refunding surplus weight.
	fn post_process(&mut self, vm_state: &mut XcVmRegisters<Call>);
}
