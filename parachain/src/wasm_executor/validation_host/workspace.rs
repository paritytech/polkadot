// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! This module implements a "workspace" - basically a wrapper around a shared memory that
//! is used as an IPC channel for communication between the validation host and it's validation
//! worker.

use crate::primitives::{ValidationParams, ValidationResult};
use super::LOG_TARGET;
use parity_scale_codec::{Decode, Encode};
use raw_sync::{
	events::{Event, EventImpl, EventInit, EventState},
	Timeout,
};
use shared_memory::{Shmem, ShmemConf};
use std::{
	error::Error,
	fmt,
	io::{Cursor, Write},
	slice,
	sync::atomic::AtomicBool,
	time::Duration,
};

// maximum memory in bytes
const MAX_PARAMS_MEM: usize = 16 * 1024 * 1024; // 16 MiB
const MAX_CODE_MEM: usize = 16 * 1024 * 1024; // 16 MiB

/// The size of the shared workspace region. The maximum amount
const SHARED_WORKSPACE_SIZE: usize = MAX_PARAMS_MEM + MAX_CODE_MEM + (1024 * 1024);

/// Params header in shared memory. All offsets should be aligned to WASM page size.
#[derive(Encode, Decode, Debug)]
struct ValidationHeader {
	code_size: u64,
	params_size: u64,
}

/// An error that could happen during validation of a candidate.
#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
pub enum WorkerValidationError {
	InternalError(String),
	ValidationError(String),
}

/// An enum that is used to marshal a validation result in order to pass it through the shared memory.
#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
pub enum ValidationResultHeader {
	Ok(ValidationResult),
	Error(WorkerValidationError),
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum Mode {
	Initialize,
	Attach,
}

fn stringify_err(err: Box<dyn Error>) -> String {
	format!("{:?}", err)
}

struct Inner {
	shmem: Shmem,
	candidate_ready_ev: Box<dyn EventImpl>,
	result_ready_ev: Box<dyn EventImpl>,
	worker_ready_ev: Box<dyn EventImpl>,

	/// Flag that indicates that the worker side is attached to this workspace.
	///
	/// While there are apparent problems attaching multiple workers to the same workspace, we don't
	/// need that anyway. So to make our reasoning a little bit simpler just add a flag and check
	/// it before attaching.
	attached: *mut AtomicBool,

	/// The number of bytes reserved by the auxilary stuff like events from the beginning of the
	/// shared memory area.
	///
	/// We expect this to be way smaller than the whole shmem size.
	consumed: usize,
}

impl Inner {
	fn layout(shmem: Shmem, mode: Mode) -> Self {
		unsafe {
			let base_ptr = shmem.as_ptr();
			let mut consumed = 0;

			let candidate_ready_ev = add_event(base_ptr, &mut consumed, mode);
			let result_ready_ev = add_event(base_ptr, &mut consumed, mode);
			let worker_ready_ev = add_event(base_ptr, &mut consumed, mode);

			// The size of AtomicBool is guaranteed to be the same as the bool, however, docs
			// on the bool primitve doesn't actually state that the in-memory size is equal to 1 byte.
			//
			// AtomicBool requires hardware support of 1 byte width of atomic operations though, so
			// that should be fine.
			//
			// We still assert here to be safe than sorry.
			static_assertions::assert_eq_size!(AtomicBool, u8);
			// SAFETY: `AtomicBool` is represented by an u8 thus will be happy to take any alignment.
			let attached = base_ptr.add(consumed) as *mut AtomicBool;
			consumed += 1;

			let consumed = align_up_to(consumed, 64);

			Self {
				shmem,
				attached,
				consumed,
				candidate_ready_ev,
				result_ready_ev,
				worker_ready_ev,
			}
		}
	}

	fn as_slice(&self) -> &[u8] {
		unsafe {
			let base_ptr = self.shmem.as_ptr().add(self.consumed);
			let remaining = self.shmem.len() - self.consumed;
			slice::from_raw_parts(base_ptr, remaining)
		}
	}

	fn as_slice_mut(&mut self) -> &mut [u8] {
		unsafe {
			let base_ptr = self.shmem.as_ptr().add(self.consumed);
			let remaining = self.shmem.len() - self.consumed;
			slice::from_raw_parts_mut(base_ptr, remaining)
		}
	}

