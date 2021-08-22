use futures::prelude::*;
use polkadot_node_primitives::SpawnNamed;

pub const MALUS: &str = "MALUS😈😈😈";

#[allow(unused)]
pub(crate) const MALICIOUS_POV: &[u8] = "😈😈pov_looks_valid_to_me😈😈".as_bytes();

/// Launch a service task for each item in the provided queue.
#[allow(unused)]
pub(crate) fn launch_processing_task<X, F, U, Q, S>(spawner: S, queue: Q, action: F)
where
	F: Fn(X) -> U + Send + 'static,
	U: Future<Output = ()> + Send + 'static,
	Q: Stream<Item = X> + Send + 'static,
	X: Send,
	S: 'static + SpawnNamed + Clone + Unpin,
{
	let spawner2 = spawner.clone();
	spawner2.spawn(
		"nemesis-queue-processor",
		Box::pin(async move {
			let spawner = spawner.clone();
			queue
				.for_each(move |input| {
					spawner.spawn("nemesis-task", Box::pin(action(input)));
					async move { () }
				})
				.await;
		}),
	);
}
