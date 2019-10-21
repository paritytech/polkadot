// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! WASM re-execution of a parachain candidate.
//! In the context of relay-chain candidate evaluation, there are some additional
//! steps to ensure that the provided input parameters are correct.
//! Assuming the parameters are correct, this module provides a wrapper around
//! a WASM VM for re-execution of a parachain candidate.

use std::{cell::RefCell, fmt, convert::TryInto};
use crate::codec::{Decode, Encode};
use wasmi::{
	self, Module, ModuleInstance, Trap, MemoryInstance, MemoryDescriptor, MemoryRef,
	ModuleImportResolver, RuntimeValue, Externals, Error as WasmError, ValueType,
	memory_units::{self, Bytes, Pages, RoundUpTo}
};
use super::{
	ValidationParams, ValidationResult, MessageRef, UpwardMessageRef,
	UpwardMessage, IncomingMessage};

#[cfg(not(target_os = "unknown"))]
pub use validation_host::{run_worker, EXECUTION_TIMEOUT_SEC};

mod validation_host;

// maximum memory in bytes
const MAX_RUNTIME_MEM: usize = 1024 * 1024 * 1024; // 1 GiB
const MAX_CODE_MEM: usize = 16 * 1024 * 1024; // 16 MiB

mod ids {
	/// Post a message to another parachain.
	pub const POST_MESSAGE: usize = 1;

	/// Post a message to this parachain's relay chain.
	pub const POST_UPWARD_MESSAGE: usize = 2;
}

/// WASM code execution mode.
///
/// > Note: When compiling for WASM, the `Remote` variants are not available.
pub enum ExecutionMode {
	/// Execute in-process. The execution can not be interrupted or aborted.
	Local,
	/// Remote execution in a spawned process.
	Remote,
	/// Remote execution in a spawned test runner.
	RemoteTest,
}

/// Error type for the wasm executor
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Wasm error
	Wasm(WasmError),
	/// Externalities error
	Externalities(ExternalitiesError),
	/// Code size it too large.
	#[display(fmt = "WASM code is {} bytes, max allowed is {}", _0, MAX_CODE_MEM)]
	CodeTooLarge(usize),
	/// Call data is too large.
	#[display(fmt = "Validation parameters are {} bytes, max allowed is {}", _0, MAX_RUNTIME_MEM)]
	ParamsTooLarge(usize),
	/// Bad return data or type.
	#[display(fmt = "Validation function returned invalid data.")]
	BadReturn,
	#[display(fmt = "Validation function timeout.")]
	Timeout,
	#[display(fmt = "IO error: {}", _0)]
	Io(std::io::Error),
	#[display(fmt = "System error: {}", _0)]
	System(Box<dyn std::error::Error>),
	#[display(fmt = "WASM worker error: {}", _0)]
	External(String),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Wasm(ref err) => Some(err),
			Error::Externalities(ref err) => Some(err),
			Error::Io(ref err) => Some(err),
			Error::System(ref err) => Some(&**err),
			_ => None,
		}
	}
}

/// Errors that can occur in externalities of parachain validation.
#[derive(Debug, Clone)]
pub enum ExternalitiesError {
	/// Unable to post a message due to the given reason.
	CannotPostMessage(&'static str),
}

/// Externalities for parachain validation.
pub trait Externalities {
	/// Called when a message is to be posted to another parachain.
	fn post_message(&mut self, message: MessageRef) -> Result<(), ExternalitiesError>;

	/// Called when a message is to be posted to the parachain's relay chain.
	fn post_upward_message(&mut self, message: UpwardMessageRef) -> Result<(), ExternalitiesError>;
}

impl fmt::Display for ExternalitiesError {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			ExternalitiesError::CannotPostMessage(ref s)
				=> write!(f, "Cannot post message: {}", s),
		}
	}
}

impl wasmi::HostError for ExternalitiesError {}
impl std::error::Error for ExternalitiesError {}

struct Resolver {
	max_memory: u32, // in pages.
	memory: RefCell<Option<MemoryRef>>,
}

