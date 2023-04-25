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

pub trait PreprocessInstruction<Call>: Default + Sized {
	/// Process a single XCM instruction, mutating the state of the XCM virtual machine.
	fn preprocess_instruction(
		&mut self,
		_vm_state: &mut XcVmRegisters<Call>,
		_instr: &mut Instruction<Call>,
	) -> Result<bool, XcmError> {
		Ok(true)
	}
}

impl<Call> PreprocessInstruction<Call> for () {}
