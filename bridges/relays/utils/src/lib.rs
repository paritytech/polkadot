// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Utilities used by different relays.

use backoff::{backoff::Backoff, ExponentialBackoff};
use futures::future::FutureExt;
use std::time::Duration;

/// Max delay after connection-unrelated error happened before we'll try the
/// same request again.
pub const MAX_BACKOFF_INTERVAL: Duration = Duration::from_secs(60);
/// Delay after connection-related error happened before we'll try
/// reconnection again.
pub const CONNECTION_ERROR_DELAY: Duration = Duration::from_secs(10);

pub mod initialize;
pub mod metrics;
pub mod relay_loop;

/// Block number traits shared by all chains that relay is able to serve.
pub trait BlockNumberBase:
	'static
	+ From<u32>
	+ Into<u64>
	+ Ord
	+ Clone
	+ Copy
	+ Default
	+ Send
	+ Sync
	+ std::fmt::Debug
	+ std::fmt::Display
	+ std::hash::Hash
	+ std::ops::Add<Output = Self>
	+ std::ops::Sub<Output = Self>
	+ num_traits::CheckedSub
	+ num_traits::Saturating
	+ num_traits::Zero
	+ num_traits::One
{
}

impl<T> BlockNumberBase for T where
	T: 'static
		+ From<u32>
		+ Into<u64>
		+ Ord
		+ Clone
		+ Copy
		+ Default
		+ Send
		+ Sync
		+ std::fmt::Debug
		+ std::fmt::Display
		+ std::hash::Hash
		+ std::ops::Add<Output = Self>
		+ std::ops::Sub<Output = Self>
		+ num_traits::CheckedSub
		+ num_traits::Saturating
		+ num_traits::Zero
		+ num_traits::One
{
}

/// Macro that returns (client, Err(error)) tuple from function if result is Err(error).
#[macro_export]
macro_rules! bail_on_error {
	($result: expr) => {
		match $result {
			(client, Ok(result)) => (client, result),
			(client, Err(error)) => return (client, Err(error)),
		}
	};
}

/// Macro that returns (client, Err(error)) tuple from function if result is Err(error).
#[macro_export]
macro_rules! bail_on_arg_error {
	($result: expr, $client: ident) => {
		match $result {
			Ok(result) => result,
			Err(error) => return ($client, Err(error)),
		}
	};
}

/// Ethereum header Id.
#[derive(Debug, Default, Clone, Copy, Eq, Hash, PartialEq)]
pub struct HeaderId<Hash, Number>(pub Number, pub Hash);

/// Error type that can signal connection errors.
pub trait MaybeConnectionError {
	/// Returns true if error (maybe) represents connection error.
	fn is_connection_error(&self) -> bool;
}

/// Stringified error that may be either connection-related or not.
#[derive(Debug)]
pub enum StringifiedMaybeConnectionError {
	/// The error is connection-related error.
	Connection(String),
	/// The error is connection-unrelated error.
	NonConnection(String),
}

impl StringifiedMaybeConnectionError {
	/// Create new stringified connection error.
	pub fn new(is_connection_error: bool, error: String) -> Self {
		if is_connection_error {
			StringifiedMaybeConnectionError::Connection(error)
		} else {
			StringifiedMaybeConnectionError::NonConnection(error)
		}
	}
}

impl MaybeConnectionError for StringifiedMaybeConnectionError {
	fn is_connection_error(&self) -> bool {
		match *self {
			StringifiedMaybeConnectionError::Connection(_) => true,
			StringifiedMaybeConnectionError::NonConnection(_) => false,
		}
	}
}

impl ToString for StringifiedMaybeConnectionError {
	fn to_string(&self) -> String {
		match *self {
			StringifiedMaybeConnectionError::Connection(ref err) => err.clone(),
			StringifiedMaybeConnectionError::NonConnection(ref err) => err.clone(),
		}
	}
}

/// Exponential backoff for connection-unrelated errors retries.
pub fn retry_backoff() -> ExponentialBackoff {
	ExponentialBackoff {
		// we do not want relayer to stop
		max_elapsed_time: None,
		max_interval: MAX_BACKOFF_INTERVAL,
		..Default::default()
	}
}

