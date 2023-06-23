// Copyright (C) Parity Technologies (UK) Ltd.
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

use crate::PUPPET_EXE;
use polkadot_node_core_pvf::testing::{spawn_with_program_path, SpawnErr};
use polkadot_node_core_pvf_common::{
	execute::Response,
	worker::{
		cpu_time_monitor_loop, stringify_panic_payload,
		thread::{get_condvar, spawn_worker_thread, wait_for_threads_with_timeout, WaitOutcome},
	},
	ProcessTime,
};
use std::{
	ops::{Add, Sub},
	sync::{mpsc::channel, Arc, Condvar, Mutex},
	time::{Duration, Instant, SystemTime},
};

// Test spawning a program that immediately exits with a failure code.
#[tokio::test]
async fn spawn_immediate_exit() {
	let result =
		spawn_with_program_path("integration-test", PUPPET_EXE, &["exit"], Duration::from_secs(2))
			.await;
	assert!(matches!(result, Err(SpawnErr::AcceptTimeout)));
}

#[tokio::test]
async fn spawn_timeout() {
	let result =
		spawn_with_program_path("integration-test", PUPPET_EXE, &["sleep"], Duration::from_secs(2))
			.await;
	assert!(matches!(result, Err(SpawnErr::AcceptTimeout)));
}

#[tokio::test]
async fn should_connect() {
	let _ = spawn_with_program_path(
		"integration-test",
		PUPPET_EXE,
		&["prepare-worker"],
		Duration::from_secs(2),
	)
	.await
	.unwrap();
}

#[test]
fn get_condvar_should_be_pending() {
	let condvar = get_condvar();
	let outcome = *condvar.0.lock().unwrap();
	assert!(outcome.is_pending());
}

#[test]
fn wait_for_threads_with_timeout_return_none_on_time_out() {
	let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
	let outcome = wait_for_threads_with_timeout(&condvar, Duration::new(1, 0));
	assert!(matches!(outcome, None));
}

#[test]
fn wait_for_threads_with_timeout_returns_outcome() {
	let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
	let condvar2 = Arc::clone(&condvar);
	std::thread::spawn(move || {
		let (lock, cvar) = &*condvar2;
		let mut outcome = lock.lock().unwrap();
		*outcome = WaitOutcome::Finished;
		cvar.notify_one();
	});
	let outcome = wait_for_threads_with_timeout(&condvar, Duration::from_secs(2));
	assert!(matches!(outcome.unwrap(), WaitOutcome::Finished));
}

#[test]
fn spawn_worker_thread_should_notify_on_done() {
	let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
	let response =
		spawn_worker_thread("thread", move || 2, Arc::clone(&condvar), WaitOutcome::TimedOut);
	let (lock, _) = &*condvar;
	let r = response.expect("").join().unwrap();
	assert!(matches!(r, 2));
	assert!(matches!(*lock.lock().unwrap(), WaitOutcome::TimedOut));
}

#[test]
fn spawn_worker_thread_should_not_notify() {
	let condvar = Arc::new((Mutex::new(WaitOutcome::Finished), Condvar::new()));
	let response =
		spawn_worker_thread("thread", move || 2, Arc::clone(&condvar), WaitOutcome::TimedOut);
	let (lock, _) = &*condvar;
	let r = response.unwrap().join().unwrap();
	assert!(matches!(r, 2));
	assert!(matches!(*lock.lock().unwrap(), WaitOutcome::Finished));
}

#[test]
fn cpu_time_monitor_loop_should_return_time_elapsed() {
	let cpu_time_start = ProcessTime::now();
	let timeout = Duration::from_secs(0);
	let (_tx, rx) = channel();
	let result = cpu_time_monitor_loop(cpu_time_start, timeout, rx);
	assert_ne!(result, None);
}

#[test]
fn cpu_time_monitor_loop_should_return_none() {
	let cpu_time_start = ProcessTime::now();
	let timeout = Duration::from_secs(10);
	let (tx, rx) = channel();
	tx.send(()).unwrap();
	let result = cpu_time_monitor_loop(cpu_time_start, timeout, rx);
	assert_eq!(result, None);
}
