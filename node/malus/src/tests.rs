use super::*;

use polkadot_node_subsystem_test_helpers::*;

use polkadot_node_subsystem::{
	messages::{AllMessages, AvailabilityStoreMessage},
	DummySubsystem,
	overseer::Subsystem,
};

#[derive(Clone, Debug)]
struct BlackHoleInterceptor;

impl<Sender> MessageInterceptor<Sender> for BlackHoleInterceptor
where
	Sender: overseer::SubsystemSender<AllMessages> + overseer::SubsystemSender<AvailabilityStoreMessage> + Clone + 'static,
{
	type Message = AvailabilityStoreMessage;
	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		None
	}
}

async fn overseer_send<T: Into<AllMessages>> (
	overseer: &mut TestSubsystemContextHandle<T>,
	msg: T,
) {
	overseer
		.send(FromOverseer::Communication { msg })
		.await;
}


#[test]
fn integrity_test() {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, mut overseer) = make_subsystem_context(pool);

	let sub = DummySubsystem;

	let sub_intercepted = InterceptedSubsystem::new(sub, BlackHoleInterceptor);


	// Try to send a message we know is going to be filtered.
	let test_fut = async move {
		let (tx, rx) = futures::channel::oneshot::channel();
		overseer_send(
			&mut overseer,
				AvailabilityStoreMessage::QueryChunk(Default::default(), 0.into(), tx)
			).await;
		rx.await;
		overseer
	};
	let subsystem = async move { sub_intercepted.start(context).await; };

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	futures::executor::block_on(futures::future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await
		},
		subsystem,
	))
	.1
	.unwrap();



}