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

//! This module provides utilities for sandboxing PVF jobs. This is necessary because jobs are
//! compiling or executing untrusted code, and in the case of an arbitrary code execution attack we
//! want to limit how much access an attacker can get to the rest of the system.
//!
//! # Details
//!
//! We are currently sandboxing by putting the PVF jobs in a "seccomp jail". We run `seccomp` (only
//! on the thread that does preparation/execution) and have it block all syscalls, except for the
//! minimum required by a legitimate job.
//!
//! ## Action on Caught Syscall
//!
//! Right now we will just log syscalls that violate the filter, but eventually we will have to
//! actually block them.
//!
//! TODO: Update when we change the action and explain why given action was chosen.
//!
//! # Compatibility
//!
//! Systems like MacOS do not support `seccomp`, so a validator cannot run on them securely.
//!
//! TODO: Mention "insecure mode" once that is implemented.

// TODO: Document how to get the allowed set of syscalls.

use seccompiler::*;
use std::collections::BTreeMap;

#[derive(thiserror::Error, Debug)]
pub enum SandboxError {
	#[error(transparent)]
	Seccomp(#[from] seccompiler::Error),

	#[error(transparent)]
	Backend(#[from] seccompiler::BackendError),

	#[error(transparent)]
	Io(#[from] std::io::Error),
}

pub type SandboxResult<T> = std::result::Result<T, SandboxError>;

// TODO: Should be tested on multiple allowed architectures in case some use different syscalls.
// TODO: Compile the filter at build-time rather than runtime.
/// Applies a `seccomp` filter to sandbox the execute thread. Whitelists the minimum number of
/// syscalls necessary to allow execution to run. See the module-level docs for more details.
pub fn seccomp_execute_thread() -> SandboxResult<()> {
	// Build a `seccomp` filter which by default blocks all syscalls except those allowed in the
	// whitelist.
	let whitelisted_rules = BTreeMap::default();
	let filter = SeccompFilter::new(
		whitelisted_rules,
		// Mismatch action: what to do if not in whitelist.
		SeccompAction::Log,
		// Match action: what to do if in whitelist.
		SeccompAction::Allow,
		std::env::consts::ARCH.try_into().unwrap(),
	)?;

	// TODO: Check if action available?

	let bpf_prog: BpfProgram = filter.try_into()?;

	// Applies filter (runs seccomp) to the calling thread.
	seccompiler::apply_filter(&bpf_prog)?;

	Ok(())
}

/// Applies a `seccomp` filter to sandbox the prepare thread. Whitelists the minimum number of
/// syscalls necessary to allow preparation to run. See the module-level docs for more details.
pub fn seccomp_prepare_thread() -> SandboxResult<()> {
	// Build a `seccomp` filter which by default blocks all syscalls except those allowed in the
	// whitelist.
	let whitelisted_rules = BTreeMap::default();
	let filter = SeccompFilter::new(
		whitelisted_rules,
		// Mismatch action: what to do if not in whitelist.
		SeccompAction::Log,
		// Match action: what to do if in whitelist.
		SeccompAction::Allow,
		std::env::consts::ARCH.try_into().unwrap(),
	)?;

	// TODO: Check if action available?

	let bpf_prog: BpfProgram = filter.try_into()?;

	// Applies filter (runs seccomp) to the calling thread.
	seccompiler::apply_filter(&bpf_prog)?;

	Ok(())
}