impl ModuleImportResolver for Resolver {
	fn resolve_func(
		&self,
		field_name: &str,
		signature: &wasmi::Signature
	) -> Result<wasmi::FuncRef, WasmError> {
		match field_name {
			"ext_post_message" => {
				let index = ids::POST_MESSAGE;
				let (params, ret_ty): (&[ValueType], Option<ValueType>) =
					(&[ValueType::I32, ValueType::I32, ValueType::I32], None);

				if signature.params() != params || signature.return_type() != ret_ty {
					Err(WasmError::Instantiation(
						format!("Export {} has a bad signature", field_name)
					))
				} else {
					Ok(wasmi::FuncInstance::alloc_host(
						wasmi::Signature::new(&params[..], ret_ty),
						index,
					))
				}
			}
			"ext_upwards_post_message" => {
				let index = ids::POST_UPWARD_MESSAGE;
				let (params, ret_ty): (&[ValueType], Option<ValueType>) =
					(&[ValueType::I32, ValueType::I32], None);

				if signature.params() != params || signature.return_type() != ret_ty {
					Err(WasmError::Instantiation(
						format!("Export {} has a bad signature", field_name)
					))
				} else {
					Ok(wasmi::FuncInstance::alloc_host(
						wasmi::Signature::new(&params[..], ret_ty),
						index,
					))
				}
			}
			_ => {
				Err(WasmError::Instantiation(
					format!("Export {} not found", field_name),
				))
			}
		}

	}

	fn resolve_memory(
		&self,
		field_name: &str,
		descriptor: &MemoryDescriptor,
	) -> Result<MemoryRef, WasmError> {
		if field_name == "memory" {
			let effective_max = descriptor.maximum().unwrap_or(self.max_memory);
			if descriptor.initial() > self.max_memory || effective_max > self.max_memory {
				Err(WasmError::Instantiation("Module requested too much memory".to_owned()))
			} else {
				let mem = MemoryInstance::alloc(
					memory_units::Pages(descriptor.initial() as usize),
					descriptor.maximum().map(|x| memory_units::Pages(x as usize)),
				)?;
				*self.memory.borrow_mut() = Some(mem.clone());
				Ok(mem)
			}
		} else {
			Err(WasmError::Instantiation("Memory imported under unknown name".to_owned()))
		}
	}
}

struct ValidationExternals<'a, E: 'a> {
	externalities: &'a mut E,
	memory: &'a MemoryRef,
}

impl<'a, E: 'a + Externalities> ValidationExternals<'a, E> {
	/// Signature: post_message(u32, *const u8, u32) -> None
	/// usage: post_message(target parachain, data ptr, data len).
	/// Data is the raw data of the message.
	fn ext_post_message(&mut self, args: ::wasmi::RuntimeArgs) -> Result<(), Trap> {
		let target: u32 = args.nth_checked(0)?;
		let data_ptr: u32 = args.nth_checked(1)?;
		let data_len: u32 = args.nth_checked(2)?;

		let (data_ptr, data_len) = (data_ptr as usize, data_len as usize);

		self.memory.with_direct_access(|mem| {
			if mem.len() < (data_ptr + data_len) {
				Err(Trap::new(wasmi::TrapKind::MemoryAccessOutOfBounds))
			} else {
				let res = self.externalities.post_message(MessageRef {
					target: target.into(),
					data: &mem[data_ptr..][..data_len],
				});

				res.map_err(|e| Trap::new(wasmi::TrapKind::Host(
					Box::new(e) as Box<_>
				)))
			}
		})
	}
	/// Signature: post_upward_message(u32, *const u8, u32) -> None
	/// usage: post_upward_message(origin, data ptr, data len).
	/// Origin is the integer representation of the dispatch origin.
	/// Data is the raw data of the message.
	fn ext_post_upward_message(&mut self, args: ::wasmi::RuntimeArgs) -> Result<(), Trap> {
		let origin: u32 = args.nth_checked(0)?;
		let data_ptr: u32 = args.nth_checked(1)?;
		let data_len: u32 = args.nth_checked(2)?;

		let (data_ptr, data_len) = (data_ptr as usize, data_len as usize);

		self.memory.with_direct_access(|mem| {
			if mem.len() < (data_ptr + data_len) {
				Err(Trap::new(wasmi::TrapKind::MemoryAccessOutOfBounds))
			} else {
				let origin = (origin as u8).try_into()
					.map_err(|_| Trap::new(wasmi::TrapKind::UnexpectedSignature))?;
				let message = UpwardMessageRef { origin, data: &mem[data_ptr..][..data_len] };
				let res = self.externalities.post_upward_message(message);
				res.map_err(|e| Trap::new(wasmi::TrapKind::Host(
					Box::new(e) as Box<_>
				)))
			}
		})
	}
}

