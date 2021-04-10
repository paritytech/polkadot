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

#![warn(missing_docs)]

//! A crate that implements PVF validation host.
//!
//! This crate provides a simple API. You first [`start`] the validation host, which gives you the
//! [handle][`ValidationHost`] and the future you need to poll.
//!
//! Then using the handle the client can send two types of requests:
//!
//! (a) PVF execution. This accepts the PVF [params][`polkadot_parachain::primitives::ValidationParams`]
//!     and the PVF [code][`Pvf`], prepares (verifies and compiles) the code, and then executes PVF
//!     with the params.
//!
//! (b) Heads up. This request allows to signal that the given PVF may be needed soon and that it
//!     should be prepared for execution.
//!
//! The preparation results are cached for some time after they either used or was signalled in heads up.
//! All requests that depends on preparation of the same PVF are bundled together and will be executed
//! as soon as the artifact is prepared.
//!
//! # Priority
//!
//! PVF execution requests can specify the [priority][`Priority`] with which the given request should
//! be handled. Different priority levels have different effects. This is discussed below.
//!
//! Preparation started by a heads up signal always starts in with the background priority. If there
//! is already a request for that PVF preparation under way the priority is inherited. If after heads
//! up, a new PVF execution request comes in with a higher priority, then the original task's priority
//! will be adjusted to match the new one if it's larger.
//!
//! Priority can never go down, only up.
//!
//! # Under the hood
//!
//! Under the hood, the validation host is built using a bunch of communicating processes, not
//! dissimilar to actors. Each of such "processes" is a future task that contains an event loop that
//! processes incoming messages, potentially delegating sub-tasks to other "processes".
//!
//! Two of these processes are queues. The first one is for preparation jobs and the second one is for
//! execution. Both of the queues are backed by separate pools of workers of different kind.
//!
//! Preparation workers handle preparation requests by preverifying and instrumenting PVF wasm code,
//! and then passing it into the compiler, to prepare the artifact.
//!
//! Artifact is a final product of preparation. If the preparation succeeded, then the artifact will
//! contain the compiled code usable for quick execution by a worker later on.
//!
//! If the preparation failed, then the worker will still write the artifact with the error message.
//! We save the artifact with the error so that we don't try to prepare the artifacts that are broken
//! repeatedly.
//!
//! The artifact is saved on disk and is also tracked by an in memory table. This in memory table
//! doesn't contain the artifact contents though, only a flag that the given artifact is compiled.
//!
//! The execute workers will be fed by the requests from the execution queue, which is basically a
//! combination of a path to the compiled artifact and the
//! [params][`polkadot_parachain::primitives::ValidationParams`].
//!
//! Each fixed interval of time a pruning task will run. This task will remove all artifacts that
//! weren't used or received a heads up signal for a while.

mod artifacts;
mod error;
mod execute;
mod executor_intf;
mod host;
mod prepare;
mod priority;
mod pvf;
mod worker_common;

#[doc(hidden)]
pub mod testing;

pub use error::{ValidationError, InvalidCandidate};
pub use priority::Priority;
pub use pvf::Pvf;

pub use host::{start, Config, ValidationHost};

pub use execute::worker_entrypoint as execute_worker_entrypoint;
pub use prepare::worker_entrypoint as prepare_worker_entrypoint;

const LOG_TARGET: &str = "parachain::pvf";
