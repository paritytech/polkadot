use futures::prelude::*;
use malus::overseer::Handle;
use polkadot_node_primitives::SpawnNamed;
use std::pin::Pin;

pub const MALUS: &str = "MALUS😈😈😈";

pub(crate) const MALICIOUS_POV: &[u8] = "😈😈valid😈😈".as_bytes();

pub(crate) fn launch_processing_task<F, X, Q>(
	spawner: impl SpawnNamed,
	overseer: Handle,
	queue: Q,
	action: F,
) where
	F: Future<Output = ()>,
	Q: Stream<Item = X>,
	X: Send,
{
	spawner.spawn(
		"nemesis",
		Pin::new(async move {
			queue.for_each(move |input| spawner.spawn("nemesis-inner", Box::pin(action)))
		}),
	);
}
