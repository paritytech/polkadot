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

//! Utilities for general error handling in Polkadot.
//!
//! Goals:
//!
//! - Ergonomic API with little repetition.
//! - Still explicitness where it matters - fatal errors should be visible and justified.
//! - Easy recovering from non fatal errors.
//! - Errors start as non fatal and can be made fatal at the level where it is really clear they
//!	  are fatal. E.g. cancellation of a oneshot might be fatal in one case, but absolutely expected
//!	  in another.
//! - Good error messages. Fatal errors don't need to be properly structured (as we won't handle
//!   them), but should provide good error messages of what is going on.
//! - Encourage many error types. One per module or even per function is totally fine - it makes
//!   error handling robust, if you only need to handle errors that can actually happen, also error
//!   messages will get better.

use thiserror::Error;

/// Error abstraction.
///
/// Errors might either be fatal and should bring the subsystem down or are at least at the point
/// of occurrence deemed potentially recoverable.
///
/// Upper layers might have a better view and might make a non fatal error of a called function a
/// fatal one. The opposite should not happen, therefore don't make an error fatal if you don't
/// know it is in all cases.
///
/// Usage pattern:
///
/// ```
/// # use thiserror::Error;
/// # use polkadot_node_subsystem::errors::RuntimeApiError;
/// # use polkadot_primitives::v1::SessionIndex;
/// # use futures::channel::oneshot;
/// # use polkadot_node_subsystem_util::Err;
/// type Error = Err<NonFatal, Fatal>;
///
/// impl From<NonFatal> for Error {
///   	fn from(e: NonFatal) -> Self {
///   		Self::from_non_fatal(e)
///  	}
/// }
///
/// impl From<Fatal> for Error {
/// 	fn from(f: Fatal) -> Self {
/// 		Self::from_fatal(f)
/// 	}
/// }
///
/// #[derive(Debug, Error)]
/// pub enum Fatal {
///		/// Runtime API subsystem is down, which means we're shutting down.
///		#[error("Runtime request got canceled - system is shutting down.")]
///		RuntimeRequestCanceled(oneshot::Canceled),
/// }
///
/// #[derive(Debug, Error)]
/// pub enum NonFatal {
///		/// Some request to the runtime failed.
///		/// For example if we prune a block we're requesting info about.
///		#[error("Runtime API error")]
///		RuntimeRequest(RuntimeApiError),

///		/// We tried fetching a session info which was not available.
///		#[error("There was no session with the given index")]
///		NoSuchSession(SessionIndex),
/// }
/// ```
/// Then mostly use `Error` in functions, you may also use `NonFatal` and `Fatal` directly in
/// functions that strictly only fail non fatal or fatal respectively, as `Fatal` and `NonFatal`
/// can automatically converted into the above defined `Error`.
/// ```
#[derive(Debug, Error)]
pub enum Err<E, F>
	where
		E: std::fmt::Debug + std::error::Error + 'static,
		F: std::fmt::Debug + std::error::Error + 'static, {
	/// Error is fatal and should be escalated up.
	///
	/// While we usually won't want to pattern match on those, a concrete descriptive enum might
	/// still be a good idea for easy auditing of what can go wrong in a module and also makes for
	/// good error messages thanks to `thiserror`.
	#[error("Fatal error occurred.")]
	Fatal(#[source] F),
	/// Error that is not fatal, at least not yet at this level of execution.
	#[error("Non fatal error occurred.")]
	Err(#[source] E),
}

/// Due to typesystem constraints we cannot implement the following methods as standard
/// `From::from` implementations. So no auto conversions by default, a simple `Result::map_err` is
/// not too bad though.
impl<E, F> Err<E, F>
	where
		E: std::fmt::Debug + std::error::Error + 'static,
		F: std::fmt::Debug + std::error::Error + 'static,
{
	/// Build an `Err` from compatible fatal error.
	pub fn from_fatal<F1: Into<F>>(f: F1) -> Self {
		Self::Fatal(f.into())
	}

	/// Build an `Err` from compatible non fatal error.
	pub fn from_non_fatal<E1: Into<E>>(e: E1) -> Self {
		Self::Err(e.into())
	}

	/// Build an `Err` from a compatible other `Err`.
	pub fn from_other<E1, F1>(e: Err<E1, F1>) -> Self
	where
		E1: Into<E> + std::fmt::Debug + std::error::Error + 'static,
		F1: Into<F> + std::fmt::Debug + std::error::Error + 'static,
	{
		match e {
			Err::Fatal(f) => Self::from_fatal(f),
			Err::Err(e) => Self::from_non_fatal(e),
		}
	}
}

/// Unwrap non fatal error and report fatal one.
///
/// This function is useful for top level error handling. Fatal errors will be extracted,
/// non fatal error will be returned for handling.
///
/// Usage:
///
/// ```no_run
/// # use thiserror::Error;
/// # use polkadot_node_subsystem_util::{Err, unwrap_non_fatal};
/// # use polkadot_node_subsystem::SubsystemError;
/// # #[derive(Error, Debug)]
/// # enum Fatal {
/// # }
/// # #[derive(Error, Debug)]
/// # enum NonFatal {
/// # }
/// # fn computation() -> Result<(), Err<NonFatal, Fatal>> {
/// # 	panic!();
/// # }
/// #
/// // Use run like so:
/// //	run(ctx)
/// //		.map_err(|e| SubsystemError::with_origin("subsystem-name", e))
/// fn run() -> std::result::Result<(), Fatal> {
///		loop {
///			// ....
///			if let Some(err) = unwrap_non_fatal(computation())? {
///				println!("Something bad happened: {}", err);
///				continue
///			}
///		}
/// }
///
/// ```
pub fn unwrap_non_fatal<E,F>(result: Result<(), Err<E,F>>) -> Result<Option<E>, F>
	where
		E: std::fmt::Debug + std::error::Error + 'static,
		F: std::fmt::Debug + std::error::Error + Send + Sync + 'static
{
	match result {
		Ok(()) => Ok(None),
		Err(Err::Fatal(f)) => Err(f),
		Err(Err::Err(e)) => Ok(Some(e)),
	}
}
