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

#![deny(unused_crate_dependencies)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro)]

//! A wrapper around `tracing` macros, to provide semi automatic
//! `traceID` annotation without codebase turnover.
//!
//! # Usage
//!
//! The API follows the [`tracing`
//! API](https://docs.rs/tracing/latest/tracing/index.html), but the docs contain
//! more detail than you probably need to know, so here's the quick version.
//!
//! Most common usage is of the form:
//!
//! ```rs
//! gum::warn!(
//!     target: LOG_TARGET,
//!     worker_pid = %idle_worker.pid,
//!     ?error,
//!     "failed to send a handshake to the spawned worker",
//! );
//! ```
//!
//! ### Log levels
//!
//! All of the the [`tracing` macros](https://docs.rs/tracing/latest/tracing/index.html#macros) are available.
//! In decreasing order of priority they are:
//!
//! - `error!`
//! - `warn!`
//! - `info!`
//! - `debug!`
//! - `trace!`
//!
//! ### `target`
//!
//! The `LOG_TARGET` should be defined once per crate, e.g.:
//!
//! ```rs
//! const LOG_TARGET: &str = "parachain::pvf";
//! ```
//!
//! This should be of the form `<target>::<subtarget>`, where the `::<subtarget>` is optional.
//!
//! The target and subtarget are used when debugging by specializing the Grafana Loki query to
//! filter specific subsystem logs. The more specific the query is the better when approaching the
//! query response limit.
//!
//! ### Fields
//!
//! Here's the rundown on how fields work:
//!
//! - Fields on spans and events are specified using the `syntax field_name = field_value`.
//! - Local variables may be used as field values without an assignment, similar to struct
//!   initializers.
//! - The `?` sigil is shorthand that specifies a field should be recorded using its `fmt::Debug`
//!   implementation.
//! - The `%` sigil operates similarly, but indicates that the value should be recorded using its
//!   `fmt::Display` implementation.
//!
//! For full details, again see [the tracing
//! docs](https://docs.rs/tracing/latest/tracing/index.html#recording-fields).
//!
//! ### Viewing traces
//!
//! When testing,
//!
//! ```rs
//! sp_tracing::init_for_tests();
//! ```
//!
//! should enable all trace logs.
//!
//! Alternatively, you can do:
//!
//! ```rs
//! sp_tracing::try_init_simple();
//! ```
//!
//! On the command line you specify `RUST_LOG` with the desired target and trace level:
//!
//! ```sh
//! RUST_LOG=parachain::pvf=trace cargo test
//! ```
//!
//! On the other hand if you want all `parachain` logs, specify `parachain=trace`, which will also
//! include logs from `parachain::pvf` and other subtargets.

pub use tracing::{enabled, event, Level};

#[doc(hidden)]
pub use jaeger::hash_to_trace_identifier;

#[doc(hidden)]
pub use polkadot_primitives::{CandidateHash, Hash};

pub use gum_proc_macro::{debug, error, info, trace, warn, warn_if_frequent};

#[cfg(test)]
mod tests;

const FREQ_SMOOTHING_FACTOR: f32 = 0.5;

/// Exponential moving average
#[derive(Debug, Default)]
struct EmaBucket {
	current: f32,
	count: u32,
}

impl EmaBucket {
	fn update(&mut self, value: f32, alpha: f32) {
		if self.count == 0 {
			self.current = value;
		} else {
			self.current += alpha * (value - self.current);
		}
		self.count += 1;
	}
}

/// Utility struct to compare the rate of its own calls.
pub struct Freq {
	ema: EmaBucket,
	last: u64,
}

impl Freq {
	/// Initiates a new instance
	pub fn new() -> Self {
		Self { ema: Default::default(), last: Default::default() }
	}

	/// Compares the rate of its own calls with the passed one.
	pub fn is_frequent(&mut self, max_rate: Times) -> bool {
		self.record();

		// Two attempts is not enough to call something as frequent.
		if self.ema.count < 3 {
			return false
		}

		let rate = 1000.0 / self.ema.current; // Current EMA represents interval in ms
		rate > max_rate.into()
	}

	fn record(&mut self) {
		let now = coarsetime::Clock::now_since_epoch().as_millis() as u64;
		if self.last > 0 {
			self.ema.update((now - self.last) as f32, FREQ_SMOOTHING_FACTOR);
		}
		self.last = now;
	}
}

/// Represents frequency per second, minute, hour and day
pub enum Times {
	/// Per second
	PerSecond(u32),
	/// Per minute
	PerMinute(u32),
	/// Per hour
	PerHour(u32),
	/// Per day
	PerDay(u32),
}

impl From<Times> for f32 {
	fn from(value: Times) -> Self {
		match value {
			Times::PerSecond(v) => v as f32,
			Times::PerMinute(v) => v as f32 / 60.0,
			Times::PerHour(v) => v as f32 / (60.0 * 60.0),
			Times::PerDay(v) => v as f32 / (60.0 * 60.0 * 24.0),
		}
	}
}