impl<'a, E: 'a + Externalities> Externals for ValidationExternals<'a, E> {
	fn invoke_index(
		&mut self,
		index: usize,
		args: ::wasmi::RuntimeArgs,
	) -> Result<Option<RuntimeValue>, Trap> {
		match index {
			ids::POST_MESSAGE => self.ext_post_message(args).map(|_| None),
			ids::POST_UPWARD_MESSAGE => self.ext_post_upward_message(args).map(|_| None),
			_ => panic!("no externality at given index"),
		}
	}
}

/// Params header in shared memory. All offsets should be aligned to WASM page size.
#[derive(Encode, Decode, Debug)]
struct ValidationHeader {
	code_size: u64,
	params_size: u64,
}

#[derive(Encode, Decode, Debug)]
pub enum ValidationResultHeader {
	Ok {
		result: ValidationResult,
		egress_message_count: u64,
		up_message_count: u64,
	},
	Error(String),
}


#[derive(Default)]
struct WorkerExternalities {
	egress_data: Vec<u8>,
	egress_message_count: usize,
	up_data: Vec<u8>,
	up_message_count: usize,
}

impl Externalities for WorkerExternalities {
	fn post_message(&mut self, message: MessageRef) -> Result<(), ExternalitiesError> {
		IncomingMessage {
			source: message.target,
			data: message.data.to_vec(),
		}
		.encode_to(&mut self.egress_data);
		self.egress_message_count += 1;
		Ok(())
	}

	fn post_upward_message(&mut self, message: UpwardMessageRef) -> Result<(), ExternalitiesError> {
		UpwardMessage {
			origin: message.origin,
			data: message.data.to_vec(),
		}
		.encode_to(&mut self.up_data);
		self.up_message_count += 1;
		Ok(())
	}
}

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate<E: substrate_externalities::Externalities>(
	validation_code: &[u8],
	params: ValidationParams,
	externalities: &mut E,
	options: ExecutionMode,
	) -> Result<ValidationResult, Error>
{
	match options {
		ExecutionMode::Local => {
			validate_candidate_internal(validation_code, &params.encode(), externalities)
		},
		#[cfg(not(target_os = "unknown"))]
		ExecutionMode::Remote =>
			validation_host::validate_candidate(validation_code, params, externalities, false),
		#[cfg(not(target_os = "unknown"))]
		ExecutionMode::RemoteTest =>
			validation_host::validate_candidate(validation_code, params, externalities, true),
		#[cfg(target_os = "unknown")]
		ExecutionMode::Remote =>
			Err(Error::System("Remote validator not available".to_string().into())),
		#[cfg(target_os = "unknown")]
		ExecutionMode::RemoteTest =>
			Err(Error::System("Remote validator not available".to_string().into())),
	}
}

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate_internal<E: substrate_externalities::Externalities>(
	validation_code: &[u8],
	encoded_call_data: &[u8],
	externalities: &mut E,
) -> Result<ValidationResult, Error> {
	let res = substrate_executor::call_in_wasm(
		"validate_block",
		encoded_call_data,
		substrate_executor::WasmExecutionMethod::Interpreted,
		externalities,
		validation_code,
		1024,
	).map_err(|e| Error::External(format!("{:?}", e)))?;

	ValidationResult::decode(&mut &res[..]).map_err(|_| Error::BadReturn.into())
}
