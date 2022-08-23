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
//! multithreaded by default, although for most tasks we spawn (which just wait for network or some
//! message to arrive), that is very much pointless and needless overhead, you will spawn less in
//! the first place.
//!
//! By the default your code is single threaded, except when actually needed:
//!		- need to wait for long running synchronous IO (a threaded runtime is actually useful here)
//!		- need to wait for some async event (message to arrive)
//!		- need to do some hefty CPU bound processing
//!
//!	and it is not acceptable to block the main task for waiting for the result, because we actually
//!	really have other things to do or at least need to stay responsive just in case.
//!
//!	With the types and traits in this module you can achieve exactly that: You write modules which
//!	just execute logic and can call into the functions of other modules - yes we are calling normal
//!	functions. For the case a module you are calling into requires an occasional background task,
//!	you provide it with a `MessageSender` that it can pass to any spawned tasks.
//!
//!	The interesting part is, that we get full separation of concerns: The called module only needs
//!	to care about its own message type. The calling module, takes care of wrapping up the messages
//!	in a containing message type (if any). This way you can build full trees of modules all ending
//!	up sending via the same channel and you end up with a single receiver at the top which needs to
//!	awaited and then you can dispatch messages in a structured fashion again.
//!
//!	Because the wrapping is optional and transparent to the lower modules, each module can also be
//!	used at the top directly without any wrapping, e.g. for standalone use or for testing purposes.
//!
//!	In the interest of time I refrain from providing a usage example here for now, but instead
//!	refer you to the dispute-distribution subsystem which makes use of it.
//!
//!	What makes this architecture nice is the separation of concerns - at the top you only have to
//!	provide a sender and dispatch received messages to the root module - it is completely
//!	irrelevant how complex that module is, it might consist of child modules also having the need
//!	to spawn and receive messages, which in turn do the same, still the root logic stays unchanged.
//!	Everything is isolated to the level where it belongs, while we still keep a single task scope
//!	in any non blocking/CPU intensive parts, which allows us to share data via references for
//!	example.
//!
//!	Nothing is ever for free though: Each level adds an indirect function call to message sending.
//!	which should be cheap enough for most applications. In particular we avoided the use of of
//!	async traits, which would have required memory allocations on each send.
//!
//!	TODO: Remove async trait requirement as this means an allocation for each message to be sent.

use std::{marker::PhantomData, convert::identity};

use futures::mpsc;

struct RootSender {}
struct ChildSender {}
struct GrandChildSender {}

struct MessageSender<M, M1> {
	sender: mpsc::Sender<M>,
	conversion: Box<dyn Fn(M1) -> M>,
}

trait MessageConversion<P, M> {

	fn convert(message: M) -> P;
	fn box_clone(self: Box<Self>) -> Box<Self>;
}

struct ParentChild { }

enum ParentMessage {
	Child(ChildMessage)
}

enum ChildMessage {
	Huhu,
	GrandChild(GrandChildMessage),

}

enum GrandChildMessage {
	Haha,
}

impl MessageConversion<ParentMessage, ChildMessage> for ParentChild {

	fn convert(message: ChildMessage) -> ParentMessage {
		ParentMessage::Child(message)
	}

	fn box_clone(self: Box<Self>) -> Box<Self> {
		Box::new(ParentChild {})
	}
}

impl<M> MessageSender<M, M>
{
	pub fn new_root(root: mpsc::Sender<M>) -> Self {
		Self {
			sender: root,
			conversion: Box::new(identity),
		}
	}
}

impl<M, M1> MessageSender<M, M1> {
	/// M1 -> M2 -> M 
	/// Inputs:
	///		F(M2) -> M (via parent)
	///		F(M1) -> M2 (via child_conversion)
	/// Result: F(M1) -> M 
	pub fn new<M2, F>(parent: MessageSender<M, M2>, child_conversion: F) -> Self 
	where 
	F: Fn(M1) -> M2 {
		let MessageSender { sender, conversion } = parent;
		Self {
			sender,
			conversion: Box::new(|x| conversion(child_conversion(x)))
		}
	}

	pub async fn send_message(&mut self, m: M1) {
		self.sender.feed((*self.conversion)(m)).await
	}
}

// impl<M1, M, F> MessageSender<M, M1, F>  
// where
//     F: Fn(M1) -> M, {
	//
	// M1 -> M2 -> M 
	// Inputs:
	//		F(M2) -> M
	//		F(M1) -> M2
	// Result: F(M1) -> M 
	//
//     pub fn new_nested<M2, F2, F3>(parent: MessageSender<M, F2>, nested_conversion: F3) -> Self 
//         where
//             F2: Fn(M2) -> M,
//             F3: Fn(M1) -> M2 {
//                 let Self { sender, conversion } = parent;
//                 let full_conversion = |m| conversion(nested_conversion(m));
//                 Self {
//                     sender,
//                     conversion: full_conversion,
//                 }
//             }
// }