/// Compact format of IDs vector.
pub fn format_ids<Id: std::fmt::Debug>(mut ids: impl ExactSizeIterator<Item = Id>) -> String {
	const NTH_PROOF: &str = "we have checked len; qed";
	match ids.len() {
		0 => "<nothing>".into(),
		1 => format!("{:?}", ids.next().expect(NTH_PROOF)),
		2 => {
			let id0 = ids.next().expect(NTH_PROOF);
			let id1 = ids.next().expect(NTH_PROOF);
			format!("[{:?}, {:?}]", id0, id1)
		}
		len => {
			let id0 = ids.next().expect(NTH_PROOF);
			let id_last = ids.last().expect(NTH_PROOF);
			format!("{}:[{:?} ... {:?}]", len, id0, id_last)
		}
	}
}

/// Stream that emits item every `timeout_ms` milliseconds.
pub fn interval(timeout: Duration) -> impl futures::Stream<Item = ()> {
	futures::stream::unfold((), move |_| async move {
		async_std::task::sleep(timeout).await;
		Some(((), ()))
	})
}

/// Which client has caused error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FailedClient {
	/// It is the source client who has caused error.
	Source,
	/// It is the target client who has caused error.
	Target,
	/// Both clients are failing, or we just encountered some other error that
	/// should be treated like that.
	Both,
}

/// Future process result.
#[derive(Debug, Clone, Copy)]
pub enum ProcessFutureResult {
	/// Future has been processed successfully.
	Success,
	/// Future has failed with non-connection error.
	Failed,
	/// Future has failed with connection error.
	ConnectionFailed,
}

impl ProcessFutureResult {
	/// Returns true if result is Success.
	pub fn is_ok(self) -> bool {
		match self {
			ProcessFutureResult::Success => true,
			ProcessFutureResult::Failed | ProcessFutureResult::ConnectionFailed => false,
		}
	}

	/// Returns Ok(true) if future has succeeded.
	/// Returns Ok(false) if future has failed with non-connection error.
	/// Returns Err if future is `ConnectionFailed`.
	pub fn fail_if_connection_error(self, failed_client: FailedClient) -> Result<bool, FailedClient> {
		match self {
			ProcessFutureResult::Success => Ok(true),
			ProcessFutureResult::Failed => Ok(false),
			ProcessFutureResult::ConnectionFailed => Err(failed_client),
		}
	}
}

/// Process result of the future from a client.
pub fn process_future_result<TResult, TError, TGoOfflineFuture>(
	result: Result<TResult, TError>,
	retry_backoff: &mut ExponentialBackoff,
	on_success: impl FnOnce(TResult),
	go_offline_future: &mut std::pin::Pin<&mut futures::future::Fuse<TGoOfflineFuture>>,
	go_offline: impl FnOnce(Duration) -> TGoOfflineFuture,
	error_pattern: impl FnOnce() -> String,
) -> ProcessFutureResult
where
	TError: std::fmt::Debug + MaybeConnectionError,
	TGoOfflineFuture: FutureExt,
{
	match result {
		Ok(result) => {
			on_success(result);
			retry_backoff.reset();
			ProcessFutureResult::Success
		}
		Err(error) if error.is_connection_error() => {
			log::error!(
				target: "bridge",
				"{}: {:?}. Going to restart",
				error_pattern(),
				error,
			);

			retry_backoff.reset();
			go_offline_future.set(go_offline(CONNECTION_ERROR_DELAY).fuse());
			ProcessFutureResult::ConnectionFailed
		}
		Err(error) => {
			let retry_delay = retry_backoff.next_backoff().unwrap_or(CONNECTION_ERROR_DELAY);
			log::error!(
				target: "bridge",
				"{}: {:?}. Retrying in {}",
				error_pattern(),
				error,
				retry_delay.as_secs_f64(),
			);

			go_offline_future.set(go_offline(retry_delay).fuse());
			ProcessFutureResult::Failed
		}
	}
}
