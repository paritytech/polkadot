// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Hierarchical messages.
//!
//! Writing concurrent and even multithreaded by default is inconvenient and slow: No references
//! hence lots of needless cloning and data duplication, locks, mutexes, ... We should reach
//! for concurrency and parallelism when there is an actual need, not just because we can and it is
//! reasonably safe in Rust.
//!
//! I very much agree with many points in this blog post for example:
//!
//! https://maciej.codes/2022-06-09-local-async.html
//!
//! Another very good post by Pierre (Tomaka):
//!
//! https://tomaka.medium.com/a-look-back-at-asynchronous-rust-d54d63934a1c
//!
//! This module helps with this in part. It does not break the multithreaded by default approach,
//! but it breaks the `spawn everything` approach. So once you `spawn` you will still be
//! multithreaded by default, despite that for most tasks we spawn (which just wait for network or some
//! message to arrive), that is very much pointless and needless overhead. You will just spawn less in
//! the first place.
//!
//! By default your code is single threaded, except when actually needed:
//!		- need to wait for long running synchronous IO (a threaded runtime is actually useful here)
//!		- need to wait for some async event (message to arrive)
//!		- need to do some hefty CPU bound processing (a thread is required here as well)
//!
//! and it is not acceptable to block the main task for waiting for the result, because we actually
//! really have other things to do or at least need to stay responsive just in case.
//!
//! With the types and traits in this module you can achieve exactly that: You write modules which
//! just execute logic and can call into the functions of other modules - yes we are calling normal
//! functions. For the case a module you are calling into requires an occasional background task,
//! you provide it with a `NestingSender<M, ChildModuleMessage>` that it can pass to any spawned tasks.
//!
//! This way you don't have to spawn a task for each module just for it to be able to handle
//! asynchronous events. The module relies on the using/enclosing code/module to forward it any
//! asynchronous messages in a structured way.
//!
//! What makes this architecture nice is the separation of concerns - at the top you only have to
//! provide a sender and dispatch received messages to the root module - it is completely
//! irrelevant how complex that module is, it might consist of child modules also having the need
//! to spawn and receive messages, which in turn do the same, still the root logic stays unchanged.
//! Everything is isolated to the level where it belongs, while we still keep a single task scope
//! in all non blocking/not CPU intensive parts, which allows us to share data via references for
//! example.
//!
//! Because the wrapping is optional and transparent to the lower modules, each module can also be
//! used at the top directly without any wrapping, e.g. for standalone use or for testing purposes.
//!
//! In the interest of time I refrain from providing a usage example here for now, but would instead
//! like to point you to the dispute-distribution subsystem which makes use of this architecture.
//!
//! Nothing is ever for free of course: Each level adds an indirect function call to message
//! sending. which should be cheap enough for most applications, but something to keep in mind. In
//! particular we avoided the use of of async traits, which would have required memory allocations
//! on each send. Also cloning of `NestingSender` is more expensive than cloning a plain
//! mpsc::Sender, the overhead should be negligible though.
//!
//! Further limitations: Because everything is routed to the same channel, it is not possible with
//! this approach to put back pressure on only a single source (as all are the same). If a module
//! has a task that requires this, it indeed has to spawn a long running task which can do the
//! back-pressure on that message source or we make it its own subsystem. This is just one of the
//! situations that justifies the complexity of asynchronism.

use std::{convert::identity, sync::Arc};

use futures::{channel::mpsc, SinkExt};

/// A message sender that supports nested messages.
///
/// This sender wraps an `mpsc::Sender` and a conversion function for converting given messages of
/// type `M1` to the message type actually supported by the mpsc (`M`).
pub struct NestingSender<M, M1> {
	sender: mpsc::Sender<M>,
	conversion: Arc<dyn Fn(M1) -> M + 'static + Send + Sync>,
}

impl<M> NestingSender<M, M>
where
	M: 'static,
{
	/// Create a new "root" sender.
	///
	/// This is a sender that directly passes messages to the mpsc.
	pub fn new_root(root: mpsc::Sender<M>) -> Self {
		Self { sender: root, conversion: Arc::new(identity) }
	}
}

impl<M, M1> NestingSender<M, M1>
where
	M: 'static,
	M1: 'static,
{
	/// Create a new `NestingSender` which wraps a given "parent" sender.
	///
	/// Doing the necessary conversion from the child message type `M1` to the parent message type
	/// `M2.
	/// M1 -> M2 -> M
	/// Inputs:
	///		F(M2) -> M (via parent)
	///		F(M1) -> M2 (via child_conversion)
	/// Result: F(M1) -> M
	pub fn new<M2>(parent: NestingSender<M, M2>, child_conversion: fn(M1) -> M2) -> Self
	where
		M2: 'static,
	{
		let NestingSender { sender, conversion } = parent;
		Self { sender, conversion: Arc::new(move |x| conversion(child_conversion(x))) }
	}

	/// Send a message via the underlying mpsc.
	///
	/// Necessary conversion is accomplished.
	pub async fn send_message(&mut self, m: M1) -> Result<(), mpsc::SendError> {
		// Flushing on an mpsc means to wait for the receiver to pick up the data - we don't want
		// to wait for that.
		self.sender.feed((self.conversion)(m)).await
	}
}

// Helper traits and implementations:

impl<M, M1> Clone for NestingSender<M, M1>
where
	M: 'static,
	M1: 'static,
{
	fn clone(&self) -> Self {
		Self { sender: self.sender.clone(), conversion: self.conversion.clone() }
	}
}
