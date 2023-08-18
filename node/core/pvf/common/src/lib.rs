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

//! Contains functionality related to PVFs that is shared by the PVF host and the PVF workers.

pub mod error;
pub mod execute;
pub mod executor_intf;
pub mod prepare;
pub mod pvf;
pub mod worker;

pub use cpu_time::ProcessTime;

// Used by `decl_worker_main!`.
#[cfg(feature = "test-utils")]
pub use sp_tracing;

const LOG_TARGET: &str = "parachain::pvf-common";

use std::mem;
use tokio::io::{self, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

#[cfg(feature = "test-utils")]
pub mod tests {
	use std::time::Duration;

	pub const TEST_EXECUTION_TIMEOUT: Duration = Duration::from_secs(3);
	pub const TEST_PREPARATION_TIMEOUT: Duration = Duration::from_secs(30);
}

/// Write some data prefixed by its length into `w`.
pub async fn framed_send(w: &mut (impl AsyncWrite + Unpin), buf: &[u8]) -> io::Result<()> {
	let len_buf = buf.len().to_le_bytes();
	w.write_all(&len_buf).await?;
	w.write_all(buf).await?;
	Ok(())
}

/// Read some data prefixed by its length from `r`.
pub async fn framed_recv(r: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
	let mut len_buf = [0u8; mem::size_of::<usize>()];
	r.read_exact(&mut len_buf).await?;
	let len = usize::from_le_bytes(len_buf);
	let mut buf = vec![0; len];
	r.read_exact(&mut buf).await?;
	Ok(buf)
}