	/// Mark that this workspace has an attached worker already. Returning `true` means that this
	/// was the first worker attached.
	fn declare_exclusive_attached(&self) -> bool {
		unsafe {
			// If this succeeded then the value was `false`, thus, we managed to attach exclusively.
			(&*self.attached)
				.compare_exchange_weak(
					false,
					true,
					std::sync::atomic::Ordering::SeqCst,
					std::sync::atomic::Ordering::SeqCst,
				)
				.is_ok()
		}
	}
}

fn align_up_to(v: usize, alignment: usize) -> usize {
	((v + alignment - 1) / alignment) * alignment
}

/// Initializes a new or attaches to an exising event.
///
/// # Safety
///
/// This function should be called with the combination of `base_ptr` and `consumed` so that `base_ptr + consumed`
/// points on the memory area that is allocated and accessible.
///
/// This function should be called only once for the same combination of the `base_ptr + consumed` and the mode.
/// Furthermore, this function should be called once for initialization.
///
/// Specifically, `consumed` should not be modified by the caller, it should be passed as is to this function.
unsafe fn add_event(base_ptr: *mut u8, consumed: &mut usize, mode: Mode) -> Box<dyn EventImpl> {
	// SAFETY: there is no safety proof since the documentation doesn't specify the particular constraints
	//         besides requiring the pointer to be valid. AFAICT, the pointer is valid.
	let ptr = base_ptr.add(*consumed);

	const EXPECTATION: &str =
		"given that the preconditions were fulfilled, the creation of the event should succeed";
	let (ev, used_bytes) = match mode {
		Mode::Initialize => Event::new(ptr, true).expect(EXPECTATION),
		Mode::Attach => Event::from_existing(ptr).expect(EXPECTATION),
	};
	*consumed += used_bytes;
	ev
}

/// A message received by the worker that specifies a candidate validation work.
pub struct WorkItem<'handle> {
	pub params: &'handle [u8],
	pub code: &'handle [u8],
}

/// An error that could be returned from [`WorkerHandle::wait_for_work`].
#[derive(Debug)]
pub enum WaitForWorkErr {
	/// An error occured during waiting for work. Typically a timeout.
	Wait(String),
	/// An error ocurred when trying to decode the validation request from the host.
	FailedToDecode(String),
}

/// An error that could be returned from [`WorkerHandle::report_result`].
#[derive(Debug)]
pub enum ReportResultErr {
	/// An error occured during signalling to the host that the result is ready.
	Signal(String),
}

/// A worker side handle to a workspace.
pub struct WorkerHandle {
	inner: Inner,
}

impl WorkerHandle {
	/// Signals to the validation host that this worker is ready to accept new work requests.
	pub fn signal_ready(&self) -> Result<(), String> {
		self.inner
			.worker_ready_ev
			.set(EventState::Signaled)
			.map_err(stringify_err)?;
		Ok(())
	}

	/// Waits until a new piece of work. Returns `Err` if the work doesn't come within the given
	/// timeout.
	pub fn wait_for_work(&mut self, timeout_secs: u64) -> Result<WorkItem, WaitForWorkErr> {
		self.inner
			.candidate_ready_ev
			.wait(Timeout::Val(Duration::from_secs(timeout_secs)))
			.map_err(stringify_err)
			.map_err(WaitForWorkErr::Wait)?;

		let mut cur = self.inner.as_slice();
		let header = ValidationHeader::decode(&mut cur)
			.map_err(|e| format!("{:?}", e))
			.map_err(WaitForWorkErr::FailedToDecode)?;

		let (params, cur) = cur.split_at(header.params_size as usize);
		let (code, _) = cur.split_at(header.code_size as usize);

		Ok(WorkItem { params, code })
	}

	/// Report back the result of validation.
	pub fn report_result(&mut self, result: ValidationResultHeader) -> Result<(), ReportResultErr> {
		let mut cur = self.inner.as_slice_mut();
		result.encode_to(&mut cur);
		self.inner
			.result_ready_ev
			.set(EventState::Signaled)
			.map_err(stringify_err)
			.map_err(ReportResultErr::Signal)?;

		Ok(())
	}
}

/// An error that could be returned from [`HostHandle::wait_until_ready`].
#[derive(Debug)]
pub enum WaitUntilReadyErr {
	/// An error occured during waiting for the signal from the worker.
	Wait(String),
}

/// An error that could be returned from [`HostHandle::request_validation`].
#[derive(Debug)]
pub enum RequestValidationErr {
	/// The code passed exceeds the maximum allowed limit.
	CodeTooLarge { actual: usize, max: usize },
	/// The call parameters exceed the maximum allowed limit.
	ParamsTooLarge { actual: usize, max: usize },
	/// An error occured during writing either the code or the call params (the inner string specifies which)
	WriteData(&'static str),
	/// An error occured during signalling that the request is ready.
	Signal(String),
}

/// An error that could be returned from [`HostHandle::wait_for_result`]
#[derive(Debug)]
pub enum WaitForResultErr {
	/// A error happened during waiting for the signal. Typically a timeout.
	Wait(String),
	/// Failed to decode the result header sent by the worker.
	HeaderDecodeErr(String),
}

/// A worker side handle to a workspace.
pub struct HostHandle {
	inner: Inner,
}

impl fmt::Debug for HostHandle {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "HostHandle")
	}
}

