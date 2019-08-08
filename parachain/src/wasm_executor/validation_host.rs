// Copyright 2019 Parity Technologies (UK) Ltd.
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

#![cfg(not(target_os = "unknown"))]

use std::{process, env, sync::Arc, sync::atomic};
use crate::codec::{Decode, Encode};
use crate::{ValidationParams, ValidationResult, MessageRef, UpwardMessageRef, UpwardMessage, IncomingMessage};
use super::{validate_candidate_internal, Error, Externalities, WorkerExternalities};
use super::{MAX_CODE_MEM, MAX_RUNTIME_MEM};
use shared_memory::{SharedMem, SharedMemConf, EventState, WriteLockable, EventWait, EventSet};
use parking_lot::Mutex;
use log::{debug, trace};

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
	pub static ref HOST: Mutex<ValidationHost> = Mutex::new(ValidationHost::new());
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
					.map_err(|_| format!("Error decoding validation request."))?;
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

unsafe impl Send for ValidationHost {}

pub struct ValidationHost {
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
	pub fn validate_candidate<E: Externalities>(
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
