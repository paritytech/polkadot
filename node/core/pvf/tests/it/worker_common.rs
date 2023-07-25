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

use crate::PUPPET_EXE_PATH;
use polkadot_node_core_pvf::testing::{spawn_job_with_worker_source, JobKind, SpawnErr, WorkerSource};
use polkadot_node_core_pvf_prepare_worker::PREPARE_EXE;
use std::time::Duration;

// Test spawning a program that immediately exits with a failure code.
#[tokio::test]
async fn spawn_immediate_exit() {
	let result = spawn_job_with_worker_source(
		&JobKind::IntegrationTest,
		WorkerSource::ProgramPath(PUPPET_EXE_PATH.into()),
		&["exit"],
		Duration::from_secs(2),
	)
	.await;
	assert!(matches!(result, Err(SpawnErr::AcceptTimeout)));
}

#[tokio::test]
async fn spawn_timeout() {
	let result = spawn_job_with_worker_source(
		&JobKind::IntegrationTest,
		WorkerSource::ProgramPath(PUPPET_EXE_PATH.into()),
		&["sleep"],
		Duration::from_secs(2),
	)
	.await;
	assert!(matches!(result, Err(SpawnErr::AcceptTimeout)));
}

#[tokio::test]
async fn should_connect() {
	let _ = spawn_job_with_worker_source(
		&JobKind::IntegrationTest,
		WorkerSource::ProgramPath(PUPPET_EXE_PATH.into()),
		&["prepare-worker"],
		Duration::from_secs(2),
	)
	.await
	.unwrap();
}

#[tokio::test]
async fn should_connect_to_in_memory_binary() {
	sp_tracing::init_for_tests();
	let _ = spawn_job_with_worker_source(
		&JobKind::Prepare,
		WorkerSource::InMemoryBytes(PREPARE_EXE),
		&["prepare-worker"],
		Duration::from_secs(2),
	)
	.await
	.unwrap();
}