impl HostHandle {
	/// Returns the OS specific ID for this workspace.
	pub fn id(&self) -> &str {
		self.inner.shmem.get_os_id()
	}

	/// Wait until the worker is online and ready for accepting validation requests.
	pub fn wait_until_ready(&self, timeout_secs: u64) -> Result<(), WaitUntilReadyErr> {
		self.inner
			.worker_ready_ev
			.wait(Timeout::Val(Duration::from_secs(timeout_secs)))
			.map_err(stringify_err)
			.map_err(WaitUntilReadyErr::Wait)?;
		Ok(())
	}

	/// Request validation with the given code and parameters.
	pub fn request_validation(
		&mut self,
		code: &[u8],
		params: ValidationParams,
	) -> Result<(), RequestValidationErr> {
		if code.len() > MAX_CODE_MEM {
			return Err(RequestValidationErr::CodeTooLarge {
				actual: code.len(),
				max: MAX_CODE_MEM,
			});
		}

		let params = params.encode();
		if params.len() > MAX_PARAMS_MEM {
			return Err(RequestValidationErr::ParamsTooLarge {
				actual: params.len(),
				max: MAX_PARAMS_MEM,
			});
		}

		let mut cur = Cursor::new(self.inner.as_slice_mut());
		ValidationHeader {
			code_size: code.len() as u64,
			params_size: params.len() as u64,
		}
		.encode_to(&mut cur);
		cur.write_all(&params)
			.map_err(|_| RequestValidationErr::WriteData("params"))?;
		cur.write_all(code)
			.map_err(|_| RequestValidationErr::WriteData("code"))?;

		self.inner
			.candidate_ready_ev
			.set(EventState::Signaled)
			.map_err(stringify_err)
			.map_err(RequestValidationErr::Signal)?;

		Ok(())
	}

	/// Wait for the validation result from the worker with the given timeout.
	///
	/// Returns `Ok` if the response was received within the deadline or error otherwise. An error
	/// could also occur because of failing decoding the result from the worker. Returning
	/// `Ok` doesn't mean that the candidate was successfully validated though, for that the client
	/// needs to inspect the returned validation result header.
	pub fn wait_for_result(
		&self,
		execution_timeout: u64,
	) -> Result<ValidationResultHeader, WaitForResultErr> {
		self.inner
			.result_ready_ev
			.wait(Timeout::Val(Duration::from_secs(execution_timeout)))
			.map_err(|e| WaitForResultErr::Wait(format!("{:?}", e)))?;

		let mut cur = self.inner.as_slice();
		let header = ValidationResultHeader::decode(&mut cur)
			.map_err(|e| WaitForResultErr::HeaderDecodeErr(format!("{:?}", e)))?;
		Ok(header)
	}
}

/// Create a new workspace and return a handle to it.
pub fn create() -> Result<HostHandle, String> {
	let shmem = ShmemConf::new()
		.size(SHARED_WORKSPACE_SIZE)
		.create()
		.map_err(|e| format!("Error creating shared memory: {:?}", e))?;

	Ok(HostHandle {
		inner: Inner::layout(shmem, Mode::Initialize),
	})
}

