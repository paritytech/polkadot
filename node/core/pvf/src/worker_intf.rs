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

//! Common logic for implementation of worker processes.

use crate::LOG_TARGET;
use futures::FutureExt as _;
use futures_timer::Delay;
use pin_project::pin_project;
use rand::Rng;
use std::{
	fmt, mem,
	path::{Path, PathBuf},
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};
use tokio::{
	io::{self, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadBuf},
	net::{UnixListener, UnixStream},
	process,
};

/// A multiple of the job timeout (in CPU time) for which we are willing to wait on the host (in
/// wall clock time). This is lenient because CPU time may go slower than wall clock time.
pub const JOB_TIMEOUT_WALL_CLOCK_FACTOR: u32 = 4;

/// This is publicly exposed only for integration tests.
#[doc(hidden)]
pub async fn spawn_with_program_path(
	debug_id: &'static str,
	program_path: impl Into<PathBuf>,
	extra_args: &'static [&'static str],
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	let program_path = program_path.into();
	with_transient_socket_path(debug_id, |socket_path| {
		let socket_path = socket_path.to_owned();
		async move {
			let listener = UnixListener::bind(&socket_path).map_err(|err| {
				gum::warn!(
					target: LOG_TARGET,
					%debug_id,
					?program_path,
					?extra_args,
					"cannot bind unix socket: {:?}",
					err,
				);
				SpawnErr::Bind
			})?;

			let handle =
				WorkerHandle::spawn(&program_path, extra_args, socket_path).map_err(|err| {
					gum::warn!(
						target: LOG_TARGET,
						%debug_id,
						?program_path,
						?extra_args,
						"cannot spawn a worker: {:?}",
						err,
					);
					SpawnErr::ProcessSpawn
				})?;

			futures::select! {
				accept_result = listener.accept().fuse() => {
					let (stream, _) = accept_result.map_err(|err| {
						gum::warn!(
							target: LOG_TARGET,
							%debug_id,
							?program_path,
							?extra_args,
							"cannot accept a worker: {:?}",
							err,
						);
						SpawnErr::Accept
					})?;
					Ok((IdleWorker { stream, pid: handle.id() }, handle))
				}
				_ = Delay::new(spawn_timeout).fuse() => {
					gum::warn!(
						target: LOG_TARGET,
						%debug_id,
						?program_path,
						?extra_args,
						?spawn_timeout,
						"spawning and connecting to socket timed out",
					);
					Err(SpawnErr::AcceptTimeout)
				}
			}
		}
	})
	.await
}

