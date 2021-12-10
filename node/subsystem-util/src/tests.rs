// Copyright 2020 Parity Technologies (UK) Ltd.
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

use super::*;
use assert_matches::assert_matches;
use executor::block_on;
use futures::{channel::mpsc, executor, future, Future, FutureExt, SinkExt, StreamExt};
use polkadot_node_jaeger as jaeger;
use polkadot_node_subsystem::{
	messages::{AllMessages, CollatorProtocolMessage},
	ActivatedLeaf, ActiveLeavesUpdate, FromOverseer, LeafStatus, OverseerSignal, SpawnedSubsystem,
};
use polkadot_node_subsystem_test_helpers::{self as test_helpers, make_subsystem_context};
use polkadot_primitives::v1::Hash;
use std::{
	pin::Pin,
	sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	},
	time::Duration,
};
use thiserror::Error;

// basic usage: in a nutshell, when you want to define a subsystem, just focus on what its jobs do;
// you can leave the subsystem itself to the job manager.

// for purposes of demonstration, we're going to whip up a fake subsystem.
// this will 'select' candidates which are pre-loaded in the job

// job structs are constructed within JobTrait::run
// most will want to retain the sender and receiver, as well as whatever other data they like
struct FakeCollatorProtocolJob {
	receiver: mpsc::Receiver<CollatorProtocolMessage>,
}

// Error will mostly be a wrapper to make the try operator more convenient;
// deriving From implementations for most variants is recommended.
// It must implement Debug for logging.
#[derive(Debug, Error)]
enum Error {
	#[error(transparent)]
	Sending(#[from] mpsc::SendError),
}

impl JobTrait for FakeCollatorProtocolJob {
	type ToJob = CollatorProtocolMessage;
	type Error = Error;
	type RunArgs = bool;
	type Metrics = ();

	const NAME: &'static str = "fake-collator-protocol-job";

	/// Run a job for the parent block indicated
	//
	// this function is in charge of creating and executing the job's main loop
	fn run<S: SubsystemSender>(
		_: Hash,
		_: Arc<jaeger::Span>,
		run_args: Self::RunArgs,
		_metrics: Self::Metrics,
		receiver: mpsc::Receiver<CollatorProtocolMessage>,
		mut sender: JobSender<S>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = FakeCollatorProtocolJob { receiver };

			if run_args {
				sender
					.send_message(CollatorProtocolMessage::Invalid(
						Default::default(),
						Default::default(),
					))
					.await;
			}

			// it isn't necessary to break run_loop into its own function,
			// but it's convenient to separate the concerns in this way
			job.run_loop().await
		}
		.boxed()
	}
}

impl FakeCollatorProtocolJob {
	async fn run_loop(mut self) -> Result<(), Error> {
		loop {
			match self.receiver.next().await {
				Some(_csm) => {
					unimplemented!("we'd report the collator to the peer set manager here, but that's not implemented yet");
				},
				None => break,
			}
		}

		Ok(())
	}
}

// with the job defined, it's straightforward to get a subsystem implementation.
type FakeCollatorProtocolSubsystem<Spawner> = JobSubsystem<FakeCollatorProtocolJob, Spawner>;

// this type lets us pretend to be the overseer
type OverseerHandle = test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>;

fn test_harness<T: Future<Output = ()>>(run_args: bool, test: impl FnOnce(OverseerHandle) -> T) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(None, log::LevelFilter::Trace)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, overseer_handle) = make_subsystem_context(pool.clone());

	let subsystem = FakeCollatorProtocolSubsystem::new(pool, run_args, ()).run(context);
	let test_future = test(overseer_handle);

	futures::pin_mut!(subsystem, test_future);

	executor::block_on(async move {
		future::join(subsystem, test_future)
			.timeout(Duration::from_secs(2))
			.await
			.expect("test timed out instead of completing")
	});
}

#[test]
fn starting_and_stopping_job_works() {
	let relay_parent: Hash = [0; 32].into();

	test_harness(true, |mut overseer_handle| async move {
		overseer_handle
			.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: relay_parent,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;
		assert_matches!(overseer_handle.recv().await, AllMessages::CollatorProtocol(_));
		overseer_handle
			.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(relay_parent),
			)))
			.await;

		overseer_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
	});
}

#[test]
fn sending_to_a_non_running_job_do_not_stop_the_subsystem() {
	let relay_parent = Hash::repeat_byte(0x01);

	test_harness(true, |mut overseer_handle| async move {
		overseer_handle
			.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: relay_parent,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		// send to a non running job
		overseer_handle
			.send(FromOverseer::Communication { msg: Default::default() })
			.await;

		// the subsystem is still alive
		assert_matches!(overseer_handle.recv().await, AllMessages::CollatorProtocol(_));

		overseer_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
	});
}

#[test]
fn test_subsystem_impl_and_name_derivation() {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, _) = make_subsystem_context::<CollatorProtocolMessage, _>(pool.clone());

	let SpawnedSubsystem { name, .. } =
		FakeCollatorProtocolSubsystem::new(pool, false, ()).start(context);
	assert_eq!(name, "fake-collator-protocol");
}

#[test]
fn tick_tack_metronome() {
	let n = Arc::new(AtomicUsize::default());

	let (tick, mut block) = mpsc::unbounded();

	let metronome = {
		let n = n.clone();
		let stream = Metronome::new(Duration::from_millis(137_u64));
		stream
			.for_each(move |_res| {
				let _ = n.fetch_add(1, Ordering::Relaxed);
				let mut tick = tick.clone();
				async move {
					tick.send(()).await.expect("Test helper channel works. qed");
				}
			})
			.fuse()
	};

	let f2 = async move {
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 1_usize);
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 2_usize);
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 3_usize);
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 4_usize);
	}
	.fuse();

	futures::pin_mut!(f2);
	futures::pin_mut!(metronome);

	block_on(async move {
		// futures::join!(metronome, f2)
		futures::select!(
			_ = metronome => unreachable!("Metronome never stops. qed"),
			_ = f2 => (),
		)
	});
}
