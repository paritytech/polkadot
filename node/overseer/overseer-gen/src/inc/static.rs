trait MapSubsystem<T> {
	type Output;

	fn map_subsystem(&self, sub: T) -> Self::Output;
}

impl<F, T, U> MapSubsystem<T> for F where F: Fn(T) -> U {
	type Output = U;

	fn map_subsystem(&self, sub: T) -> U {
		(self)(sub)
	}
}

type SubsystemIncomingMessages<M> = ::futures::stream::Select<
	::metered::MeteredReceiver<MessagePacket<M>>,
	::metered::UnboundedMeteredReceiver<MessagePacket<M>>,
>;



#[derive(Debug, Default, Clone)]
struct SignalsReceived(Arc<AtomicUsize>);

impl SignalsReceived {
	fn load(&self) -> usize {
		self.0.load(atomic::Ordering::SeqCst)
	}

	fn inc(&self) {
		self.0.fetch_add(1, atomic::Ordering::SeqCst);
	}
}

/// A sender from subsystems to other subsystems.
#[derive(Debug, Clone)]
pub struct OverseerSubsystemSender {
	channels: ChannelsOut,
	signals_received: SignalsReceived,
}

#[async_trait::async_trait]
impl SubsystemSender for OverseerSubsystemSender {
	async fn send_message(&mut self, msg: AllMessages) {
		self.channels.send_and_log_error(self.signals_received.load(), msg).await;
	}

	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		// This can definitely be optimized if necessary.
		for msg in msgs {
			self.send_message(msg).await;
		}
	}

	fn send_unbounded_message(&mut self, msg: AllMessages) {
		self.channels.send_unbounded_and_log_error(self.signals_received.load(), msg);
	}
}

#[derive(Clone)]
struct SubsystemMeters {
	bounded: metered::Meter,
	unbounded: metered::Meter,
	signals: metered::Meter,
}

impl SubsystemMeters {
	fn read(&self) -> SubsystemMeterReadouts {
		SubsystemMeterReadouts {
			bounded: self.bounded.read(),
			unbounded: self.unbounded.read(),
			signals: self.signals.read(),
		}
	}
}

struct SubsystemMeterReadouts {
	bounded: metered::Readout,
	unbounded: metered::Readout,
	signals: metered::Readout,
}
