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
use super::{ValidationParams, ValidationResult, MessageRef, UpwardMessageRef};

mod ids {
	/// Post a message to another parachain.
	pub const POST_MESSAGE: usize = 1;

	/// Post a message to this parachain's relay chain.
	pub const POST_UPWARDS_MESSAGE: usize = 2;
}

/// Error type for the wasm executor
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Wasm error
	Wasm(WasmError),
	/// Externalities error
	Externalities(ExternalitiesError),
	/// Call data too big. WASM32 only has a 32-bit address space.
	#[display(fmt = "Validation parameters took up {} bytes, max allowed by WASM is {}", _0, i32::max_value())]
	ParamsTooLarge(usize),
	/// Bad return data or type.
	#[display(fmt = "Validation function returned invalid data.")]
	BadReturn,
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Wasm(ref err) => Some(err),
			Error::Externalities(ref err) => Some(err),
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
impl ::std::error::Error for ExternalitiesError {}

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

				if signature.params() != params && signature.return_type() != ret_ty {
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
				let index = ids::POST_UPWARDS_MESSAGE;
				let (params, ret_ty): (&[ValueType], Option<ValueType>) =
					(&[ValueType::I32, ValueType::I32], None);

				if signature.params() != params && signature.return_type() != ret_ty {
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
			ids::POST_UPWARDS_MESSAGE => self.ext_post_upward_message(args).map(|_| None),
			_ => panic!("no externality at given index"),
		}
	}
}

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate<E: Externalities>(
	validation_code: &[u8],
	params: ValidationParams,
	externalities: &mut E,
) -> Result<ValidationResult, Error> {
	use wasmi::LINEAR_MEMORY_PAGE_SIZE;

	// maximum memory in bytes
	const MAX_MEM: u32 = 1024 * 1024 * 1024; // 1 GiB

	// instantiate the module.
	let memory;
	let mut externals;
	let module = {
		let module = Module::from_buffer(validation_code)?;

		let module_resolver = Resolver {
			max_memory: MAX_MEM / LINEAR_MEMORY_PAGE_SIZE.0 as u32,
			memory: RefCell::new(None),
		};

		let module = ModuleInstance::new(
			&module,
			&wasmi::ImportsBuilder::new().with_resolver("env", &module_resolver),
		)?;

		memory = module_resolver.memory.borrow()
			.as_ref()
			.ok_or_else(|| WasmError::Instantiation("No imported memory instance".to_owned()))?
			.clone();

		externals = ValidationExternals {
			externalities,
			memory: &memory,
		};

		module.run_start(&mut externals).map_err(WasmError::Trap)?
	};

	// allocate call data in memory.
	// we guarantee that:
	// - `offset` has alignment at least of 8,
	// - `len` is not zero.
	let (offset, len) = {
		let encoded_call_data = params.encode();

		// hard limit from WASM.
		if encoded_call_data.len() > i32::max_value() as usize {
			return Err(Error::ParamsTooLarge(encoded_call_data.len()));
		}

		// allocate sufficient amount of wasm pages to fit encoded call data.
		let call_data_pages: Pages = Bytes(encoded_call_data.len()).round_up_to();
		let allocated_mem_start: Bytes = memory.grow(call_data_pages)?.into();

		memory.set(allocated_mem_start.0 as u32, &encoded_call_data)
			.expect(
				"enough memory allocated just before this; \
				copying never fails if memory is large enough; qed"
			);

		(allocated_mem_start.0, encoded_call_data.len())
	};

	let output = module.invoke_export(
		"validate_block",
		&[RuntimeValue::I32(offset as i32), RuntimeValue::I32(len as i32)],
		&mut externals,
	).map_err(|e| -> Error {
		e.as_host_error()
			.and_then(|he| he.downcast_ref::<ExternalitiesError>())
			.map(|ee| Error::Externalities(ee.clone()))
			.unwrap_or_else(move || e.into())
	})?;

	match output {
		Some(RuntimeValue::I32(len_offset)) => {
			let len_offset = len_offset as u32;

			let mut len_bytes = [0u8; 4];
			memory.get_into(len_offset, &mut len_bytes)?;
			let len_offset = len_offset as usize;

			let len = u32::decode(&mut &len_bytes[..])
				.ok_or_else(|| Error::BadReturn)? as usize;

			let return_offset = if len > len_offset {
				return Err(Error::BadReturn);
			} else {
				len_offset - len
			};

			memory.with_direct_access(|mem| {
				if mem.len() < return_offset + len {
					return Err(Error::BadReturn);
				}

				ValidationResult::decode(&mut &mem[return_offset..][..len])
					.ok_or_else(|| Error::BadReturn)
					.map_err(Into::into)
			})
		}
		_ => Err(Error::BadReturn),
	}
}