async fn with_transient_socket_path<T, F, Fut>(debug_id: &'static str, f: F) -> Result<T, SpawnErr>
where
	F: FnOnce(&Path) -> Fut,
	Fut: futures::Future<Output = Result<T, SpawnErr>> + 'static,
{
	let socket_path = tmpfile(&format!("pvf-host-{}", debug_id))
		.await
		.map_err(|_| SpawnErr::TmpFile)?;
	let result = f(&socket_path).await;

	// Best effort to remove the socket file. Under normal circumstances the socket will be removed
	// by the worker. We make sure that it is removed here, just in case a failed rendezvous.
	let _ = tokio::fs::remove_file(socket_path).await;

	result
}

/// Returns a path under the given `dir`. The file name will start with the given prefix.
///
/// There is only a certain number of retries. If exceeded this function will give up and return an
/// error.
pub async fn tmpfile_in(prefix: &str, dir: &Path) -> io::Result<PathBuf> {
	fn tmppath(prefix: &str, dir: &Path) -> PathBuf {
		use rand::distributions::Alphanumeric;

		const DESCRIMINATOR_LEN: usize = 10;

		let mut buf = Vec::with_capacity(prefix.len() + DESCRIMINATOR_LEN);
		buf.extend(prefix.as_bytes());
		buf.extend(rand::thread_rng().sample_iter(&Alphanumeric).take(DESCRIMINATOR_LEN));

		let s = std::str::from_utf8(&buf)
			.expect("the string is collected from a valid utf-8 sequence; qed");

		let mut file = dir.to_owned();
		file.push(s);
		file
	}

	const NUM_RETRIES: usize = 50;

	for _ in 0..NUM_RETRIES {
		let candidate_path = tmppath(prefix, dir);
		if !candidate_path.exists() {
			return Ok(candidate_path)
		}
	}

	Err(io::Error::new(io::ErrorKind::Other, "failed to create a temporary file"))
}

/// The same as [`tmpfile_in`], but uses [`std::env::temp_dir`] as the directory.
pub async fn tmpfile(prefix: &str) -> io::Result<PathBuf> {
	let temp_dir = PathBuf::from(std::env::temp_dir());
	tmpfile_in(prefix, &temp_dir).await
}

/// A struct that represents an idle worker.
///
/// This struct is supposed to be used as a token that is passed by move into a subroutine that
/// initiates a job. If the worker dies on the duty, then the token is not returned.
#[derive(Debug)]
pub struct IdleWorker {
	/// The stream to which the child process is connected.
	pub stream: UnixStream,

	/// The identifier of this process. Used to reset the niceness.
	pub pid: u32,
}

/// An error happened during spawning a worker process.
#[derive(Clone, Debug)]
pub enum SpawnErr {
	/// Cannot obtain a temporary file location.
	TmpFile,
	/// Cannot bind the socket to the given path.
	Bind,
	/// An error happened during accepting a connection to the socket.
	Accept,
	/// An error happened during spawning the process.
	ProcessSpawn,
	/// The deadline allotted for the worker spawning and connecting to the socket has elapsed.
	AcceptTimeout,
	/// Failed to send handshake after successful spawning was signaled
	Handshake,
}

/// This is a representation of a potentially running worker. Drop it and the process will be killed.
///
/// A worker's handle is also a future that resolves when it's detected that the worker's process
/// has been terminated. Since the worker is running in another process it is obviously not
/// necessary to poll this future to make the worker run, it's only for termination detection.
///
/// This future relies on the fact that a child process's stdout `fd` is closed upon it's termination.
#[pin_project]
pub struct WorkerHandle {
	child: process::Child,
	child_id: u32,
	#[pin]
	stdout: process::ChildStdout,
	program: PathBuf,
	drop_box: Box<[u8]>,
}

impl WorkerHandle {
	fn spawn(
		program: impl AsRef<Path>,
		extra_args: &[&str],
		socket_path: impl AsRef<Path>,
	) -> io::Result<Self> {
		let mut child = process::Command::new(program.as_ref())
			.args(extra_args)
			.arg("--socket-path")
			.arg(socket_path.as_ref().as_os_str())
			.stdout(std::process::Stdio::piped())
			.kill_on_drop(true)
			.spawn()?;

		let child_id = child
			.id()
			.ok_or(io::Error::new(io::ErrorKind::Other, "could not get id of spawned process"))?;
		let stdout = child
			.stdout
			.take()
			.expect("the process spawned with piped stdout should have the stdout handle");

		Ok(WorkerHandle {
			child,
			child_id,
			stdout,
			program: program.as_ref().to_path_buf(),
			// We don't expect the bytes to be ever read. But in case we do, we should not use a buffer
			// of a small size, because otherwise if the child process does return any data we will end up
			// issuing a syscall for each byte. We also prefer not to do allocate that on the stack, since
			// each poll the buffer will be allocated and initialized (and that's due `poll_read` takes &mut [u8]
			// and there are no guarantees that a `poll_read` won't ever read from there even though that's
			// unlikely).
			//
			// OTOH, we also don't want to be super smart here and we could just afford to allocate a buffer
			// for that here.
			drop_box: vec![0; 8192].into_boxed_slice(),
		})
	}

	/// Returns the process id of this worker.
	pub fn id(&self) -> u32 {
		self.child_id
	}
}

impl futures::Future for WorkerHandle {
	type Output = ();

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let me = self.project();
		// Create a `ReadBuf` here instead of storing it in `WorkerHandle` to avoid a lifetime
		// parameter on `WorkerHandle`. Creating the `ReadBuf` is fairly cheap.
		let mut read_buf = ReadBuf::new(&mut *me.drop_box);
		match futures::ready!(AsyncRead::poll_read(me.stdout, cx, &mut read_buf)) {
			Ok(()) => {
				if read_buf.filled().len() > 0 {
					// weird, we've read something. Pretend that never happened and reschedule
					// ourselves.
					cx.waker().wake_by_ref();
					Poll::Pending
				} else {
					// Nothing read means `EOF` means the child was terminated. Resolve.
					Poll::Ready(())
				}
			},
			Err(err) => {
				// The implementation is guaranteed to not to return `WouldBlock` and Interrupted. This
				// leaves us with legit errors which we suppose were due to termination.

				// Log the status code.
				gum::debug!(
					target: LOG_TARGET,
					worker_pid = %me.child_id,
					status_code = ?me.child.try_wait().ok().flatten().map(|c| c.to_string()),
					"pvf worker ({}): {:?}",
					me.program.display(),
					err,
				);
				Poll::Ready(())
			},
		}
	}
}

impl fmt::Debug for WorkerHandle {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "WorkerHandle(pid={})", self.id())
	}
}

/// Convert the given path into a byte buffer.
pub fn path_to_bytes(path: &Path) -> &[u8] {
	// Ideally, we take the `OsStr` of the path, send that and reconstruct this on the other side.
	// However, libstd doesn't provide us with such an option. There are crates out there that
	// allow for extraction of a path, but TBH it doesn't seem to be a real issue.
	//
	// However, should be there reports we can incorporate such a crate here.
	path.to_str().expect("non-UTF-8 path").as_bytes()
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
