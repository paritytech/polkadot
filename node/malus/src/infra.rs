

/// Filter incoming and outgoing messages.
trait MsgFilter: Send + Clone + 'static {

	type Message: Send + 'static;

	/// Filter messages that are to be received by
	/// the subsystem.
	fn filter_in(&self, msg: Self::Message) -> Option<Self::Message> {
		Some(msg)
	}

	/// Modify outgoing messages.
	fn filter_out(&self, msg: Self::Message) -> Option<Self::Message> {
		Some(msg)
	}
}

/////////////

#[derive(Clone)]
struct FilteredSender<Sender, Fil> {
	inner: Sender,
	message_filter: Fil,
}

#[async_trait::async_trait]
impl SubsystemSender for FilteredSender<Sender> where Sender: SubsystemSender {
	async fn send_message(&mut self, msg: AllMessages) {
		if let Some(msg) = self.message_filter.filter_out(msg) {
			self.inner.send_message(msg);
		}
	}

	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send {
		for msg in msgs {
			self.send_message(msg)
		}
	}

	fn send_unbounded_message(&mut self, msg: AllMessages) {
		if let Some(msg) = self.message_filter.filter_out(msg) {
			self.inner.send_unbounded_message(msg);
		}
	}

}


/////////////


struct FilteredContext<X, Fil>{
	inner: X,
	message_filter: Fil,
};


impl FilteredContext<X: SubsystemContext, Fil: MsgFilter> {
	pub fn new(inner: X, message_filter: Fil) {
		Self {
			inner,
			message_filter,
		}
	}
}

impl<X: SubsystemContext, Fil: MsgFilter> SubsystemContext for FilteredContext<X> {
	type Message = <X as SubsystemContext>::Message;
	type Sender = FilteredSender<
		<X as SubsystemContext>::Sender,
		,
	>;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message>>, ()> {
		self.0.try_recv()
	}

	/// Receive a message.
	async fn recv(&mut self) -> SubsystemResult<FromOverseer<Self::Message>> {
		self.0.recv()
	}

	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()> {
		self.0.spawn(name, s)
	}

	async fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()> {
		self.0.spawn_blocking(name, s)
	}

	fn sender(&mut self) -> &mut Self::Sender {
		self.0.sender()
	}
}

struct FilteredSubsystem<Sub, Fil> {
	pub subsystem: Sub,
	pub message_filter: Fil,
}

impl<Context, Sub, Fil> FilteredSubsystem<Sub, Fil>
where
	Context: SubsystemContext,
	Sub: Subsystem<Context>,
	Fil: MsgFilter<Message = <Context as SubsystemContext>::Message>,
{
	fn new(subsystem: Sub, message_filter: Fil) -> Self {
		Self {
			subsystem,
			message_filter,
		}
	}
}

impl<Context, Sub> Subsystem<Context> for FilteredSubsystem<Sub>
where
	Sub: Subsystem<Context>,
	Context: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawendSubsystem {
		self.subsystem.start(ctx)
	}
}
