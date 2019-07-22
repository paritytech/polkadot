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

use std::{cell::RefCell, fmt, convert::TryInto, process, env};
use std::sync::{Arc, atomic};
use crate::codec::{Decode, Encode};
use wasmi::{
	self, Module, ModuleInstance, Trap, MemoryInstance, MemoryDescriptor, MemoryRef,
	ModuleImportResolver, RuntimeValue, Externals, Error as WasmError, ValueType,
	memory_units::{self, Bytes, Pages, RoundUpTo}
};
use super::{ValidationParams, ValidationResult, MessageRef, UpwardMessageRef, UpwardMessage, IncomingMessage};
use shared_memory::{SharedMem, SharedMemConf, EventState, WriteLockable, EventWait, EventSet};
use parking_lot::Mutex;
use log::{trace, debug};

// maximum memory in bytes
const MAX_RUNTIME_MEM: usize = 1024 * 1024 * 1024; // 1 GiB
const MAX_CODE_MEM: usize = 16 * 1024 * 1024; // 16 MiB
// Message data limit
const MAX_MESSAGE_MEM: usize = 16 * 1024 * 1024; // 16 MiB

const WORKER_ARGS_TEST: &[&'static str] = &["--nocapture", "validation_worker"];
/// CLI Argument to start in validation worker mode.
const WORKER_ARG: &'static str = "validation-worker";
const WORKER_ARGS: &[&'static str] = &[WORKER_ARG];

enum Event {
	CandidateReady = 0,
	ResultReady = 1,
	WorkerReady = 2,
}

lazy_static::lazy_static! {
	static ref HOST: Mutex<ValidationHost> = Mutex::new(ValidationHost::new());
}

mod ids {
	/// Post a message to another parachain.
	pub const POST_MESSAGE: usize = 1;

	/// Post a message to this parachain's relay chain.
	pub const POST_UPWARDS_MESSAGE: usize = 2;
}

/// WASM code execution mode.
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

/// Validation worker process entry point. Runs a loop waiting for canidates to validate
/// and sends back results via shared memory.
pub fn run_worker(mem_id: &str) -> Result<(), String> {
	let mut memory = match SharedMem::open(mem_id) {
		Ok(memory) => memory,
		Err(e) => {
			debug!("Error opening shared memory: {:?}", e);
			return Err(format!("Error opening shared memory: {:?}", e));
		}
	};
	let mut externalities = WorkerExternalities::default();

	let exit = Arc::new(atomic::AtomicBool::new(false));
	// spawn parent monitor thread
	let watch_exit = exit.clone();
	std::thread::spawn(move || {
		use std::io::Read;
		let mut in_data = Vec::new();
		std::io::stdin().read_to_end(&mut in_data).ok(); // pipe terminates when parent process exits
		debug!("Parent process is dead. Exiting");
		exit.store(true, atomic::Ordering::Relaxed);
	});

	memory.set(Event::WorkerReady as usize, EventState::Signaled)
		.map_err(|e| format!("Error setting shared event: {:?}", e))?;

	loop {
		if watch_exit.load(atomic::Ordering::Relaxed) {
			break;
		}

		debug!("Waiting for candidate");
		match memory.wait(Event::CandidateReady as usize, shared_memory::Timeout::Sec(1)) {
			Err(e) => {
				// Timeout
				trace!("Timeout waiting for candidate: {:?}", e);
				continue;
			}
			Ok(()) => {}
		}

		{
			debug!("Processing candidate");
			// we have candidate data
			let mut slice = memory.wlock_as_slice(0)
				.map_err(|e| format!("Error locking shared memory: {:?}", e))?;

			let result = {
				let data: &mut[u8] = &mut **slice;
				let (header_buf, rest) = data.split_at_mut(1024);
				let mut header_buf: &[u8] = header_buf;
				let header = ValidationHeader::decode(&mut header_buf)
					.ok_or_else(|| format!("Error decoding validation request."))?;
				debug!("Candidate header: {:?}", header);
				let (code, rest) = rest.split_at_mut(MAX_CODE_MEM);
				let (code, _) = code.split_at_mut(header.code_size as usize);
				let (call_data, rest) = rest.split_at_mut(MAX_RUNTIME_MEM);
				let (call_data, _) = call_data.split_at_mut(header.params_size as usize);
				let message_data = rest;

				let result = validate_candidate_internal(code, call_data, &mut externalities);
				debug!("Candidate validated: {:?}", result);

				match result {
					Ok(r) => {
						if externalities.egress_data.len() + externalities.up_data.len() > MAX_MESSAGE_MEM {
							ValidationResultHeader::Error("Message data is too large".into())
						} else {
							let e_len = externalities.egress_data.len();
							let up_len = externalities.up_data.len();
							message_data[0..e_len].copy_from_slice(&externalities.egress_data);
							message_data[e_len..(e_len + up_len)].copy_from_slice(&externalities.up_data);
							ValidationResultHeader::Ok {
								result: r,
								egress_message_count: externalities.egress_message_count as u64,
								up_message_count: externalities.up_message_count as u64,
							}
						}
					},
					Err(e) => ValidationResultHeader::Error(e.to_string()),
				}
			};
			let mut data: &mut[u8] = &mut **slice;
			result.encode_to(&mut data);
		}
		debug!("Signaling result");
		memory.set(Event::ResultReady as usize, EventState::Signaled)
			.map_err(|e| format!("Error setting shared event: {:?}", e))?;
	}
	Ok(())
}

unsafe impl Send for ValidationHost {}

struct ValidationHost {
	worker: Option<process::Child>,
	memory: Option<SharedMem>,
}


impl Drop for ValidationHost {
	fn drop(&mut self) {
		if let Some(ref mut worker) = &mut self.worker {
			worker.kill().ok();
		}
	}
}

impl ValidationHost {
	fn create_memory() -> Result<SharedMem, Error> {
		let mem_size = MAX_RUNTIME_MEM + MAX_CODE_MEM + MAX_MESSAGE_MEM + 1024;
		let mem_config = SharedMemConf::new()
			.set_size(mem_size)
			.add_lock(shared_memory::LockType::Mutex, 0, mem_size)?
			.add_event(shared_memory::EventType::Auto)?  // Event::CandidateReady
			.add_event(shared_memory::EventType::Auto)?  // Event::ResultReady
			.add_event(shared_memory::EventType::Auto)?; // Evebt::WorkerReady

		Ok(mem_config.create()?)
	}

	fn new() -> ValidationHost {
		ValidationHost {
			worker: None,
			memory: None,
		}
	}

	fn start_worker(&mut self, test_mode: bool) -> Result<(), Error> {
		if let Some(ref mut worker) = self.worker {
			// Check if still alive
			if let Ok(None) = worker.try_wait() {
				// Still running
				return Ok(());
			}
		}
		let memory = Self::create_memory()?;
		let self_path = env::current_exe()?;
		debug!("Starting worker at {:?}", self_path);
		let mut args = if test_mode { WORKER_ARGS_TEST.to_vec() } else { WORKER_ARGS.to_vec() };
		args.push(memory.get_os_path());
		let worker = process::Command::new(self_path)
			.args(args)
			.stdin(process::Stdio::piped())
			.spawn()?;
		self.worker = Some(worker);

		memory.wait(Event::WorkerReady as usize, shared_memory::Timeout::Sec(5))?;
		self.memory = Some(memory);
		Ok(())
	}

	/// Validate a candidate under the given validation code.
	///
	/// This will fail if the validation code is not a proper parachain validation module.
	fn validate_candidate<E: Externalities>(
		&mut self,
		validation_code: &[u8],
		params: ValidationParams,
		externalities: &mut E,
		test_mode: bool,
	) -> Result<ValidationResult, Error>
	{
		if validation_code.len() > MAX_CODE_MEM {
			return Err(Error::CodeTooLarge(validation_code.len()));
		}
		// First, check if need to spawn the child process
		self.start_worker(test_mode)?;
		let memory = self.memory.as_mut().expect("memory is always `Some` after `start_worker` completes successfully");
		{
			// Put data in shared mem
			let data: &mut[u8] = &mut **memory.wlock_as_slice(0)?;
			let (mut header_buf, rest) = data.split_at_mut(1024);
			let (code, rest) = rest.split_at_mut(MAX_CODE_MEM);
			let (code, _) = code.split_at_mut(validation_code.len());
			let (call_data, _) = rest.split_at_mut(MAX_RUNTIME_MEM);
			code[..validation_code.len()].copy_from_slice(validation_code);
			let encoded_params = params.encode();
			if encoded_params.len() >= MAX_RUNTIME_MEM {
				return Err(Error::ParamsTooLarge(MAX_RUNTIME_MEM));
			}
			call_data[..encoded_params.len()].copy_from_slice(&encoded_params);

			let header = ValidationHeader {
				code_size: validation_code.len() as u64,
				params_size: encoded_params.len() as u64,
			};

			header.encode_to(&mut header_buf);
		}

		debug!("Signaling candidate");
		memory.set(Event::CandidateReady as usize, EventState::Signaled)?;

		debug!("Waiting for results");
		match memory.wait(Event::ResultReady as usize, shared_memory::Timeout::Sec(5)) {
			Err(e) => {
				debug!("Worker timeout: {:?}", e);
				if let Some(mut worker) = self.worker.take() {
					worker.kill().ok();
				}
				return Err(Error::Timeout.into());
			}
			Ok(()) => {}
		}

		{
			let data: &[u8] = &**memory.wlock_as_slice(0)?;
			let (header_buf, rest) = data.split_at(1024);
			let (_, rest) = rest.split_at(MAX_CODE_MEM);
			let (_, message_data) = rest.split_at(MAX_RUNTIME_MEM);
			let mut header_buf: &[u8] = header_buf;
			let mut message_data: &[u8] = message_data;
			let header = ValidationResultHeader::decode(&mut header_buf).unwrap();
			match header {
				ValidationResultHeader::Ok { result, egress_message_count, up_message_count } => {
					for _ in 0 .. egress_message_count {
						let message = IncomingMessage::decode(&mut message_data).unwrap();
						let message_ref = MessageRef {
							target: message.source,
							data: &message.data,
						};
						externalities.post_message(message_ref)?;
					}
					for _ in 0 .. up_message_count {
						let message = UpwardMessage::decode(&mut message_data).unwrap();
						let message_ref = UpwardMessageRef {
							origin: message.origin,
							data: &message.data,
						};
						externalities.post_upward_message(message_ref)?;
					}
					Ok(result)
				}
				ValidationResultHeader::Error(message) => {
					Err(Error::External(message).into())
				}
			}
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
	options: ExecutionMode,
	) -> Result<ValidationResult, Error>
{
	match options {
		ExecutionMode::Local => {
			validate_candidate_internal(validation_code, &params.encode(), externalities)
		},
		ExecutionMode::Remote =>
			HOST.lock().validate_candidate(validation_code, params, externalities, false),
		ExecutionMode::RemoteTest =>
			HOST.lock().validate_candidate(validation_code, params, externalities, true),
	}
}

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate_internal<E: Externalities>(
	validation_code: &[u8],
	encoded_call_data: &[u8],
	externalities: &mut E,
	) -> Result<ValidationResult, Error> {
	use wasmi::LINEAR_MEMORY_PAGE_SIZE;

	// instantiate the module.
	let memory;
	let mut externals;
	let module = {
		let module = Module::from_buffer(validation_code)?;

		let module_resolver = Resolver {
			max_memory: (MAX_RUNTIME_MEM / LINEAR_MEMORY_PAGE_SIZE.0) as u32,
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