/// Open a workspace with the given `id`.
///
/// You can attach only once to a single workspace.
pub fn open(id: &str) -> Result<WorkerHandle, String> {
	let shmem = ShmemConf::new()
		.os_id(id)
		.open()
		.map_err(|e| format!("Error opening shared memory: {:?}", e))?;

	#[cfg(unix)]
	unlink_shmem(&id);

	let inner = Inner::layout(shmem, Mode::Attach);
	if !inner.declare_exclusive_attached() {
		return Err(format!("The workspace has been already attached to"));
	}

	return Ok(WorkerHandle { inner });

	#[cfg(unix)]
	fn unlink_shmem(shmem_id: &str) {
		// Unlink the shmem. Unlinking it from the filesystem will make it unaccessible for further
		// opening, however, the kernel will still let the object live until the last reference dies
		// out.
		//
		// There is still a chance that the shm stays on the fs, but that's a highly unlikely case
		// that we don't address at this time.

		// shared-memory doesn't return file path to the shmem if get_flink_path is called, so we
		// resort to `shm_unlink`.
		//
		// Additionally, even thouygh `fs::remove_file` is said to use `unlink` we still avoid relying on it,
		// because the stdlib doesn't actually provide any gurantees on what syscalls will be called.
		// (Not sure, what alternative it has though).
		unsafe {
			// must be in a local var in order to be not deallocated.
			let shmem_id_cstr =
				std::ffi::CString::new(shmem_id).expect("the shmmem id cannot have NUL in it; qed");

			if libc::shm_unlink(shmem_id_cstr.as_ptr()) == -1 {
				// failed to remove the shmem file nothing we can do ¯\_(ツ)_/¯
				log::warn!(
					target: LOG_TARGET,
					"failed to remove the shmem with id {}",
					shmem_id,
				);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use polkadot_core_primitives::OutboundHrmpMessage;

	use crate::primitives::BlockData;

	use super::*;
	use std::thread;

	#[test]
	fn wait_until_ready() {
		let host = create().unwrap();

		let worker_handle = thread::spawn({
			let id = host.id().to_string();
			move || {
				let worker = open(&id).unwrap();
				worker.signal_ready().unwrap();
			}
		});

		host.wait_until_ready(1).unwrap();

		worker_handle.join().unwrap();
	}

	#[test]
	fn wait_until_ready_timeout() {
		let host = create().unwrap();

		let _worker_handle = thread::spawn({
			let id = host.id().to_string();
			move || {
				let _worker = open(&id).unwrap();
			}
		});

		assert!(matches!(
			host.wait_until_ready(1),
			Err(WaitUntilReadyErr::Wait(_))
		));
	}

	#[test]
	fn open_junk_id() {
		assert!(open("").is_err());
		assert!(open("non_existent").is_err());
		assert!(open("☭").is_err());
	}

	#[test]
	fn attach_twice() {
		let host = create().unwrap();

		thread::spawn({
			let id = host.id().to_string();
			move || {
				let _worker1 = open(&id).unwrap();
				assert!(open(&id).is_err());
			}
		});
	}

	#[test]
	fn validation_works() {
		let mut host = create().unwrap();

		let worker_handle = thread::spawn({
			let id = host.id().to_string();
			move || {
				let mut worker = open(&id).unwrap();
				worker.signal_ready().unwrap();

				let work = worker.wait_for_work(3).unwrap();
				assert_eq!(work.code, b"\0asm\01\00\00\00");

				worker
					.report_result(ValidationResultHeader::Ok(ValidationResult {
						head_data: Default::default(),
						new_validation_code: None,
						upward_messages: vec![],
						horizontal_messages: vec![],
						processed_downward_messages: 322,
						hrmp_watermark: 0,
					}))
					.unwrap();
			}
		});

		host.wait_until_ready(1).unwrap();
		host.request_validation(
			b"\0asm\01\00\00\00",
			ValidationParams {
				parent_head: Default::default(),
				block_data: BlockData(b"hello world".to_vec()),
				relay_parent_number: 228,
				relay_parent_storage_root: Default::default(),
			},
		)
		.unwrap();

		match host.wait_for_result(3).unwrap() {
			ValidationResultHeader::Ok(r) => {
				assert_eq!(r.processed_downward_messages, 322);
			}
			_ => panic!(),
		}

		worker_handle.join().unwrap();
	}

	#[test]
	fn works_with_jumbo_sized_params() {
		let mut host = create().unwrap();

		let jumbo_code = vec![0x42; 16 * 1024 * 104];
		let fat_pov = vec![0x33; 16 * 1024 * 104];
		let big_params = ValidationParams {
			parent_head: Default::default(),
			block_data: BlockData(fat_pov),
			relay_parent_number: 228,
			relay_parent_storage_root: Default::default(),
			// If modifying please make sure that this has a big size.
		};
		let plump_result = ValidationResultHeader::Ok(ValidationResult {
			head_data: Default::default(),
			new_validation_code: Some(jumbo_code.clone().into()),
			processed_downward_messages: 322,
			hrmp_watermark: 0,
			// We don't know about the limits here. Just make sure that those are reasonably big.
			upward_messages: fill(|| vec![0x99; 8 * 1024], 64),
			horizontal_messages: fill(
				|| OutboundHrmpMessage {
					recipient: 1.into(),
					data: vec![0x11; 8 * 1024],
				},
				64,
			),
			// If modifying please make sure that this has a big size.
		});

		let _worker_handle = thread::spawn({
			let id = host.id().to_string();
			let jumbo_code = jumbo_code.clone();
			let big_params = big_params.clone();
			let plump_result = plump_result.clone();
			move || {
				let mut worker = open(&id).unwrap();
				worker.signal_ready().unwrap();

				let work = worker.wait_for_work(3).unwrap();
				assert_eq!(work.code, &jumbo_code);
				assert_eq!(work.params, &big_params.encode());

				worker.report_result(plump_result).unwrap();
			}
		});

		host.wait_until_ready(1).unwrap();
		host.request_validation(&jumbo_code, big_params).unwrap();

		assert_eq!(host.wait_for_result(3).unwrap(), plump_result);

		fn fill<T, F: Fn() -> T>(f: F, times: usize) -> Vec<T> {
			std::iter::repeat_with(f).take(times).collect()
		}
	}
}
