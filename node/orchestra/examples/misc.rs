// Copyright (C) 2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use orchestra::{Spawner, *};

#[derive(Debug, Clone, Copy)]
pub enum SigSigSig {
	Conclude,
	Foo,
}

#[derive(Debug, Clone)]
pub struct DummySpawner;

impl Spawner for DummySpawner {
	fn spawn_blocking(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		unimplemented!("spawn blocking {} {}", task_name, subsystem_name.unwrap_or("default"))
	}

	fn spawn(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		unimplemented!("spawn {} {}", task_name, subsystem_name.unwrap_or("default"))
	}
}

/// The external event.
#[derive(Debug, Clone)]
pub struct EvX;

impl EvX {
	pub fn focus<'a, T>(&'a self) -> Result<EvX, ()> {
		unimplemented!("focus")
	}
}

#[derive(Debug, Clone, Copy)]
pub struct Yikes;

impl std::fmt::Display for Yikes {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		writeln!(f, "yikes!")
	}
}

impl std::error::Error for Yikes {}

impl From<orchestra::OrchestraError> for Yikes {
	fn from(_: orchestra::OrchestraError) -> Yikes {
		Yikes
	}
}

impl From<orchestra::mpsc::SendError> for Yikes {
	fn from(_: orchestra::mpsc::SendError) -> Yikes {
		Yikes
	}
}

#[derive(Debug, Clone)]
pub struct MsgStrukt(pub u8);

#[derive(Debug, Clone, Copy)]
pub struct Plinko;
